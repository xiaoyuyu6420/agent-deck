/**
 * Gateway —— Agent Deck 的 HTTP + WebSocket 服务入口。
 *
 * - HTTP: `GET /health` 返回简单运行时信息，其他路径 404。
 * - WebSocket: simulator（TUI / 手机 app）连接后订阅灯帧、board 状态。
 *
 * 设计要点：
 *   - 默认绑 `127.0.0.1`，不暴露到局域网。
 *   - 所有 ws / http 错误都被捕获并打日志，不能让进程崩。
 *   - 自动订阅进程内 bus 的 `board.changed`，SessionBoard recompute 后
 *     把最新帧广播给所有已连接 client。
 */

import { WebSocketServer, WebSocket } from 'ws'
import {
  createServer,
  type IncomingMessage,
  type Server,
  type ServerResponse,
} from 'node:http'
import type { ServerMessage, ClientMessage } from './types.js'
import type { GatewayOptions, GatewayHandlers } from './types.js'
import { bus } from '../bus.js'

const DEFAULT_HOST = '127.0.0.1'

const SERVER_MESSAGE_TYPES = new Set(['leds', 'board', 'focus', 'event'])

/**
 * bus payload 是 unknown，这里做最小形状校验，
 * 只有看着像 ServerMessage 的才透传到 wire。
 */
function isServerMessageLike(v: unknown): v is ServerMessage {
  if (typeof v !== 'object' || v === null) return false
  const type = (v as { type?: unknown }).type
  return typeof type === 'string' && SERVER_MESSAGE_TYPES.has(type)
}

export class Gateway {
  private readonly httpServer: Server
  private wss: WebSocketServer | undefined
  private readonly clients = new Set<WebSocket>()
  private readonly handlers: GatewayHandlers
  private readonly port: number
  private readonly host: string
  private readonly startedAt = Date.now()
  private unsubBus?: () => void
  private running = false

  constructor(opts: GatewayOptions, handlers: GatewayHandlers) {
    this.port = opts.port
    this.host = opts.host ?? DEFAULT_HOST
    this.handlers = handlers

    this.httpServer = createServer((req, res) => this.handleHttp(req, res))
  }

  // ───────────────────────────────────────────────────────────────────────────
  // 生命周期
  // ───────────────────────────────────────────────────────────────────────────

  /** 启动 HTTP + WS 服务，监听 port / host。 */
  async start(): Promise<void> {
    if (this.running) return

    // HTTP 错误兜底（端口占用 / 权限 等）
    this.httpServer.on('error', (err) => {
      console.error('[gateway] http server error:', err)
    })

    // WebSocketServer 挂到 http server 上：upgrade 直接交给 ws 处理。
    const wss = new WebSocketServer({ server: this.httpServer })
    this.wss = wss
    wss.on('error', (err) => {
      console.error('[gateway] ws server error:', err)
    })
    wss.on('connection', (ws, req) => {
      this.handleConnection(ws, req)
    })

    // 自动订阅 bus：board 状态变化时广播给所有 client。
    // bus payload 类型为 unknown，先做最小形状校验再当 ServerMessage 处理，
    // 避免把脏数据透传到 wire 上。
    this.unsubBus = bus.on('board.changed', (payload) => {
      if (!isServerMessageLike(payload)) return
      this.broadcast(payload)
    })

    await new Promise<void>((resolve, reject) => {
      const onError = (err: NodeJS.ErrnoException): void => {
        this.httpServer.off('listening', onListening)
        reject(err)
      }
      const onListening = (): void => {
        this.httpServer.off('error', onError)
        resolve()
      }
      this.httpServer.once('error', onError)
      this.httpServer.once('listening', onListening)
      this.httpServer.listen(this.port, this.host)
    })

    // 只有 listen 成功才算 running，避免半启动状态被 stop() 触碰。
    this.running = true
  }

