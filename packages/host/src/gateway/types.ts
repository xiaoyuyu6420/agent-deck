/**
 * Gateway 共享类型。
 *
 * 把 protocol 的 wire 类型重新导出，避免每个引用方都直接 import protocol。
 */

import type { ServerMessage, ClientMessage } from '@agent-deck/protocol'
import type { WebSocket } from 'ws'

export type { ServerMessage, ClientMessage }

export interface GatewayOptions {
  /** 监听端口 */
  port: number
  /** 绑定地址，默认 127.0.0.1（出于安全不暴露到局域网） */
  host?: string
}

export interface GatewayHandlers {
  /** 收到 client 消息时调用 */
  onMessage: (msg: ClientMessage, ws: WebSocket) => void
  /** 连接建立 */
  onConnect?: (ws: WebSocket) => void
  /** 连接断开 */
  onDisconnect?: (ws: WebSocket) => void
}
