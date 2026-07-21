/**
 * ZcodeSqliteObserver —— 只读观察 ZCode Desktop 的两个 sqlite DB，
 * 把会话状态映射成 SessionSnapshot 列表并广播给订阅者。
 *
 * 数据来源：
 *   - ~/.zcode/v2/tasks-index.sqlite  →  tasks 表（任务元信息）
 *   - ~/.zcode/cli/db/db.sqlite       →  tool_usage 表（含 approval_status）
 *
 * 观察策略：
 *   - 以只读方式打开 tasks DB，ATTACH tool_usage DB（别名为 cli）
 *   - 监听两个 -wal 文件的 mtime（fs.watch + 兜底轮询 + 节流）
 *   - 同时起 pollIntervalMs 定时器兜底
 *   - 查询结果做 JSON 去重，仅在变化时广播
 *
 * 安全：
 *   - ATTACH 路径用 prepared statement 参数绑定，避免注入
 *   - 所有读连接均为 readonly
 */

import { existsSync, statSync, watch, type FSWatcher } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { setTimeout as delay } from 'node:timers'
import Database from 'better-sqlite3'
import type { Database as DatabaseType, Statement as StatementType } from 'better-sqlite3'

import type { SessionSnapshot } from '@agent-deck/protocol'

import { mapZcodeRow } from './mapper.js'
import type { ZcodeRow } from './mapper.js'

// ─────────────────────────────────────────────────────────────────────────────
// 默认路径
// ─────────────────────────────────────────────────────────────────────────────

const ZCODE_HOME = join(homedir(), '.zcode')
const DEFAULT_TASKS_DB = join(ZCODE_HOME, 'v2', 'tasks-index.sqlite')
const DEFAULT_TOOL_DB = join(ZCODE_HOME, 'cli', 'db', 'db.sqlite')

/** 防陈旧窗口：tool_usage 的 started_at 早于该时间戳的 pending 视为僵尸 */
const STALE_WINDOW_MS = 30 * 60 * 1000 // 30 min

/** wal 事件节流（避免高频写入导致连击） */
const THROTTLE_MS = 500

/** 兜底轮询间隔（即使 fs.watch 失效也会定期查） */
const DEFAULT_POLL_INTERVAL_MS = 500

// ─────────────────────────────────────────────────────────────────────────────
// Options
// ─────────────────────────────────────────────────────────────────────────────

export interface SqliteObserverOptions {
  /** tasks DB 路径，默认 ~/.zcode/v2/tasks-index.sqlite */
  tasksDbPath?: string
  /** tool_usage DB 路径，默认 ~/.zcode/cli/db/db.sqlite */
  toolDbPath?: string
  /** 轮询间隔（ms），默认 500 */
  pollIntervalMs?: number
  /** 自指防护：排除的 workspace 路径（包含匹配） */
  excludeWorkspaces?: string[]
  /** 自指防护：排除的 sessionId（相等匹配） */
  excludeTaskIds?: string[]
  /** DB 文件不存在时是否报错（false=静默返回空） */
  failOnMissing?: boolean
  /** 日志函数 */
  log?: (msg: string, ...args: unknown[]) => void
}

// ─────────────────────────────────────────────────────────────────────────────
// 查询
// ─────────────────────────────────────────────────────────────────────────────

/**
 * 跨库 join 查询：tasks × cli.tool_usage。
 *
 * waiting 字段：存在 started_at 在 30 分钟内、状态为 running、
 * approval_status 为 requested、尚未完成的 tool_usage 行即为 1。
 * detail：最新一条待批准 tool 的 tool_name + side_effect_scope。
 */
