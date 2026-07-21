/**
 * E2E: Phase 0 验收 Demo 的全链路
 *
 * 验收（对齐 docs/zcode-integration.md + 方案 §12 Phase 0）：
 *   1. ZCode 任务进入 working → simulator 槽变蓝 (working)
 *   2. 工具进入 requested → 同槽变橙 (waiting)
 *   3. 用户点 Accept（在 fixture 里写回 db）→ 同槽变蓝 (working)
 *   4. 任务完成 → 同槽变绿 (done)
 *   5. error 任务 → 红灯 (error)
 *   6. urgency：等待 > 2 分钟颜色偏红、fx 变 blink_*
 *   7. 自指防护：被 exclude 的 workspace 不亮灯
 *
 * 整个链路：
 *   fixture sqlite → ZcodeSqliteObserver → ZcodeAdapter → SessionBoard →
 *   SimulatorBridge → Gateway WS → 真 ws 客户端 → 断言 ServerMessage
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import {
  createZcodeFixture,
  insertTask,
  insertTool,
  updateTaskStatus,
  completeTool,
  type FixtureDb,
} from './fixtures.js'
import { startTestHost, allocatePort, type TestHost } from './testHost.js'
import { createWsHarness, lastOfType, type WsHarness } from './wsHarness.js'
import type { LedFrame, BoardState } from '@agent-deck/protocol'

interface Ctx {
  fixture: FixtureDb
  host: TestHost
  ws: WsHarness
  port: number
}

async function setup(ctx: Ctx, opts: { excludeWorkspaces?: string[] } = {}): Promise<void> {
  ctx.fixture = createZcodeFixture()
  ctx.port = await allocatePort()
  ctx.host = await startTestHost({
    tasksDbPath: ctx.fixture.tasksDbPath,
    toolDbPath: ctx.fixture.toolDbPath,
    port: ctx.port,
    excludeWorkspaces: opts.excludeWorkspaces,
  })
  ctx.ws = await createWsHarness(ctx.port)
}

async function teardown(ctx: Ctx): Promise<void> {
  try {
    await ctx.ws?.close()
  } catch {
    /* ignore */
  }
  try {
    await ctx.host?.shutdown()
  } catch {
    /* ignore */
  }
  ctx.fixture?.cleanup()
}


/** 等待 board 状态里出现某个 sessionId 的某 status。 */
async function waitForBoardStatus(
  ctx: Ctx,
  sessionId: string,
  status: string,
  timeoutMs = 1500,
): Promise<BoardState> {
  await ctx.ws.waitFor(
    (msgs) =>
      msgs.some(
        (m) =>
          m.type === 'board' &&
          (m as BoardState).slots.some(
            (s) => s.sessionId === sessionId && s.status === status,
          ),
      ),
    { timeoutMs, label: `board status=${status} for ${sessionId}` },
  )
  const board = lastOfType<BoardState>(ctx.ws.messages, 'board')
  if (!board) throw new Error('no board state received')
  return board
}

