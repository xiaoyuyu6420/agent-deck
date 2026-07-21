#!/usr/bin/env tsx
/**
 * verify —— Phase 0 手动验证脚本
 *
 * 起一份连真实 ~/.zcode 的 host，把每次 board 状态变化彩色打印出来。
 * 用来人肉验证 Phase 0 验收 Demo：
 *   1. ZCode Desktop 跑任务 → A1 蓝
 *   2. 弹确认 → A1 橙
 *   3. 点 Accept → A1 蓝
 *   4. 完成 → A1 绿
 *   5. 出错 → 红
 *
 * 用法：
 *   pnpm verify                         # 用真实 ~/.zcode
 *   pnpm verify --zcode-home /tmp/fake  # 指向其他路径
 *   pnpm verify --port 9000             # 指定端口（默认动态分配）
 *
 * 安全：自动把 cwd 加进 excludeWorkspaces，避免 verify 自己跑的 ZCode 任务被自己看到。
 */

import { homedir } from 'node:os'
import { join, resolve } from 'node:path'
import { argv, exit, stdout, stderr } from 'node:process'

import { SessionBoard } from '../src/board/SessionBoard.js'
import { ZcodeAdapter } from '../src/backends/zcode/ZcodeAdapter.js'
import { Gateway } from '../src/gateway/server.js'
import { SimulatorBridge } from '../src/device/SimulatorBridge.js'
import { ActionRouter } from '../src/actions/ActionRouter.js'
import type { BoardState, LedFrame, SlotBinding, DeckStatus } from '@agent-deck/protocol'
import type { AgentBackend } from '../src/backends/types.js'

/** ActionRouter 期望的 backend 形状（接受 AgentBackend 或退化为含 shutdown 的最小集） */
type AgentBackendLike = AgentBackend

// ─────────────────────────────────────────────────────────────────────────────
// CLI 参数
// ─────────────────────────────────────────────────────────────────────────────

interface Args {
  zcodeHome: string
  port: number | 'auto'
  raw: boolean
}

function parseArgs(): Args {
  const args: Args = {
    zcodeHome: join(homedir(), '.zcode'),
    port: 'auto',
    raw: false,
  }
  for (let i = 2; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--zcode-home') {
      args.zcodeHome = argv[++i] ?? args.zcodeHome
    } else if (a === '--port') {
      const v = argv[++i]
      args.port = v && /^\d+$/.test(v) ? Number(v) : 'auto'
    } else if (a === '--raw') {
      args.raw = true
    } else if (a === '-h' || a === '--help') {
      stdout.write(USAGE)
      exit(0)
    }
  }
  return args
}

const USAGE = `agent-deck verify —— Phase 0 手动验证

用法：
  pnpm verify [options]

Options:
  --zcode-home <path>  ZCode 主目录（默认 ~/.zcode）
  --port <number>      Gateway 端口（默认动态分配）
  --raw                同时打印原始灯帧 JSON
  -h, --help           显示帮助

操作：
  1. 启动后保持运行
  2. 在 ZCode Desktop 里跑一个任务（例如让它 Bash 跑个 sleep 30）
  3. 当 ZCode 弹确认时，看下面打印的槽位是否变橙
  4. 在 ZCode 里点 Accept，看槽位是否变蓝
  5. 任务完成，看是否变绿
  6. Ctrl+C 退出
`

// ─────────────────────────────────────────────────────────────────────────────
// ANSI 渲染
// ─────────────────────────────────────────────────────────────────────────────

const ANSI = {
  reset: '\x1b[0m',
  bold: '\x1b[1m',
  dim: '\x1b[2m',
  // 24-bit 真彩色背景
  bg: (r: number, g: number, b: number): string =>
    `\x1b[48;2;${r};${g};${b}m`,
  fg: (r: number, g: number, b: number): string =>
    `\x1b[38;2;${r};${g};${b}m`,
  // 状态颜色（文字用）
  gray: '\x1b[90m',
  green: '\x1b[32m',
  yellow: '\x1b[33m',
  red: '\x1b[31m',
  blue: '\x1b[34m',
  magenta: '\x1b[35m',
  cyan: '\x1b[36m',
}