  /** 关闭所有连接并释放端口。 */
  async stop(): Promise<void> {
    if (!this.running) return
    this.running = false

    this.unsubBus?.()
    this.unsubBus = undefined

    // 1) 主动关闭所有 ws 连接
    for (const ws of this.clients) {
      try {
        ws.close()
      } catch (err) {
        console.error('[gateway] error closing ws:', err)
      }
    }

    // 2) 关闭 wss / httpServer，等待真正释放
    const tasks: Promise<void>[] = []

    const wss = this.wss
    if (wss) {
      tasks.push(
        new Promise<void>((resolve) => {
          wss.close((err) => {
            if (err) console.error('[gateway] wss.close error:', err)
            resolve()
          })
        }),
      )
    }

    tasks.push(
      new Promise<void>((resolve) => {
        this.httpServer.close((err) => {
          if (err) console.error('[gateway] httpServer.close error:', err)
          resolve()
        })
      }),
    )

    // 3) 等到所有 client socket 真正关闭（httpServer.close 不会主动踢连接）
    for (const ws of this.clients) {
      tasks.push(
        new Promise<void>((resolve) => {
          if (ws.readyState === WebSocket.CLOSED) {
            resolve()
            return
          }
          ws.once('close', () => resolve())
        }),
      )
    }

    await Promise.all(tasks)
    this.clients.clear()
  }

  // ───────────────────────────────────────────────────────────────────────────
  // 公开 API
  // ───────────────────────────────────────────────────────────────────────────

  /** 广播一条 ServerMessage 给所有 OPEN 状态的 client。 */
  broadcast(msg: ServerMessage): void {
    let json: string
    try {
      json = JSON.stringify(msg)
    } catch (err) {
      console.error('[gateway] broadcast: failed to serialize message:', err)
      return
    }
    for (const ws of this.clients) {
      if (ws.readyState !== WebSocket.OPEN) continue
      try {
        ws.send(json)
      } catch (err) {
        console.error('[gateway] broadcast: ws.send failed:', err)
        this.removeClient(ws)
      }
    }
  }

  /** 当前已连接 client 数。 */
  clientCount(): number {
    return this.clients.size
  }

  // ───────────────────────────────────────────────────────────────────────────
  // HTTP
  // ───────────────────────────────────────────────────────────────────────────

  private handleHttp(req: IncomingMessage, res: ServerResponse): void {
    try {
      const url = req.url ?? ''
      if (req.method === 'GET' && url.split('?')[0] === '/health') {
        const body = {
          ok: true,
          clients: this.clients.size,
          uptime: Math.round((Date.now() - this.startedAt) / 1000),
        }
        res.writeHead(200, { 'content-type': 'application/json' })
        res.end(JSON.stringify(body))
        return
      }
      res.writeHead(404, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ error: 'not found' }))
    } catch (err) {
      console.error('[gateway] http handler error:', err)
      try {
        if (!res.headersSent) res.writeHead(500)
        res.end('internal error')
      } catch {
        /* ignore */
      }
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // WebSocket
  // ───────────────────────────────────────────────────────────────────────────

  private handleConnection(ws: WebSocket, _req: IncomingMessage): void {
    this.clients.add(ws)

    // 任何未捕获的 ws 错误都必须吞掉，避免冒泡到 process 杀掉进程。
    ws.on('error', (err) => {
      console.error('[gateway] client ws error:', err)
      this.removeClient(ws)
    })

    ws.on('close', () => {
      this.removeClient(ws)
      try {
        this.handlers.onDisconnect?.(ws)
      } catch (err) {
        console.error('[gateway] onDisconnect handler threw:', err)
      }
    })

    ws.on('message', (raw) => {
      let msg: ClientMessage
      try {
        // ws v8 RawData 是 Buffer[] | Buffer
        const buf: Buffer = Buffer.isBuffer(raw)
          ? raw
          : Buffer.concat(raw as readonly Buffer[])
        msg = JSON.parse(buf.toString('utf-8')) as ClientMessage
      } catch {
        this.send(ws, { type: 'event', event: 'error', data: 'invalid json' })
        return
      }
      try {
        this.handlers.onMessage(msg, ws)
      } catch (err) {
        console.error('[gateway] onMessage handler threw:', err)
      }
    })

    try {
      this.handlers.onConnect?.(ws)
    } catch (err) {
      console.error('[gateway] onConnect handler threw:', err)
    }
  }

  private removeClient(ws: WebSocket): void {
    this.clients.delete(ws)
  }

  private send(ws: WebSocket, msg: ServerMessage): void {
    if (ws.readyState !== WebSocket.OPEN) return
    try {
      ws.send(JSON.stringify(msg))
    } catch (err) {
      console.error('[gateway] send failed:', err)
      this.removeClient(ws)
    }
  }
}
