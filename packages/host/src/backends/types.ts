/**
 * Backend 接口契约 —— 所有 adapter（ZCode / Codex）必须实现此接口。
 *
 * V1 仅要求实现 observe（观察）；动作方法（accept/reject/stop）可选，
 * 未实现的 backend 让 ActionRouter 走"unsupported 降级"分支。
 */

import type { SessionSnapshot, BackendId } from '@agent-deck/protocol'

export type { BackendId }

export interface AgentBackend {
  /** 后端标识 */
  readonly id: BackendId

  /** 启动观察（连接 IPC、监听文件、订阅事件等） */
  start(): Promise<void>

  /** 停止观察，释放资源 */
  shutdown(): Promise<void>

  /**
   * 订阅 session 快照变化。
   * 返回 unsubscribe 函数。
   * observer 应在每次状态变化时调用 cb，传入当前所有 session 的完整快照列表。
   */
  observe(cb: (snapshots: SessionSnapshot[]) => void): () => void

  // ─── 动作层（可选，V1.1+ 才实现）───────────────────────────────────────────

  /** 接受某 session 的当前 pending 请求 */
  accept?(sessionId: string): Promise<void>

  /** 拒绝某 session 的当前 pending 请求 */
  reject?(sessionId: string): Promise<void>

  /** 停止某 session 的当前回合 */
  interrupt?(sessionId: string): Promise<void>

  /** 向某 session 发送文本 */
  send?(sessionId: string, text: string): Promise<void>
}

/**
 * Backend 事件（更细粒度的订阅，可选）
 */
export type BackendEvent =
  | { type: 'session'; session: SessionSnapshot }
  | { type: 'removed'; sessionId: string }
  | { type: 'error'; error: Error }

/**
 * Backend 健康状态
 */
export interface BackendHealth {
  id: BackendId
  running: boolean
  lastError?: string
  lastSeenAt?: number
}