const STATUS_COLOR: Record<DeckStatus, string> = {
  off: ANSI.gray,
  idle: ANSI.gray,
  working: ANSI.blue,
  waiting: ANSI.yellow,
  done: ANSI.green,
  error: ANSI.red,
}

/** 把一个 RGB 三元组渲染成 8 字符宽的色块 */
function colorBlock(rgb: [number, number, number] | null): string {
  if (rgb === null) return `${ANSI.dim}${ANSI.bg(40, 40, 40)}        ${ANSI.reset}`
  const [r, g, b] = rgb
  // 文字用对比色：亮度高用黑，低用白
  const lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255
  const fg = lum > 0.55 ? ANSI.fg(0, 0, 0) : ANSI.fg(255, 255, 255)
  return `${ANSI.bg(r, g, b)}${fg}  ████  ${ANSI.reset}`
}

/** 单个槽位的可视化：[色块] A1  working  title (detail) */
function renderSlot(slot: SlotBinding, led: LedSlotLike): string {
  const idx = `A${slot.i + 1}`
  const statusStr = `${STATUS_COLOR[slot.status]}${slot.status.padEnd(7)}${ANSI.reset}`
  const color = colorBlock(led.rgb)
  let meta = ''
  if (slot.sessionId) {
    const title = slot.title?.slice(0, 40) ?? '(untitled)'
    const backend = slot.backend ?? '?'
    meta = `${ANSI.dim}${backend}/${slot.sessionId.slice(0, 12)}${ANSI.reset}  ${title}`
    if (slot.detail) {
      meta += `\n         ${ANSI.dim}↳ ${slot.detail.slice(0, 60)}${ANSI.reset}`
    }
    if (slot.focused) {
      meta += ` ${ANSI.cyan}(focused)${ANSI.reset}`
    }
  } else {
    meta = `${ANSI.dim}— empty —${ANSI.reset}`
  }
  return `  ${ANSI.bold}${idx}${ANSI.reset} ${color}  ${statusStr}  ${meta}`
}

interface LedSlotLike {
  i: number
  rgb: [number, number, number] | null
  br: number
  fx: string
}

/** 把 board + leds 渲染成一段可读文本 */
function renderBoard(board: BoardState, led: LedFrame, raw: boolean): string {
  const now = new Date().toLocaleTimeString()
  const lines: string[] = []
  lines.push(
    `${ANSI.dim}── ${now} ──${ANSI.reset}  mode=${board.mode} focus=A${board.focus + 1}`,
  )
  for (let i = 0; i < board.slots.length; i++) {
    const slot = board.slots[i]!
    const ledSlot = led.slots.find((s) => s.i === i) ?? {
      i,
      rgb: null,
      br: 0,
      fx: 'solid',
    }
    lines.push(renderSlot(slot, ledSlot as LedSlotLike))
  }
  if (raw) {
    lines.push(`${ANSI.dim}leds: ${JSON.stringify(led)}${ANSI.reset}`)
  }
  return lines.join('\n')
}

// ─────────────────────────────────────────────────────────────────────────────
// 端口分配
// ─────────────────────────────────────────────────────────────────────────────

