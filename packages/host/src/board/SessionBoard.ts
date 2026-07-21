/**
 * SessionBoard —— host 的「状态总表」。
 *
 * 所有 backend 把观察到的会话快照推给它，它负责：
 *   1. 合并所有 backend 的快照
 *   2. 调用 slotAllocator 决定哪个会话占哪个槽
 *   3. 调用 theme.ts 给每个槽位算出 RGB/亮度/fx
 *   4. 输出最终的 LedFrame（推给设备）+ BoardState（推给 simulator UI）
 *
 * 对外是不可变的数据流：通过 onLedFrame / onBoardState 订阅输出。
 */

import {
  type BackendId,
  type BoardState,
  type LedFrame,
  type LedSlot,
  type PolicyMode,
  type SessionSnapshot,
  type SlotBinding,
  DONE_TTL_MS,
  RISK_BOOST,
  SLOT_COUNT,
} from '@agent-deck/protocol'

import { bus } from '../bus.js'
import type { AgentBackend } from '../backends/types.js'
import {
  allocateSlots,
  type AllocatedSlot,
  type ScoredSession,
} from './slotAllocator.js'
import { CODEX_THEME, paint, type ThemePalette } from './theme.js'

// ─────────────────────────────────────────────────────────────────────────────
// 构造参数
// ─────────────────────────────────────────────────────────────────────────────

export interface SessionBoardOptions {
  /** 槽位数，默认 SLOT_COUNT */
  slotCount?: number
  /** 灯效调色板，默认 CODEX_THEME */
  palette?: ThemePalette
  /** 当前时间函数，默认 Date.now（测试可注入） */
  now?: () => number
}

// ─────────────────────────────────────────────────────────────────────────────
// 内部辅助
// ─────────────────────────────────────────────────────────────────────────────

interface BackendEntry {
  backend: AgentBackend
  unsubscribe: () => void
}

/**
 * 计算单个 session 的 urgency ∈ [0, 1]。
 *
 *   - 非 waiting 状态：urgency = 0
 *   - waiting：timeUrgency = clamp01(ageSec / 120)，再与 RISK_BOOST[risk] 取 max
 *
 * 与 theme.ts 内部使用的语义保持一致；这里独立实现避免循环依赖。
 */
function computeUrgency(snapshot: SessionSnapshot, now: number): number {
  if (snapshot.status !== 'waiting') return 0
  const waitingSince = snapshot.waitingSince
  if (waitingSince === undefined) return 0
  const ageSec = (now - waitingSince) / 1000
  const timeUrgency = Math.min(Math.max(ageSec / 120, 0), 1)
  const riskBoost = snapshot.risk ? (RISK_BOOST[snapshot.risk] ?? 0) : 0
  return Math.max(timeUrgency, riskBoost)
}

/** session 在 sessions Map 中的复合 key */
function sessionKey(backend: BackendId, sessionId: string): string {
  return `${backend}:${sessionId}`
}

// ─────────────────────────────────────────────────────────────────────────────
// SessionBoard
// ─────────────────────────────────────────────────────────────────────────────

export class SessionBoard {
  private readonly slotCount: number
  private readonly palette: ThemePalette
  private readonly now: () => number

  /** 所有 backend 推上来的 session 快照，key = `${backend}:${sessionId}` */
  private readonly sessions = new Map<string, SessionSnapshot>()

  /** 已注册的 backend，key = backend.id */
  private readonly backends = new Map<BackendId, BackendEntry>()

  /** 当前焦点槽 */
  private focus = 0

  /** 钉死的槽位 → sessionId（手动绑定，透传给 slotAllocator） */
  private readonly pins = new Map<number, string>()

  /** 策略模式（V1 不动态切换，默认 act） */
  private readonly mode: PolicyMode = 'act'

  /** led 帧订阅者 */
  private readonly ledHandlers = new Set<(frame: LedFrame) => void>()

  /** board state 订阅者 */
  private readonly stateHandlers = new Set<(state: BoardState) => void>()

  /** 防止 dispose 后再广播 */
  private disposed = false

  /** 最后一次输出的 led 帧（供新连接 client 立即取） */
  private lastLedFrame: LedFrame | null = null