const QUERY_SQL = `
SELECT
  t.task_id            AS task_id,
  t.title              AS title,
  t.task_status        AS task_status,
  t.workspace_path     AS workspace_path,
  t.workspace_identity AS workspace_identity,
  t.workspace_key      AS workspace_key,
  t.provider           AS provider,
  t.mode               AS mode,
  t.model              AS model,
  t.created_at         AS created_at,
  t.updated_at         AS updated_at,
  t.unread_at          AS unread_at,
  t.pinned             AS pinned,
  t.archived           AS archived,
  t.deleted            AS deleted,
  CASE WHEN EXISTS(
    SELECT 1 FROM cli.tool_usage tu
    WHERE tu.session_id = t.task_id
      AND tu.approval_status = 'requested'
      AND tu.status = 'running'
      AND tu.completed_at IS NULL
      AND tu.started_at > (strftime('%s','now') - ${Math.floor(STALE_WINDOW_MS / 1000)}) * 1000
  ) THEN 1 ELSE 0 END AS waiting,
  (
    SELECT tu.tool_name || ': ' || COALESCE(tu.side_effect_scope, '')
    FROM cli.tool_usage tu
    WHERE tu.session_id = t.task_id
      AND tu.approval_status = 'requested'
      AND tu.status = 'running'
      AND tu.completed_at IS NULL
    ORDER BY tu.started_at DESC
    LIMIT 1
  ) AS detail
FROM tasks t
WHERE t.task_status IN ('running', 'completed', 'error')
  AND t.deleted = 0
  AND t.archived = 0
ORDER BY t.updated_at DESC
LIMIT 20
`

// ─────────────────────────────────────────────────────────────────────────────
// Observer
// ─────────────────────────────────────────────────────────────────────────────

export class ZcodeSqliteObserver {
  private readonly tasksDbPath: string
  private readonly toolDbPath: string
  private readonly pollIntervalMs: number
  private readonly excludeWorkspaces: string[]
  private readonly excludeTaskIds: Set<string>
  private readonly failOnMissing: boolean
  private readonly log: (msg: string, ...args: unknown[]) => void

  private db: DatabaseType | null = null
  private stmt: StatementType | null = null

  private listeners = new Set<(snapshots: SessionSnapshot[]) => void>()
  private lastSnapshots: SessionSnapshot[] = []
  private lastSignature = ''

  private started = false
  private pollTimer: NodeJS.Timeout | null = null
  private walWatchers: FSWatcher[] = []
  /** mtime 兜底轮询 */
  private mtimeTimer: NodeJS.Timeout | null = null
  private lastTasksWalMtime = 0
  private lastToolWalMtime = 0
  /** 节流句柄 */
  private throttleTimer: NodeJS.Timeout | null = null

  constructor(opts: SqliteObserverOptions = {}) {
    this.tasksDbPath = opts.tasksDbPath ?? DEFAULT_TASKS_DB
    this.toolDbPath = opts.toolDbPath ?? DEFAULT_TOOL_DB
    this.pollIntervalMs = opts.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS
    this.excludeWorkspaces = opts.excludeWorkspaces ?? []
    this.excludeTaskIds = new Set(opts.excludeTaskIds ?? [])
    this.failOnMissing = opts.failOnMissing ?? false
    this.log = opts.log ?? (() => {})
  }

  // ─── 生命周期 ──────────────────────────────────────────────────────────────

  /**
   * 开始观察：
   *   - 检查 DB 文件
   *   - 只读打开 tasks DB
   *   - 参数化 ATTACH tool_usage DB
   *   - 预编译查询
   *   - 启动 fs.watch + mtime 兜底 + 定时兜底
   */
  start(): void {
    if (this.started) return
    this.started = true

    const tasksExists = existsSync(this.tasksDbPath)
    const toolExists = existsSync(this.toolDbPath)

    if (!tasksExists || !toolExists) {
      if (this.failOnMissing) {
        this.started = false
        throw new Error(
          `[zcode-observer] DB missing: tasks=${this.tasksDbPath} (${tasksExists}), tool=${this.toolDbPath} (${toolExists})`,
        )
      }
      this.log(
        '[zcode-observer] DB missing, running in passive mode: tasks=%s tool=%s',
        tasksExists,
        toolExists,
      )
      // 仍启动定时器，以便后续文件出现时能感知（mtime 兜底）
      this.startTimers()
      return
    }

    try {
      this.openDb()
    } catch (err) {
      this.started = false
      if (this.failOnMissing) throw err
      this.log('[zcode-observer] openDb failed, passive mode: %o', err)
      this.started = true
      this.startTimers()
      return
    }

    this.startTimers()
    // 立即拉一次，让订阅者拿到初始状态
    try {
      this.pollOnce()
    } catch (err) {
      this.log('[zcode-observer] initial pollOnce failed: %o', err)
    }
  }