async function allocatePort(): Promise<number> {
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

// ─────────────────────────────────────────────────────────────────────────────
// 主流程
// ─────────────────────────────────────────────────────────────────────────────

async function main(): Promise<void> {
  const args = parseArgs()

  // 自动防自指：把 cwd 和 agent-deck 仓库根加进 exclude
  const cwd = process.cwd()
  const repoRoot = resolve(import.meta.dirname, '..', '..', '..')
  const excludeWorkspaces = [cwd, repoRoot]

  const port =
    args.port === 'auto' ? await allocatePort() : (args.port as number)

  stderr.write(
    `${ANSI.bold}agent-deck verify${ANSI.reset}\n` +
      `  zcodeHome:        ${args.zcodeHome}\n` +
      `  tasks DB:         ${join(args.zcodeHome, 'v2', 'tasks-index.sqlite')}\n` +
      `  tool_usage DB:    ${join(args.zcodeHome, 'cli', 'db', 'db.sqlite')}\n` +
      `  gateway port:     ${port}\n` +
      `  excludeWorkspaces:\n` +
      excludeWorkspaces.map((w) => `    - ${w}`).join('\n') +
      '\n' +
      `${ANSI.dim}（在 ZCode Desktop 里跑个任务，下面会显示状态变化。Ctrl+C 退出）${ANSI.reset}\n\n`,
  )

  // 用可控 now 的 board
  const board = new SessionBoard({ now: () => Date.now() })

  const zcode = new ZcodeAdapter({
    tasksDbPath: join(args.zcodeHome, 'v2', 'tasks-index.sqlite'),
    toolDbPath: join(args.zcodeHome, 'cli', 'db', 'db.sqlite'),
    pollIntervalMs: 500,
    excludeWorkspaces,
    failOnMissing: false,
    log: (msg) => stderr.write(`${ANSI.dim}${msg}${ANSI.reset}\n`),
  })

  const backends = new Map<string, AgentBackendLike>([['zcode', zcode]])
  board.addBackend(zcode)
  await zcode.start()

  const router = new ActionRouter({
    board,
    getSlotBinding: (i) => {
      const state = board.getBoardState()
      if (!state) return undefined
      return state.slots.find((s) => s.i === i)
    },
    getBackend: (id) => backends.get(id),
    log: (msg) => stderr.write(`${ANSI.dim}${msg}${ANSI.reset}\n`),
  })

  const bridgeRef: { current: SimulatorBridge | null } = { current: null }

  const gateway = new Gateway(
    { port, host: '127.0.0.1' },
    {
      onConnect: (ws) => {
        const state = board.getBoardState()
        const frame = board.getLedFrame()
        if (state) ws.send(JSON.stringify(state))
        if (frame) ws.send(JSON.stringify(frame))
      },
      onMessage: async (msg) => {
        try {
          await bridgeRef.current?.handleClientMessage(msg)
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

  // 订阅 board 变化，打印
  let lastSig = ''
  const printUpdate = (): void => {
    const state = board.getBoardState()
    const led = board.getLedFrame()
    if (!state || !led) return
    // 用 (sessionId, status) 签名去重，避免 recompute 抖动刷屏
    const sig = JSON.stringify(state.slots.map((s) => [s.sessionId, s.status, s.detail]))
    if (sig === lastSig) return
    lastSig = sig
    stdout.write(renderBoard(state, led, args.raw) + '\n\n')
  }

  board.onBoardState(() => printUpdate())
  // 周期打印 urgency 渐变（waiting 槽位）
  setInterval(() => printUpdate(), 1000).unref?.()
  // 首帧
  printUpdate()

  // 优雅退出
  let shuttingDown = false
  const shutdown = async (sig: string): Promise<void> => {
    if (shuttingDown) return
    shuttingDown = true
    stderr.write(`\n${ANSI.dim}received ${sig}, shutting down...${ANSI.reset}\n`)
    try {
      bridge.stop()
      await gateway.stop()
      await zcode.shutdown()
      board.dispose()
    } catch (err) {
      console.error('error during shutdown:', err)
    }
    exit(0)
  }
  process.on('SIGINT', () => void shutdown('SIGINT'))
  process.on('SIGTERM', () => void shutdown('SIGTERM'))
}

main().catch((err) => {
  console.error('verify failed:', err)
  exit(1)
})