  /** 最后一次输出的 board state（供新连接 client 立即取） */
  private lastBoardState: BoardState | null = null

  constructor(opts?: SessionBoardOptions) {
    this.slotCount = opts?.slotCount ?? SLOT_COUNT
    this.palette = opts?.palette ?? CODEX_THEME
    this.now = opts?.now ?? (() => Date.now())
  }

  // ─── backend 注册 ────────────────────────────────────────────────────────

  /** 注册一个 backend，开始订阅它的快照。 */
  addBackend(backend: AgentBackend): void {
    const id = backend.id
    // 已注册过：先解除旧订阅，保持幂等
    const existing = this.backends.get(id)
    if (existing) {
      existing.unsubscribe()
    }

    const unsubscribe = backend.observe((snapshots) => {
      this.handleSnapshots(id, snapshots)
    })

    this.backends.set(id, { backend, unsubscribe })
  }

  /** 移除 backend，清掉它的所有 session。 */
  removeBackend(id: BackendId): void {
    const entry = this.backends.get(id)
    if (!entry) return
    try {
      entry.unsubscribe()
    } catch (err) {
      // backend unsubscribe 不应抛，但兜底防止影响主流程
      this.reportError(err)
    }
    this.backends.delete(id)

    // 删除该 backend 的所有 session
    for (const key of Array.from(this.sessions.keys())) {
      if (key.startsWith(`${id}:`)) {
        this.sessions.delete(key)
      }
    }

    // 注：指向已消失 session 的 pin 不在此处清理 —— slotAllocator 在 session
    // 不存在时会把 pinned 槽留空但仍占位，正是 pin 的预期语义（防被抢占）。
    this.recompute()
  }

  /** backend observe 回调：用新列表覆盖该 backend 的所有 session。 */
  private handleSnapshots(backend: BackendId, snapshots: SessionSnapshot[]): void {
    if (this.disposed) return

    // 1. 删除该 backend 下、不在新列表里的 session
    const incoming = new Set<string>()
    for (const s of snapshots) {
      incoming.add(sessionKey(backend, s.sessionId))
    }
    for (const key of Array.from(this.sessions.keys())) {
      if (key.startsWith(`${backend}:`) && !incoming.has(key)) {
        this.sessions.delete(key)
      }
    }

    // 2. 更新 / 插入新列表中的 session
    for (const s of snapshots) {
      this.sessions.set(sessionKey(backend, s.sessionId), s)
    }

    // 3. 触发重新计算
    this.recompute()
  }

  // ─── 用户操作 ──────────────────────────────────────────────────────────────

  /** 设置当前焦点槽。 */
  setFocus(i: number): void {
    if (i < 0 || i >= this.slotCount || !Number.isInteger(i)) return
    if (this.focus === i) return
    this.focus = i
    this.recompute()
  }

  /** 设置 / 取消钉死。sessionId 为 null 表示取消该槽位的 pin。 */
  pin(i: number, sessionId: string | null): void {
    if (i < 0 || i >= this.slotCount || !Number.isInteger(i)) return
    if (sessionId === null) {
      if (this.pins.delete(i)) {
        this.recompute()
      }
      return
    }
    const prev = this.pins.get(i)
    if (prev === sessionId) return
    this.pins.set(i, sessionId)
    this.recompute()
  }

  // ─── 重新计算 ────────────────────────────────────────────────────────────

  /** 强制重新计算并广播（外部触发或内部状态变更后调用）。 */
  recompute(): void {
    if (this.disposed) return

    const now = this.now()

    // 1. 扫一遍：过期的 done 直接从 sessions 删除
    this.purgeExpiredDone(now)

    // 2. 构造 ScoredSession[]
    const scored: ScoredSession[] = []
    for (const s of this.sessions.values()) {
      scored.push({ ...s, urgency: computeUrgency(s, now) })
    }

    // 3. 分配槽位
    const allocated = allocateSlots(scored, {
      slotCount: this.slotCount,
      focus: this.focus,
      pins: this.pins,
    })

    // 4. 组装 LedFrame + BoardState
    const ledFrame = this.buildLedFrame(allocated)
    const boardState = this.buildBoardState(allocated)

    // 5. 广播给订阅者
    this.broadcast(ledFrame, boardState)
  }

