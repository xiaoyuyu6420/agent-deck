/**
 * E2E fixture 工厂 —— 在临时目录里造两个 sqlite，schema 与本机 ZCode 真实表对齐。
 *
 * 复制自 ~/.zcode/v2/tasks-index.sqlite 和 ~/.zcode/cli/db/db.sqlite 的真实 schema
 *（2026-07-21 验证），确保 ZcodeSqliteObserver 的 QUERY_SQL 能直接跑过。
 *
 * 关键约束：
 *   - 写连接用普通模式（不是 readonly），fixture 是测试自己的 DB
 *   - 所有时间戳用 number（ms epoch），与 ZCode 真实存储一致
 *   - 函数返回的 DB 必须在测试结束时调用 close()
 */

import Database from 'better-sqlite3'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

export interface FixtureDb {
  /** tasks-index.sqlite 路径，传给 ZcodeSqliteObserver.tasksDbPath */
  tasksDbPath: string
  /** db.sqlite 路径，传给 ZcodeSqliteObserver.toolDbPath */
  toolDbPath: string
  /** tasks 表的写连接 */
  tasks: Database.Database
  /** tool_usage 表的写连接 */
  tool: Database.Database
  /** 临时目录，teardown 时 rmSync */
  dir: string
  /** 释放：关闭连接 + 删临时目录 */
  cleanup: () => void
}

/**
 * 建一份全新 ZCode 双 DB fixture。
 * schema 完全照搬本机真实表，保证 QUERY_SQL 不被卡住。
 */
export function createZcodeFixture(): FixtureDb {
  const dir = mkdtempSync(join(tmpdir(), 'agent-deck-e2e-'))
  const tasksDbPath = join(dir, 'tasks-index.sqlite')
  const toolDbPath = join(dir, 'db.sqlite')

  const tasks = new Database(tasksDbPath)
  tasks.exec(`
    CREATE TABLE tasks (
      workspace_key TEXT NOT NULL,
      workspace_path TEXT NOT NULL,
      workspace_identity TEXT,
      task_id TEXT NOT NULL,
      title TEXT NOT NULL DEFAULT '',
      task_status TEXT,
      provider TEXT,
      mode TEXT NOT NULL DEFAULT 'build',
      model TEXT,
      migration_source TEXT,
      forked_from_task_id TEXT,
      created_at INTEGER NOT NULL,
      updated_at INTEGER NOT NULL,
      unread_at INTEGER,
      pinned INTEGER NOT NULL DEFAULT 0,
      archived INTEGER NOT NULL DEFAULT 0,
      deleted INTEGER NOT NULL DEFAULT 0,
      title_overridden INTEGER NOT NULL DEFAULT 0,
      meta_json TEXT NOT NULL DEFAULT '{}',
      searchable_text TEXT NOT NULL DEFAULT '',
      PRIMARY KEY (workspace_key, task_id)
    );
  `)

  const tool = new Database(toolDbPath)
  tool.exec(`
    CREATE TABLE tool_usage (
      id text primary key,
      session_id text not null,
      turn_id text,
      trace_id text,
      tool_call_id text not null,
      tool_name text not null,
      side_effect_scope text,
      read_only integer,
      destructive integer,
      approval_status text,
      status text not null check(status in ('running', 'completed', 'error', 'cancelled')),
      started_at integer not null,
      first_output_at integer,
      completed_at integer,
      duration_ms integer,
      time_to_first_output_ms integer,
      exit_code integer,
      output_bytes integer not null default 0,
      stdout_bytes integer not null default 0,
      stderr_bytes integer not null default 0,
      truncated integer not null default 0 check(truncated in (0, 1)),
      retry_count integer not null default 0,
      retryable integer not null default 0 check(retryable in (0, 1)),
      cancelled_by_user integer not null default 0 check(cancelled_by_user in (0, 1)),
      error_type text,
      error_code text,
      error_message text
    );
  `)

  const stmtTasks = tasks.prepare(
    `INSERT INTO tasks (
       workspace_key, workspace_path, task_id, title, task_status,
       created_at, updated_at, deleted, archived
     ) VALUES (?, ?, ?, ?, ?, ?, ?, 0, 0)`,
  )
  const stmtTool = tool.prepare(
    `INSERT INTO tool_usage (
       id, session_id, tool_call_id, tool_name, side_effect_scope,
       approval_status, status, started_at, completed_at
     ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  )

  // 把这两个 stmt 挂在 tasks 上便于上层调用，类型上做最小断言
  ;(tasks as unknown as { _insertTask: unknown })._insertTask = stmtTasks
  ;(tool as unknown as { _insertTool: unknown })._insertTool = stmtTool

  return {
    tasksDbPath,
    toolDbPath,
    tasks,
    tool,
    dir,
    cleanup: () => {
      try {
        tasks.close()
      } catch {
        /* ignore */
      }
      try {
        tool.close()
      } catch {
        /* ignore */
      }
      rmSync(dir, { recursive: true, force: true })
    },
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// 行写入器
// ─────────────────────────────────────────────────────────────────────────────

export interface TaskRowInput {
  task_id: string
  title?: string
  task_status: 'running' | 'completed' | 'error' | null
  workspace_path?: string
  workspace_key?: string
  /** ms epoch，默认 Date.now() */
  updated_at?: number
  created_at?: number
}

export interface ToolRowInput {
  id: string
  session_id: string
  tool_name: string
  side_effect_scope?: string | null
  approval_status?: 'none' | 'requested' | 'approved' | 'rejected' | null
  status?: 'running' | 'completed' | 'error' | 'cancelled'
  started_at?: number
  completed_at?: number | null
}

export function insertTask(fixture: FixtureDb, row: TaskRowInput): void {
  const stmt = (fixture.tasks as unknown as { _insertTask: Database.Statement })
    ._insertTask
  stmt.run(
    row.workspace_key ?? `ws:${row.task_id}`,
    row.workspace_path ?? '/tmp/test-workspace',
    row.task_id,
    row.title ?? 'test task',
    row.task_status,
    row.created_at ?? Date.now(),
    row.updated_at ?? Date.now(),
  )
}

export function insertTool(fixture: FixtureDb, row: ToolRowInput): void {
  const stmt = (fixture.tool as unknown as { _insertTool: Database.Statement })
    ._insertTool
  stmt.run(
    row.id,
    row.session_id,
    `call_${row.id}`, // tool_call_id
    row.tool_name,
    row.side_effect_scope ?? null,
    row.approval_status ?? 'none',
    row.status ?? 'completed',
    row.started_at ?? Date.now(),
    row.completed_at === undefined ? null : row.completed_at,
  )
}

/** 更新 task_status / updated_at，用于模拟"任务完成" */
export function updateTaskStatus(
  fixture: FixtureDb,
  taskId: string,
  status: 'running' | 'completed' | 'error',
  updatedAt = Date.now(),
): void {
  fixture.tasks
    .prepare(
      `UPDATE tasks SET task_status = ?, updated_at = ? WHERE task_id = ?`,
    )
    .run(status, updatedAt, taskId)
}

/** 把 tool_usage 标为 completed（模拟用户点 Accept 后的状态） */
export function completeTool(fixture: FixtureDb, toolId: string): void {
  fixture.tool
    .prepare(
      `UPDATE tool_usage SET status = 'completed', completed_at = ?, approval_status = 'approved' WHERE id = ?`,
    )
    .run(Date.now(), toolId)
}
