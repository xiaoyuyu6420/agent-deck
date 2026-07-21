/**
 * ZCode DB 行 → SessionSnapshot 的映射（纯函数）
 */

import type { DeckStatus, Risk, SessionSnapshot } from '@agent-deck/protocol'

/** tasks + tool_usage 跨库 join 后的原始行 */
export interface ZcodeRow {
  task_id: string
  title: string | null
  task_status: string | null // 'running' | 'completed' | 'error' | NULL
  workspace_path: string | null
  updated_at: number // ms epoch
  waiting: 0 | 1 // SQL CASE 输出
  detail: string | null // "Bash: shell" 形式
}

/**
 * 映射规则：
 * - task_status='error' → 'error'
 * - task_status='completed' → 'done'
 * - task_status='running' && waiting=1 → 'waiting'
 * - task_status='running' && waiting=0 → 'working'
 * - 其他（含 NULL）→ 'idle'
 *
 * waitingSince: 仅 waiting 状态时为 updated_at（近似），否则 undefined
 * risk: 仅 waiting 状态时由 detail 推断
 */
export function mapZcodeRow(row: ZcodeRow): SessionSnapshot {
  return mapZcodeRowAt(row, Date.now())
}

/** 纯函数版本，测试可注入 now */
export function mapZcodeRowAt(row: ZcodeRow, _now: number): SessionSnapshot {
  const status = mapStatus(row.task_status, row.waiting === 1)
  const risk = status === 'waiting' ? inferRisk(row.detail) : undefined
  return {
    backend: 'zcode',
    sessionId: row.task_id,
    title: row.title ?? '(untitled)',
    status,
    risk,
    detail: row.detail ?? undefined,
    waitingSince: status === 'waiting' ? row.updated_at : undefined,
    updatedAt: row.updated_at,
    workspacePath: row.workspace_path ?? undefined,
  }
}

function mapStatus(taskStatus: string | null, waiting: boolean): DeckStatus {
  if (taskStatus === 'error') return 'error'
  if (taskStatus === 'completed') return 'done'
  if (taskStatus === 'running') return waiting ? 'waiting' : 'working'
  return 'idle'
}

/**
 * 从 detail 字段推断风险等级（大小写不敏感）
 *
 * - shell / Bash / git push / git reset / rm / delete / destroy → high
 * - fileWrite / fileEdit / Edit / Write / file → medium
 * - userInteraction / AskUser / read / Grep / Glob → low
 * - 其他 / null → medium（保守）
 */
export function inferRisk(detail: string | null): Risk {
  if (!detail) return 'medium'
  const d = detail.toLowerCase()

  // high
  const highKeywords = [
    'shell',
    'bash',
    'git push',
    'git reset',
    'rm ',
    'rm-',
    'delete',
    'destroy',
    'force',
    'sudo',
    'chmod',
    'mv ',
    'unlink',
  ]
  for (const kw of highKeywords) {
    if (d.includes(kw)) return 'high'
  }

  // low
  const lowKeywords = [
    'userinteraction',
    'askuser',
    'read',
    'grep',
    'glob',
    'list',
    'todo',
    'view',
    'ls ',
  ]
  for (const kw of lowKeywords) {
    if (d.includes(kw)) return 'low'
  }

  // medium
  const mediumKeywords = [
    'filewrite',
    'fileedit',
    'edit',
    'write',
    'file',
    'patch',
    'create',
    'mkdir',
  ]
  for (const kw of mediumKeywords) {
    if (d.includes(kw)) return 'medium'
  }

  return 'medium'
}