  /** 删除 status==='done' 且 now - updatedAt > DONE_TTL_MS 的 session。 */
  private purgeExpiredDone(now: number): void {
    for (const [key, s] of this.sessions) {
      if (s.status === 'done' && now - s.updatedAt > DONE_TTL_MS) {
        this.sessions.delete(key)
      }
    }
  }

  /** 由 AllocatedSlot[] 构造 LedFrame。 */
  private buildLedFrame(allocated: AllocatedSlot[]): LedFrame {
    const slots: LedSlot[] = allocated.map((slot) => {
      if (!slot.session) {
        return { i: slot.i, rgb: null, br: 0, fx: 'solid' as const }
      }
      const out = paint(
        {
          status: slot.session.status,
          risk: slot.session.risk,
          waitingSince: slot.session.waitingSince,
          now: this.now(),
        },
        this.palette,
      )
      return { i: slot.i, rgb: out.rgb, br: out.br, fx: out.fx }
    })
    return { type: 'leds', slots }
  }

  /** 由 AllocatedSlot[] 构造 BoardState。 */
  private buildBoardState(allocated: AllocatedSlot[]): BoardState {
    const slots: SlotBinding[] = allocated.map((slot) => ({
      i: slot.i,
      backend: slot.session?.backend,
      sessionId: slot.session?.sessionId,
      title: slot.session?.title,
      status: slot.session?.status ?? 'off',
      detail: slot.session?.detail,
      focused: slot.i === this.focus,
    }))
    return {
      type: 'board',
      slots,
      focus: this.focus,
      mode: this.mode,
    }
  }

  /** 广播 led 帧 + board state 给本地订阅者，并 emit 到 bus。 */
  private broadcast(ledFrame: LedFrame, boardState: BoardState): void {
    this.lastLedFrame = ledFrame
    this.lastBoardState = boardState
    for (const cb of this.ledHandlers) {
      try {
        cb(ledFrame)
      } catch (err) {
        this.reportError(err)
      }
    }
    for (const cb of this.stateHandlers) {
      try {
        cb(boardState)
      } catch (err) {
        this.reportError(err)
      }
    }
    try {
      bus.emit('board.changed', ledFrame)
    } catch (err) {
      this.reportError(err)
    }
  }

  // ─── 订阅 ──────────────────────────────────────────────────────────────────

  /** 订阅 led 帧（给 device bridge 用）。返回 unsubscribe。 */
  onLedFrame(cb: (frame: LedFrame) => void): () => void {
    this.ledHandlers.add(cb)
    return () => {
      this.ledHandlers.delete(cb)
    }
  }

  /** 订阅 board state（给 simulator gateway 用）。返回 unsubscribe。 */
  onBoardState(cb: (state: BoardState) => void): () => void {
    this.stateHandlers.add(cb)
    return () => {
      this.stateHandlers.delete(cb)
    }
  }

  // ─── Getter（外部读取当前状态，main.ts / ActionRouter 用） ──────────────────

  /** 当前焦点槽 */
  getFocus(): number {
    return this.focus
  }

  /** 最后一次推的 LedFrame（可能为 null） */
  getLedFrame(): LedFrame | null {
    return this.lastLedFrame
  }

  /** 最后一次推的 BoardState（可能为 null） */
  getBoardState(): BoardState | null {
    return this.lastBoardState
  }

  /** 当前策略模式 */
  getMode(): PolicyMode {
    return this.mode
  }

  // ─── 销毁 ──────────────────────────────────────────────────────────────────

  /** 销毁：清理所有订阅、清空 handlers。 */
  dispose(): void {
    if (this.disposed) return
    this.disposed = true

    for (const entry of this.backends.values()) {
      try {
        entry.unsubscribe()
      } catch (err) {
        this.reportError(err)
      }
    }
    this.backends.clear()
    this.sessions.clear()
    this.pins.clear()
    this.ledHandlers.clear()
    this.stateHandlers.clear()
  }

  // ─── 内部工具 ────────────────────────────────────────────────────────────────

  /** 统一错误出口（避免 console.log，仅记录真正的异常）。 */
  private reportError(err: unknown): void {
    // 仅在真正异常时输出；正常控制流不经过这里
    console.error('[SessionBoard] handler threw:', err)
  }
}
