/**
 * ZcodeAdapter —— 把 ZcodeSqliteObserver 包装成统一的 AgentBackend。
 *
 * 观察层：在 start() 时创建 observer 并注册 onChange 回调，
 *         把 observer 推送的快照扇出（fan-out）给所有 subscribers。
 * 动作层：V1.1 才接入，此处 accept/reject/stop/send 保持 undefined，
 *         让 ActionRouter 走"unsupported 降级"分支。
 *
 * 生命周期：每次 start() 创建全新的 observer，避免重用导致的监听器累积；
 *           stop() 调 observer.stop() 释放资源并清空 subscribers。
 */

import type { AgentBackend } from '../types.js'
import { ZcodeSqliteObserver, type SqliteObserverOptions } from './SqliteObserver.js'
import type { SessionSnapshot } from '@agent-deck/protocol'

export interface ZcodeAdapterOptions extends SqliteObserverOptions {
  /** 标识符，默认 'zcode' */
}

export class ZcodeAdapter implements AgentBackend {
  readonly id = 'zcode' as const

  private readonly opts: ZcodeAdapterOptions
  private observer: ZcodeSqliteObserver | null = null
  private subscribers = new Set<(snapshots: SessionSnapshot[]) => void>()
  private started = false

  constructor(opts?: ZcodeAdapterOptions) {
    this.opts = opts ?? {}
  }

  async start(): Promise<void> {
    if (this.started) return

    // 每次 start 创建全新 observer，避免重用导致 onChange 监听器累积
    const observer = new ZcodeSqliteObserver(this.opts)
    this.observer = observer

    // 注册 onChange：observer 推送快照后，广播给所有 subscribers
    observer.onChange((snapshots) => {
      for (const cb of this.subscribers) {
        try {
          cb(snapshots)
        } catch {
          // 单个 subscriber 抛错不应影响其他订阅者
        }
      }
    })

    // observer.start() 是同步的，内部立即拉取首帧并触发上面的广播
    observer.start()
    this.started = true
  }

  async shutdown(): Promise<void> {
    if (!this.started) return
    this.observer?.stop()
    this.observer = null
    this.subscribers.clear()
    this.started = false
  }

  observe(cb: (snapshots: SessionSnapshot[]) => void): () => void {
    this.subscribers.add(cb)
    return () => {
      this.subscribers.delete(cb)
    }
  }

  // 动作方法（accept/reject/stop/send）在 V1.1 才实现，此处故意不定义，
  // 保持接口可选方法为 undefined。
}