  /** 关闭观察、释放所有资源 */
  stop(): void {
    this.started = false

    if (this.pollTimer) {
      clearInterval(this.pollTimer)
      this.pollTimer = null
    }
    if (this.mtimeTimer) {
      clearInterval(this.mtimeTimer)
      this.mtimeTimer = null
    }
    if (this.throttleTimer) {
      clearTimeout(this.throttleTimer)
      this.throttleTimer = null
    }
    for (const w of this.walWatchers) {
      try {
        w.close()
      } catch {
        /* ignore */
      }
    }
    this.walWatchers = []

    if (this.db) {
      try {
        // DETACH 防止部分场景下的告警（连接关闭本来也会清理）
        try {
          this.db.exec('DETACH cli')
        } catch {
          /* not attached or already detached */
        }
        this.db.close()
      } catch (err) {
        this.log('[zcode-observer] db.close failed: %o', err)
      }
      this.db = null
      this.stmt = null
    }
  }

  // ─── 订阅 ───────────────────────────────────────────────────────────────────

  /**
   * 订阅快照变化。回调在每次 DB 变化（且签名变化）时调用。
   * 返回 unsubscribe 函数。若已 start，订阅后立即收到一次当前快照。
   */
  onChange(cb: (snapshots: SessionSnapshot[]) => void): () => void {
    this.listeners.add(cb)
    // 初次订阅时回放当前缓存，让调用方拿到首帧
    if (this.lastSnapshots.length > 0) {
      try {
        cb(this.lastSnapshots)
      } catch (err) {
        this.log('[zcode-observer] onChange replay threw: %o', err)
      }
    }
    return () => {
      this.listeners.delete(cb)
    }
  }

  // ─── 查询 ───────────────────────────────────────────────────────────────────

  /**
   * 手动触发一次查询并广播（如有变化）。返回当前快照。
   * DB 未就绪时返回上次缓存（或空数组）。
   */
  pollOnce(): SessionSnapshot[] {
    if (!this.db || !this.stmt) {
      return this.lastSnapshots
    }

    let rows: ZcodeRow[]
    try {
      rows = this.stmt.all() as ZcodeRow[]
    } catch (err) {
      this.log('[zcode-observer] query failed, returning cache: %o', err)
      return this.lastSnapshots
    }

    const snapshots: SessionSnapshot[] = []
    for (const row of rows) {
      let snap: SessionSnapshot
      try {
        snap = mapZcodeRow(row)
      } catch (err) {
        this.log('[zcode-observer] mapZcodeRow failed for task %s: %o', row?.task_id, err)
        continue
      }

      // 自指防护：workspace 包含 / sessionId 相等
      if (this.isExcluded(snap)) continue

      snapshots.push(snap)
    }

    // 去重广播：JSON 签名一致则跳过
    const signature = this.signatureOf(snapshots)
    if (signature !== this.lastSignature) {
      this.lastSignature = signature
      this.lastSnapshots = snapshots
      this.broadcast(snapshots)
    } else {
      // 签名相同也刷新缓存引用（updatedAt 可能已变但签名未变的情况极少）
      this.lastSnapshots = snapshots
    }

    return snapshots
  }

  // ─── 内部：DB 打开 ───────────────────────────────────────────────────────────

  private openDb(): void {
    // 只读打开 tasks DB（fileMustExist:false 以便 passive 模式也能复用同一调用路径，
    // 但此处仅在文件存在时进入）
    const db = new Database(this.tasksDbPath, {
      readonly: true,
      fileMustExist: false,
    })

    // 让 sqlite 在只读连接上仍能读 wal（默认即可，显式确保）
    db.pragma('query_only = ON')

    // 参数化 ATTACH，杜绝注入
    try {
      db.prepare('ATTACH ? AS cli').run(this.toolDbPath)
    } catch (err) {
      try {
        db.close()
      } catch {
        /* ignore */
      }
      throw err
    }

    this.db = db
    this.stmt = db.prepare(QUERY_SQL)
  }

