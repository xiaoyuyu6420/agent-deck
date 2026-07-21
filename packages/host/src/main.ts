/**
 * agent-deck-host 入口
 *
 * 启动顺序：
 *   1. 加载 config
 *   2. 创建 SessionBoard
 *   3. 创建并启动 backends (zcode + codex)
 *   4. 创建 ActionRouter
 *   5. 创建 Gateway (WS + HTTP)
 *   6. 创建 SimulatorBridge 连接 board → gateway
 *   7. 信号优雅退出
 */

import { loadConfig, type HostConfig } from './config.js'
import { SessionBoard } from './board/SessionBoard.js'
import { ZcodeAdapter } from './backends/zcode/ZcodeAdapter.js'
import { CodexAdapter } from './backends/codex/CodexAdapter.js'
import type { AgentBackend } from './backends/types.js'
import { Gateway } from './gateway/server.js'
import { ActionRouter } from './actions/ActionRouter.js'
import { SimulatorBridge } from './device/SimulatorBridge.js'

async function main(): Promise<void> {
  const config = loadConfig()
  const log = makeLogger(config)

  log(`[agent-deck] starting host`)
  log(
    `[agent-deck] config: port=${config.port} backends=${config.enabledBackends.join(',')} slots=${config.slotCount}`,
  )

  // ─── 1. SessionBoard ──────────────────────────────────────────────────────
  const board = new SessionBoard({ slotCount: config.slotCount })

  // ─── 2. Backends ──────────────────────────────────────────────────────────
  const backends = new Map<string, AgentBackend>()

  if (config.enabledBackends.includes('zcode')) {
    const zcode = new ZcodeAdapter({
      excludeWorkspaces: config.excludeWorkspaces,
      excludeTaskIds: config.excludeTaskIds,
      tasksDbPath: config.zcodeHome
        ? `${config.zcodeHome}/v2/tasks-index.sqlite`
        : undefined,
      toolDbPath: config.zcodeHome
        ? `${config.zcodeHome}/cli/db/db.sqlite`
        : undefined,
      log,
    })
    backends.set('zcode', zcode)
    board.addBackend(zcode)
    await zcode.start()
    log(`[agent-deck] zcode backend started`)
  }

  if (config.enabledBackends.includes('codex')) {
    const codex = new CodexAdapter({ log })
    backends.set('codex', codex)
    board.addBackend(codex)
    await codex.start()
    log(`[agent-deck] codex backend started`)
  }

  // ─── 3. ActionRouter ──────────────────────────────────────────────────────
  const router = new ActionRouter({
    board,
    getSlotBinding: (i) => {
      const state = board.getBoardState()
      if (!state) return undefined
      return state.slots.find((s) => s.i === i)
    },
    getBackend: (id) => backends.get(id),
    log,
  })

  // ─── 4. Gateway + Bridge（用 let 解决循环依赖）─────────────────────────────
  // bridge 需要 gateway，gateway.onMessage 需要 bridge，用 mutable ref 解耦
  const bridgeRef: { current: SimulatorBridge | null } = { current: null }

  const gateway = new Gateway(
    { port: config.port, host: '127.0.0.1' },
    {
      onConnect: (ws) => {
        log(`[gateway] client connected (${gateway.clientCount()})`)
        // 新连接立即推送当前状态
        const state = board.getBoardState()
        const frame = board.getLedFrame()
        if (state) ws.send(JSON.stringify(state))
        if (frame) ws.send(JSON.stringify(frame))
      },
      onDisconnect: () => {
        log(`[gateway] client disconnected (${gateway.clientCount()})`)
      },
      onMessage: async (msg) => {
        const b = bridgeRef.current
        if (!b) return
        try {
          await b.handleClientMessage(msg)
        } catch (err) {
          log(`[gateway] message handler error:`, err)
        }
      },
    },
  )
  await gateway.start()
  log(`[agent-deck] gateway listening on ws://127.0.0.1:${config.port}`)

  // ─── 5. SimulatorBridge ──────────────────────────────────────────────────
  const bridge = new SimulatorBridge({ board, router, gateway })
  bridge.start()
  bridgeRef.current = bridge
  log(`[agent-deck] simulator bridge started`)

  // ─── 6. 周期性 recompute（urgency 渐变需要）─────────────────────────────────
  const recomputeTimer = setInterval(() => {
    try {
      board.recompute()
    } catch (err) {
      log(`[agent-deck] recompute error:`, err)
    }
  }, 1000)

  // ─── 7. 信号处理 ──────────────────────────────────────────────────────────
  let shuttingDown = false
  const shutdown = async (signal: string): Promise<void> => {
    if (shuttingDown) return
    shuttingDown = true
    log(`[agent-deck] received ${signal}, shutting down...`)
    clearInterval(recomputeTimer)
    try {
      bridge.stop()
      await gateway.stop()
      for (const backend of backends.values()) {
        await backend.shutdown()
      }
      board.dispose()
    } catch (err) {
      console.error('[agent-deck] error during shutdown:', err)
    }
    process.exit(0)
  }

  process.on('SIGINT', () => void shutdown('SIGINT'))
  process.on('SIGTERM', () => void shutdown('SIGTERM'))

  log(`[agent-deck] ready ✓  (Ctrl+C to quit)`)
}

function makeLogger(
  config: HostConfig,
): (msg: string, ...args: unknown[]) => void {
  if (config.debug) {
    return (msg, ...args) => console.log(msg, ...args)
  }
  return (msg, ...args) => {
    if (msg.startsWith('[agent-deck]')) {
      console.log(msg, ...args)
    }
  }
}

main().catch((err) => {
  console.error('[agent-deck] fatal:', err)
  process.exit(1)
})