describe('e2e: ZCode sqlite → WS 灯帧 全链路', () => {
  const ctx: Ctx = {} as Ctx

  beforeEach(async () => {
    ;(ctx as unknown as Record<string, unknown>).ws = undefined
    await teardown(ctx)
    await setup(ctx)
  })

  afterEach(async () => {
    await teardown(ctx)
  })

  it('working 任务 → 槽位蓝色 breathe', async () => {
    insertTask(ctx.fixture, {
      task_id: 'sess_working',
      task_status: 'running',
      updated_at: Date.now(),
    })
    // 等 board 真正把 sess_working 标成 working，再取灯帧
    await waitForBoardStatus(ctx, 'sess_working', 'working')

    const led = lastOfType<LedFrame>(ctx.ws.messages, 'leds')!
    const occupied = led.slots.find(
      (s) => s.rgb !== null && s.fx === 'breathe',
    )
    expect(occupied, '至少有一个槽被占且 breathe').toBeDefined()
    // 蓝色基色 #304FFE = [48, 79, 254]
    const [r, g, b] = occupied!.rgb!
    expect(r).toBeLessThan(120) // 蓝色 R 较小
    expect(g).toBeLessThan(150)
    expect(b).toBeGreaterThan(200)
  })

  it('waiting（requested）任务 → 槽位橙色', async () => {
    const now = Date.now()
    insertTask(ctx.fixture, {
      task_id: 'sess_waiting',
      task_status: 'running',
      updated_at: now,
    })
    insertTool(ctx.fixture, {
      id: 'tu_1',
      session_id: 'sess_waiting',
      tool_name: 'Bash',
      side_effect_scope: 'shell',
      approval_status: 'requested',
      status: 'running',
      started_at: now,
      completed_at: null,
    })

    // 等 board 把它标成 waiting
    const board = await waitForBoardStatus(ctx, 'sess_waiting', 'waiting')
    const slot = board.slots.find((s) => s.sessionId === 'sess_waiting')
    expect(slot).toBeDefined()
    expect(slot!.status).toBe('waiting')
    expect(slot!.detail).toContain('Bash')

    // 灯帧：waiting 一定是偏橙/红色
    const led = lastOfType<LedFrame>(ctx.ws.messages, 'leds')
    expect(led).toBeDefined()
    const slotLed = led!.slots.find((s) => s.rgb !== null && (s.rgb[0] > 150))
    expect(slotLed, '应有暖色（橙/红）槽位').toBeDefined()
    const [r] = slotLed!.rgb!
    expect(r).toBeGreaterThan(180) // 橙红色 R 很高
  })

  it('完整 demo: working → waiting → accept → working → done', async () => {
    const t0 = Date.now()
    insertTask(ctx.fixture, {
      task_id: 'sess_flow',
      task_status: 'running',
      updated_at: t0,
    })

    // 1. working
    await waitForBoardStatus(ctx, 'sess_flow', 'working')

    // 2. 进入 waiting
    insertTool(ctx.fixture, {
      id: 'tu_flow',
      session_id: 'sess_flow',
      tool_name: 'Bash',
      side_effect_scope: 'git push',
      approval_status: 'requested',
      status: 'running',
      started_at: Date.now(),
      completed_at: null,
    })
    await waitForBoardStatus(ctx, 'sess_flow', 'waiting')

    // 3. 模拟用户点 Accept：在 fixture 里写回 tool_usage
    completeTool(ctx.fixture, 'tu_flow')
    await waitForBoardStatus(ctx, 'sess_flow', 'working')

    // 4. 任务完成
    updateTaskStatus(ctx.fixture, 'sess_flow', 'completed')
    await waitForBoardStatus(ctx, 'sess_flow', 'done')

    // 灯帧：done = 绿色 solid (#00FF4C)
    const led = lastOfType<LedFrame>(ctx.ws.messages, 'leds')
    expect(led).toBeDefined()
    const doneSlot = led!.slots.find((s) => s.rgb !== null)
    expect(doneSlot).toBeDefined()
    const [, g, b] = doneSlot!.rgb!
    expect(g).toBeGreaterThan(200) // 绿色 G 较大
    expect(b).toBeLessThan(150)
    expect(doneSlot!.fx).toBe('solid')
  })

  it('error 任务 → 红色 solid', async () => {
    insertTask(ctx.fixture, {
      task_id: 'sess_error',
      task_status: 'error',
      updated_at: Date.now(),
    })

    await waitForBoardStatus(ctx, 'sess_error', 'error')
    const led = lastOfType<LedFrame>(ctx.ws.messages, 'leds')
    expect(led).toBeDefined()
    const errSlot = led!.slots.find((s) => s.rgb !== null)
    expect(errSlot).toBeDefined()
    const [r, g, b] = errSlot!.rgb!
    expect(r).toBeGreaterThan(200) // 红色 R 高
    expect(g).toBeLessThan(80)
    expect(b).toBeLessThan(80)
    expect(errSlot!.fx).toBe('solid')
  })

  it('自指防护：excludeWorkspaces 命中的会话不出现', async () => {
    await teardown(ctx)
    await setup(ctx, { excludeWorkspaces: ['/self/excluded'] })

    insertTask(ctx.fixture, {
      task_id: 'sess_self',
      task_status: 'running',
      workspace_path: '/self/excluded/project',
      updated_at: Date.now(),
    })
    insertTask(ctx.fixture, {
      task_id: 'sess_other',
      task_status: 'running',
      workspace_path: '/somewhere/else',
      updated_at: Date.now(),
    })

    // 给观察者时间扫一遍
    await waitForBoardStatus(ctx, 'sess_other', 'working')

    // 再等一会儿，确保观察者扫过 self
    await new Promise((r) => setTimeout(r, 200))
    const board = lastOfType<BoardState>(ctx.ws.messages, 'board')
    expect(board).toBeDefined()
    const allSessionIds = board!.slots
      .map((s) => s.sessionId)
      .filter((x): x is string => Boolean(x))
    expect(allSessionIds).not.toContain('sess_self')
    expect(allSessionIds).toContain('sess_other')
  })

  it('防陈旧：30 分钟前的 requested 不算 waiting', async () => {
    const recent = Date.now()
    const stale = Date.now() - 31 * 60 * 1000 // 31 min ago
    insertTask(ctx.fixture, {
      task_id: 'sess_stale',
      task_status: 'running',
      updated_at: recent,
    })
    insertTool(ctx.fixture, {
      id: 'tu_stale',
      session_id: 'sess_stale',
      tool_name: 'Bash',
      approval_status: 'requested',
      status: 'running',
      started_at: stale, // 超出 30 分钟窗口
      completed_at: null,
    })

    // 等到 sess_stale 出现在 board 里
    await ctx.ws.waitFor(
      (msgs) =>
        msgs.some(
          (m) =>
            m.type === 'board' &&
            (m as BoardState).slots.some((s) => s.sessionId === 'sess_stale'),
        ),
      { timeoutMs: 1500, label: 'sess_stale appears in board' },
    )
    const board = lastOfType<BoardState>(ctx.ws.messages, 'board')
    const slot = board!.slots.find((s) => s.sessionId === 'sess_stale')
    expect(slot).toBeDefined()
    // 关键：陈旧 requested 不应升级为 waiting，仍是 working
    expect(slot!.status).toBe('working')
  })
})

