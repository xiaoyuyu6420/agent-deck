/**
 * WebSocket 客户端封装 —— 自动重连、JSON 解析
 */

import WebSocket from 'ws'
import type { ServerMessage } from '@agent-deck/protocol'

export type ConnStatus = 'connecting' | 'connected' | 'disconnected'

export interface WsClientOptions {
  url?: string
  onMessage: (msg: ServerMessage) => void
  onStatus?: (status: ConnStatus) => void
}

export class WsClient {
  private ws: WebSocket | null = null
  private reconnectTimer: NodeJS.Timeout | null = null
  private disposed = false
  private url: string

  constructor(private opts: WsClientOptions) {
    this.url = opts.url ?? 'ws://127.0.0.1:8787'
  }

  start(): void {
    this.connect()
  }

  private connect(): void {
    if (this.disposed) return
    this.opts.onStatus?.('connecting')
    let ws: WebSocket
    try {
      ws = new WebSocket(this.url)
    } catch (err) {
      this.scheduleReconnect()
      return
    }
    this.ws = ws

    ws.on('open', () => {
      if (this.disposed) return
      this.opts.onStatus?.('connected')
    })

    ws.on('message', (data) => {
      try {
        const text =
          typeof data === 'string'
            ? data
            : Buffer.isBuffer(data)
              ? data.toString('utf-8')
              : Array.isArray(data)
                ? Buffer.concat(data).toString('utf-8')
                : String(data)
        const msg = JSON.parse(text) as ServerMessage
        this.opts.onMessage(msg)
      } catch {
        // ignore parse error
      }
    })

    const handleClose = () => {
      if (this.disposed) return
      this.opts.onStatus?.('disconnected')
      this.scheduleReconnect()
    }
    ws.on('close', handleClose)
    ws.on('error', () => {
      // error 之后会触发 close，由 close 处理重连
    })
  }

  private scheduleReconnect(): void {
    if (this.disposed) return
    if (this.reconnectTimer) return
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      this.connect()
    }, 1500)
    this.reconnectTimer.unref?.()
  }

  send(msg: unknown): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg))
    }
  }

  dispose(): void {
    this.disposed = true
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    if (this.ws) {
      try {
        this.ws.close()
      } catch {
        /* ignore */
      }
      this.ws = null
    }
  }
}
