/**
 * WS harness —— 用真 ws 客户端连 Gateway，收集收到的所有 ServerMessage。
 *
 * 用法：
 *   const harness = await createWsHarness(port)
 *   await harness.waitFor((msgs) => msgs.some(m => m.type === 'leds'))
 *   harness.close()
 */

import WebSocket, { type WebSocket as WsType } from 'ws'
import type { ServerMessage } from '@agent-deck/protocol'

export interface WsHarness {
  ws: WsType
  messages: ServerMessage[]
  /** 关闭并等待 close 事件 */
  close(): Promise<void>
  /**
   * 等待条件满足。predicate 收到当前累计 messages，返回 true 即完成。
   * 默认 timeout 2000ms，满足返回 messages 快照，超时抛错（含已有消息）。
   */
  waitFor(
    predicate: (msgs: ServerMessage[]) => boolean,
    opts?: { timeoutMs?: number; label?: string },
  ): Promise<ServerMessage[]>
}

export function createWsHarness(
  port: number,
  opts: { timeoutMs?: number } = {},
): Promise<WsHarness> {
  const timeoutMs = opts.timeoutMs ?? 3000
  return new Promise((resolve, reject) => {
    const url = `ws://127.0.0.1:${port}`
    let ws: WsType
    try {
      ws = new WebSocket(url)
    } catch (err) {
      reject(err)
      return
    }
    const messages: ServerMessage[] = []
    const timer = setTimeout(() => {
      try {
        ws.close()
      } catch {
        /* ignore */
      }
      reject(new Error(`ws connect to ${url} timed out after ${timeoutMs}ms`))
    }, timeoutMs)

    ws.on('open', () => {
      clearTimeout(timer)
      resolve({
        ws,
        messages,
        close: () =>
          new Promise<void>((res) => {
            if (ws.readyState === WebSocket.CLOSED) {
              res()
              return
            }
            ws.once('close', () => res())
            try {
              ws.close()
            } catch {
              res()
            }
          }),
        waitFor: (predicate, waitOpts = {}) =>
          waitForImpl(messages, predicate, waitOpts),
      })
    })

    ws.on('message', (raw) => {
      try {
        const text =
          typeof raw === 'string'
            ? raw
            : Buffer.isBuffer(raw)
              ? raw.toString('utf-8')
              : Array.isArray(raw)
                ? Buffer.concat(raw as readonly Buffer[]).toString('utf-8')
                : String(raw)
        messages.push(JSON.parse(text) as ServerMessage)
      } catch {
        /* ignore parse error */
      }
    })

    ws.on('error', (err) => {
      clearTimeout(timer)
      reject(err)
    })
  })
}

function waitForImpl(
  messages: ServerMessage[],
  predicate: (msgs: ServerMessage[]) => boolean,
  opts: { timeoutMs?: number; label?: string } = {},
): Promise<ServerMessage[]> {
  const timeoutMs = opts.timeoutMs ?? 2000
  return new Promise((resolve, reject) => {
    // 先快路径检查一次
    if (predicate(messages)) {
      resolve([...messages])
      return
    }
    const timer = setTimeout(() => {
      clearInterval(poller)
      reject(
        new Error(
          `waitFor${opts.label ? ` (${opts.label})` : ''} timed out after ${timeoutMs}ms; ` +
            `received ${messages.length} messages: ${JSON.stringify(messages.map((m) => m.type))}`,
        ),
      )
    }, timeoutMs)
    // 100ms 轮询：简单可靠，e2e 不追求极致效率
    const poller = setInterval(() => {
      if (predicate(messages)) {
        clearTimeout(timer)
        clearInterval(poller)
        resolve([...messages])
      }
    }, 50)
  })
}

/**
 * 找到最新一条指定类型的 ServerMessage。
 * 用于断言"最近的灯帧"。
 */
export function lastOfType<T extends ServerMessage>(
  messages: ServerMessage[],
  type: T['type'],
): T | undefined {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i]!.type === type) {
      return messages[i] as T
    }
  }
  return undefined
}