  // ─── 内部：定时器 & 文件监听 ─────────────────────────────────────────────────

  private startTimers(): void {
    // 兜底定时器：即便 fs.watch/mtime 都失效也会定期查
    this.pollTimer = setInterval(() => {
      this.safePoll()
    }, this.pollIntervalMs)
    this.pollTimer.unref?.()

    // fs.watch -wal 文件
    this.watchWal(this.tasksDbPath)
    this.watchWal(this.toolDbPath)

    // mtime 兜底轮询（fs.watch 在某些网络盘/外部卷不可靠）
    this.lastTasksWalMtime = this.walMtime(this.tasksDbPath)
    this.lastToolWalMtime = this.walMtime(this.toolDbPath)
    this.mtimeTimer = setInterval(() => {
      this.checkMtime()
    }, Math.max(this.pollIntervalMs, 500))
    this.mtimeTimer.unref?.()
  }

  private watchWal(dbPath: string): void {
    const walPath = dbPath + '-wal'
    if (!existsSync(walPath)) return
    try {
      const w = watch(walPath, () => {
        this.scheduleThrottledPoll()
      })
      w.on('error', (err) => {
        this.log('[zcode-observer] wal watcher error on %s: %o', walPath, err)
      })
      this.walWatchers.push(w)
    } catch (err) {
      this.log('[zcode-observer] fs.watch unavailable for %s: %o', walPath, err)
    }
  }

  private walMtime(dbPath: string): number {
    const walPath = dbPath + '-wal'
    try {
      return statSync(walPath).mtimeMs
    } catch {
      return 0
    }
  }

  private checkMtime(): void {
    const t = this.walMtime(this.tasksDbPath)
    const c = this.walMtime(this.toolDbPath)
    if (t !== this.lastTasksWalMtime || c !== this.lastToolWalMtime) {
      this.lastTasksWalMtime = t
      this.lastToolWalMtime = c
      this.scheduleThrottledPoll()
    }
  }

  /** 节流：THROTTLE_MS 内多次触发只查一次 */
  private scheduleThrottledPoll(): void {
    if (this.throttleTimer) return
    this.throttleTimer = setTimeout(() => {
      this.throttleTimer = null
      this.safePoll()
    }, THROTTLE_MS)
    this.throttleTimer.unref?.()
  }

  private safePoll(): void {
    try {
      this.pollOnce()
    } catch (err) {
      this.log('[zcode-observer] pollOnce threw: %o', err)
    }
  }

  // ─── 内部：过滤 & 广播 & 签名 ───────────────────────────────────────────────

  private isExcluded(snap: SessionSnapshot): boolean {
    if (this.excludeTaskIds.has(snap.sessionId)) return true
    const wp = snap.workspacePath
    if (wp) {
      for (const ex of this.excludeWorkspaces) {
        if (ex && wp.includes(ex)) return true
      }
    }
    return false
  }

  private broadcast(snapshots: SessionSnapshot[]): void {
    for (const cb of this.listeners) {
      try {
        cb(snapshots)
      } catch (err) {
        this.log('[zcode-observer] listener threw: %o', err)
      }
    }
  }

  /**
   * 计算快照签名用于去重。只比较影响展示/语义的字段，避免 updatedAt 每秒变化导致抖动。
   * 但 updatedAt 仍然纳入，因为任务真实更新需要广播；mapper 输出的 updatedAt 来自
   * DB 行的 updated_at，不会自增。
   */
  private signatureOf(snapshots: SessionSnapshot[]): string {
    return JSON.stringify(
      snapshots.map((s) => ({
        i: s.sessionId,
        s: s.status,
        r: s.risk ?? null,
        d: s.detail ?? null,
        w: s.waitingSince ?? null,
        u: s.updatedAt,
        t: s.title,
      })),
    )
  }
}
