/**
 * EventBus —— 进程内 pub/sub，所有模块通过它解耦。
 *
 * 使用：
 *   bus.on('board.changed', cb)
 *   bus.emit('board.changed', frame)
 */

export interface EventMap {
  'session.updated': { backend: string; snapshot: unknown }
  'board.changed': unknown
  'backend.health': unknown
  'action.requested': unknown
  'action.done': unknown
  'action.failed': { op: string; error: Error }
  'device.connected': { kind: 'simulator' | 'usb' }
  'device.disconnected': { kind: 'simulator' | 'usb' }
  'voice.partial': { text: string }
  'voice.final': { text: string }
}

type Handler<T> = (payload: T) => void

export class EventBus {
  private handlers = new Map<string, Set<Handler<unknown>>>()

  on<K extends keyof EventMap>(event: K, handler: Handler<EventMap[K]>): () => void {
    const key = event as string
    let set = this.handlers.get(key)
    if (!set) {
      set = new Set()
      this.handlers.set(key, set)
    }
    set.add(handler as Handler<unknown>)
    return () => {
      set?.delete(handler as Handler<unknown>)
    }
  }

  emit<K extends keyof EventMap>(event: K, payload: EventMap[K]): void {
    const set = this.handlers.get(event as string)
    if (!set) return
    for (const handler of set) {
      try {
        ;(handler as Handler<EventMap[K]>)(payload)
      } catch (err) {
        // handler 抛错不影响其他 handler
        console.error(`[bus] handler for "${String(event)}" threw:`, err)
      }
    }
  }

  /** 移除所有 handler（测试用） */
  clear(): void {
    this.handlers.clear()
  }
}

/** 单例 */
export const bus = new EventBus()
