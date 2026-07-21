/**
 * CodexAdapter —— Codex backend 的 V1 骨架。
 *
 * V1 不强制实现真正连接（codex app-server 接入在 Phase 1）。
 * 当前为安全的 stub：start 时记录一行日志，observe 立即推送空快照列表。
 */

import type { AgentBackend } from '../types.js'
import type { SessionSnapshot } from '@agent-deck/protocol'

export interface CodexAdapterOptions {
  /** codex 二进制路径，默认 'codex'（PATH 查找） */
  codexBin?: string
  /** 日志 */
  log?: (msg: string, ...args: unknown[]) => void
}

// TODO Phase 1: 连接 codex app-server / remote-control
export class CodexAdapter implements AgentBackend {
  readonly id = 'codex' as const

  private readonly codexBin: string
  private readonly log: (msg: string, ...args: unknown[]) => void
  private subscribers = new Set<(snapshots: SessionSnapshot[]) => void>()
  private started = false

  constructor(opts?: CodexAdapterOptions) {
    this.codexBin = opts?.codexBin ?? 'codex'
    this.log = opts?.log ?? (() => {})
  }

  async start(): Promise<void> {
    if (this.started) return
    this.log('[codex] V1 stub - 实际接入在 Phase 1')
    this.started = true
  }

  async shutdown(): Promise<void> {
    if (!this.started) return
    this.subscribers.clear()
    this.started = false
  }

  observe(cb: (snapshots: SessionSnapshot[]) => void): () => void {
    this.subscribers.add(cb)
    // V1 stub：立即推送一次空快照列表，不订阅任何事件
    cb([])
    return () => {
      this.subscribers.delete(cb)
    }
  }

  // 动作方法（accept/reject/stop/send）在 V1.1 才实现，此处故意不定义，
  // 保持接口可选方法为 undefined。
}
