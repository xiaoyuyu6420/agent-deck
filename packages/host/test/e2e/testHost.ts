/**
 * TestHost —— 在测试里搭一份"迷你 main.ts"，所有组件都接真实现。
 *
 * 与 main.ts 的区别：
 *   - now 通过 mockClock 注入（urgency 渐变可以受控）
 *   - ZcodeAdapter 指向 fixture DB，pollIntervalMs 调到 30ms
 *   - 不起 SIGINT/SIGTERM 监听
 *   - 端口动态分配，避免冲突
 */

import { SessionBoard } from '../../src/board/SessionBoard.js'
import { ZcodeAdapter } from '../../src/backends/zcode/ZcodeAdapter.js'
import { Gateway } from '../../src/gateway/server.js'
import { SimulatorBridge } from '../../src/device/SimulatorBridge.js'
import { ActionRouter } from '../../src/actions/ActionRouter.js'
import type { ServerMessage, ClientMessage } from '@agent-deck/protocol'

export interface TestHostOptions {
  tasksDbPath: string
  toolDbPath: string
  /** Gateway 端口，必填（调用方负责分配空闲端口） */
  port: number
  /** mockClock，用于 urgency 推进；默认 () => Date.now() */
  now?: () => number
  /** 自指防护 */
  excludeWorkspaces?: string[]
  excludeTaskIds?: string[]
}

export interface TestHost {
  board: SessionBoard
  zcode: ZcodeAdapter
  gateway: Gateway
  router: ActionRouter
  bridge: SimulatorBridge
  backends: Map<string, { shutdown(): Promise<void> }>
  /** 收到 client message 的处理入口（来自 Gateway.onMessage） */
  onClientMessage: (msg: ClientMessage) => Promise<void>
  /** 关闭一切，调用方在 afterEach 调用 */
  shutdown(): Promise<void>
}

export async function startTestHost(opts: TestHostOptions): Promise<TestHost> {
  const board = new SessionBoard({ now: opts.now })

  const zcode = new ZcodeAdapter({
    tasksDbPath: opts.tasksDbPath,
    toolDbPath: opts.toolDbPath,
    pollIntervalMs: 30,
    excludeWorkspaces: opts.excludeWorkspaces,
    excludeTaskIds: opts.excludeTaskIds,
    failOnMissing: true,
    log: () => {
      /* silent in tests */
    },
  })

  const backends = new Map<string, { shutdown(): Promise<void> }>()
  backends.set('zcode', zcode)

  board.addBackend(zcode)
  await zcode.start()

  const router = new ActionRouter({
    board,
    getSlotBinding: (i) => {
      const state = board.getBoardState()
      if (!state) return undefined
      return state.slots.find((s) => s.i === i)
    },
    getBackend: (id) =>
      backends.get(id) as unknown as
        | ReturnType<typeof backends.get>
        | undefined,
    log: () => {
      /* silent */
    },
  })

  const bridgeRef: { current: SimulatorBridge | null } = { current: null }

  const gateway = new Gateway(
    { port: opts.port, host: '127.0.0.1' },
    {
      onConnect: (ws) => {
        const state = board.getBoardState()
        const frame = board.getLedFrame()
        if (state) ws.send(JSON.stringify(state satisfies ServerMessage))
        if (frame) ws.send(JSON.stringify(frame satisfies ServerMessage))
      },
      onMessage: async (msg) => {
        const b = bridgeRef.current
        if (!b) return
        try {
          await b.handleClientMessage(msg satisfies ClientMessage)
        } catch {
          /* swallow */
        }
      },
    },
  )
  await gateway.start()

  const bridge = new SimulatorBridge({ board, router, gateway })
  bridge.start()
  bridgeRef.current = bridge

  return {
    board,
    zcode,
    gateway,
    router,
    bridge,
    backends,
    onClientMessage: (msg) => bridge.handleClientMessage(msg),
    shutdown: async () => {
      bridge.stop()
      await gateway.stop()
      for (const b of backends.values()) {
        await b.shutdown()
      }
      board.dispose()
    },
  }
}

/**
 * 拿一个系统分配的空闲端口。
 * 通过 bind port 0 让 OS 给端口，然后立刻 close 让其可用。
 */
export async function allocatePort(): Promise<number> {
  const { createServer } = await import('node:http')
  return new Promise((resolve, reject) => {
    const srv = createServer()
    srv.on('error', reject)
    srv.listen(0, '127.0.0.1', () => {
      const addr = srv.address()
      if (addr && typeof addr === 'object') {
        const port = addr.port
        srv.close(() => resolve(port))
      } else {
        srv.close()
        reject(new Error('failed to allocate port'))
      }
    })
  })
}