describe('e2e: urgency 渐变（waiting 随时间偏红）', () => {
  const ctx: Ctx = {} as Ctx

  afterEach(async () => {
    await teardown(ctx)
  })

  it('waiting 30s 内 fx=solid；mock now 推到 3min 后 fx=blink_fast', async () => {
    // 用可控的 now 注入 board，让 urgency 受控推进
    let mockNow = Date.now()
    const fixture = createZcodeFixture()
    const port = await allocatePort()
    ctx.fixture = fixture
    ctx.port = port

    const host = await startTestHost({
      tasksDbPath: fixture.tasksDbPath,
      toolDbPath: fixture.toolDbPath,
      port,
      now: () => mockNow,
    })
    ctx.host = host
    ctx.ws = await createWsHarness(port)

    insertTask(fixture, {
      task_id: 'sess_urgency',
      task_status: 'running',
      updated_at: mockNow,
    })
    insertTool(fixture, {
      id: 'tu_urgency',
      session_id: 'sess_urgency',
      tool_name: 'AskUserQuestion',
      side_effect_scope: 'userInteraction',
      approval_status: 'requested',
      status: 'running',
      started_at: mockNow,
      completed_at: null,
    })

    // 等进入 waiting
    await waitForBoardStatus(ctx, 'sess_urgency', 'waiting', 2000)
    let led = lastOfType<LedFrame>(ctx.ws.messages, 'leds')!
    let slot = led.slots.find((s) => s.rgb !== null)!
    // 刚进入：fx=solid（urgency<0.33）
    expect(slot.fx, '刚进入 waiting 应是 solid').toBe('solid')

    // 推时间到 3 分钟后，触发 recompute
    mockNow += 3 * 60 * 1000
    host.board.recompute()

    await ctx.ws.waitFor(
      (msgs) => {
        const l = lastOfType<LedFrame>(msgs, 'leds')
        if (!l) return false
        const s = l.slots.find((x) => x.rgb !== null)
        return Boolean(s && s.fx === 'blink_fast')
      },
      { timeoutMs: 1500, label: 'urgency 推进后 fx=blink_fast' },
    )

    led = lastOfType<LedFrame>(ctx.ws.messages, 'leds')!
    slot = led.slots.find((s) => s.rgb !== null)!
    expect(slot.fx).toBe('blink_fast')
    // 3 min 后 urgency=1，颜色应接近 #FF2200 [255, 34, 0]
    const [r, g, b] = slot.rgb!
    expect(r).toBeGreaterThan(240)
    expect(g).toBeLessThan(80)
    expect(b).toBeLessThan(40)
  })
})
