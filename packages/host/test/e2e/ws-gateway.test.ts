/**
 * E2E: WS Gateway 双向通信
 *
 * 覆盖 main.ts 第 76-112 行的 gateway ↔ bridge ↔ router 闭环：
 *   1. 新 client 连接时立即收到当前 board + leds（onConnect 回放）
 *   2. client 发 focus → board 更新 focus → 推回 board
 *   3. client 发 accept（V1 backend 未实现 accept 方法）→ 走 unsupported 降级
 *      不应导致连接断开或 host 崩
 *   4. 多个 client 连接，状态变化时都收到广播
 *   5. HTTP /health 正常响应
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { createZcodeFixture, insertTask, type FixtureDb } from './fixtures.js'
import { startTestHost, allocatePort, type TestHost } from './testHost.js'
import { createWsHarness, lastOfType, type WsHarness } from './wsHarness.js'
import type { BoardState, LedFrame, ClientMessage } from '@agent-deck/protocol'

interface Ctx {
  fixture: FixtureDb
  host: TestHost
  ws: WsHarness
  port: number
}

async function setup(ctx: Ctx): Promise<void> {
  ctx.fixture = createZcodeFixture()
  ctx.port = await allocatePort()
  ctx.host = await startTestHost({
    tasksDbPath: ctx.fixture.tasksDbPath,
    toolDbPath: ctx.fixture.toolDbPath,
    port: ctx.port,
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

function send(ws: WsHarness, msg: ClientMessage): void {
  ws.ws.send(JSON.stringify(msg))
}

describe('e2e: WS gateway 双向通信', () => {
  const ctx: Ctx = {} as Ctx

  beforeEach(async () => {
    await teardown(ctx)
    await setup(ctx)
  })

  afterEach(async () => {
    await teardown(ctx)
  })

  it('新连接立即收到当前 board + leds（onConnect 回放）', async () => {
    // 先放一条数据，让 board 有内容
    insertTask(ctx.fixture, {
      task_id: 'sess_init',
      task_status: 'running',
      updated_at: Date.now(),
    })
    // 等 host 把它推给已连接的 ws
    await ctx.ws.waitFor(
      (msgs) =>
        msgs.some(
          (m) =>
            m.type === 'board' &&
            (m as BoardState).slots.some((s) => s.sessionId === 'sess_init'),
        ),
      { timeoutMs: 1500, label: 'initial board has sess_init' },
    )

    // 现在第二个 client 连进来，应该立即收到当前状态
    const ws2 = await createWsHarness(ctx.port)
    try {
      await ws2.waitFor(
        (msgs) =>
          msgs.some((m) => m.type === 'board') &&
          msgs.some((m) => m.type === 'leds'),
        { timeoutMs: 1000, label: 'ws2 收到 board + leds 回放' },
      )
      // ws2 收到的 board 里也应该有 sess_init
      const board = lastOfType<BoardState>(ws2.messages, 'board')
      expect(board).toBeDefined()
      const ids = board!.slots
        .map((s) => s.sessionId)
        .filter((x): x is string => Boolean(x))
      expect(ids).toContain('sess_init')
    } finally {
      await ws2.close()
    }
  })

  it('client 发 focus → board 更新 focus 字段并推回', async () => {
    insertTask(ctx.fixture, {
      task_id: 'sess_focus',
      task_status: 'running',
      updated_at: Date.now(),
    })
    await ctx.ws.waitFor(
      (msgs) => msgs.some((m) => m.type === 'leds'),
      { timeoutMs: 1500 },
    )

    // 把 focus 切到槽 2
    send(ctx.ws, { t: 'action', action: { op: 'focus', i: 2 } })

    await ctx.ws.waitFor(
      (msgs) =>
        msgs.some(
          (m) => m.type === 'board' && (m as BoardState).focus === 2,
        ),
      { timeoutMs: 1000, label: 'board.focus 推到 2' },
    )
    const board = lastOfType<BoardState>(ctx.ws.messages, 'board')
    expect(board!.focus).toBe(2)
    // 对应槽位 focused=true
    const focusedSlot = board!.slots.find((s) => s.i === 2)
    expect(focusedSlot).toBeDefined()
    expect(focusedSlot!.focused).toBe(true)
  })

  it('client 发 key id=a2 down → 等同于 focus 槽 1', async () => {
    insertTask(ctx.fixture, {
      task_id: 'sess_key',
      task_status: 'running',
      updated_at: Date.now(),
    })
    await ctx.ws.waitFor(
      (msgs) => msgs.some((m) => m.type === 'leds'),
      { timeoutMs: 1500 },
    )

    send(ctx.ws, { t: 'key', id: 'a2', edge: 'down' })

    await ctx.ws.waitFor(
      (msgs) =>
        msgs.some(
          (m) => m.type === 'board' && (m as BoardState).focus === 1,
        ),
      { timeoutMs: 1000, label: 'key a2 → focus=1' },
    )
  })

  it('V1 backend 无 accept 实现 → client 发 accept 不崩、连接保持', async () => {
    insertTask(ctx.fixture, {
      task_id: 'sess_unsupported',
      task_status: 'running',
      updated_at: Date.now(),
    })
    // 等 board 真正把 sess_unsupported 占到槽里
    await ctx.ws.waitFor(
      (msgs) =>
        msgs.some(
          (m) =>
            m.type === 'board' &&
            (m as BoardState).slots.some(
              (s) => s.sessionId === 'sess_unsupported',
            ),
        ),
      { timeoutMs: 1500, label: 'sess_unsupported 占槽' },
    )
    const beforeBoard = lastOfType<BoardState>(ctx.ws.messages, 'board')
    const occupiedIdx = beforeBoard!.slots.findIndex(
      (s) => s.sessionId === 'sess_unsupported',
    )
    expect(occupiedIdx).toBeGreaterThanOrEqual(0)
    send(ctx.ws, { t: 'action', action: { op: 'focus', i: occupiedIdx } })
    await ctx.ws.waitFor(
      (msgs) =>
        msgs.some(
          (m) =>
            m.type === 'board' && (m as BoardState).focus === occupiedIdx,
        ),
      { timeoutMs: 1000 },
    )

    // 发 accept：V1 应优雅降级（unsupported），不抛、不断连
    send(ctx.ws, { t: 'action', action: { op: 'accept' } })
    // 等 200ms 确认没有 close 事件、连接仍在
    await new Promise((r) => setTimeout(r, 200))
    expect(ctx.ws.ws.readyState).toBe(1) // OPEN=1
  })

  it('client 发非法 JSON → 收到 error event，连接保持', async () => {
    ctx.ws.ws.send('not a json')
    await ctx.ws.waitFor(
      (msgs) =>
        msgs.some(
          (m) => m.type === 'event' && (m as { event: string }).event === 'error',
        ),
      { timeoutMs: 1000, label: '收到 invalid json error event' },
    )
    expect(ctx.ws.ws.readyState).toBe(1)
  })

  it('多 client 同时连接，状态变化都收到广播', async () => {
    const ws2 = await createWsHarness(ctx.port)
    try {
      insertTask(ctx.fixture, {
        task_id: 'sess_broadcast',
        task_status: 'running',
        updated_at: Date.now(),
      })
      // 两个 client 都应在合理时间内收到 sess_broadcast
      await Promise.all([
        ctx.ws.waitFor(
          (msgs) =>
            msgs.some(
              (m) =>
                m.type === 'board' &&
                (m as BoardState).slots.some(
                  (s) => s.sessionId === 'sess_broadcast',
                ),
            ),
          { timeoutMs: 1500, label: 'ws1 收到广播' },
        ),
        ws2.waitFor(
          (msgs) =>
            msgs.some(
              (m) =>
                m.type === 'board' &&
                (m as BoardState).slots.some(
                  (s) => s.sessionId === 'sess_broadcast',
                ),
            ),
          { timeoutMs: 1500, label: 'ws2 收到广播' },
        ),
      ])
    } finally {
      await ws2.close()
    }
  })

  it('HTTP /health 返回 ok + clients 数', async () => {
    // 至少已有一个 client（ctx.ws）
    const resp = await fetch(`http://127.0.0.1:${ctx.port}/health`)
    expect(resp.status).toBe(200)
    const body = (await resp.json()) as { ok: boolean; clients: number; uptime: number }
    expect(body.ok).toBe(true)
    expect(body.clients).toBeGreaterThanOrEqual(1)
    expect(body.uptime).toBeGreaterThanOrEqual(0)
  })

  it('HTTP 404 对未知路径', async () => {
    const resp = await fetch(`http://127.0.0.1:${ctx.port}/nonexistent`)
    expect(resp.status).toBe(404)
  })

  it('leds 帧的形状严格符合 protocol（5 槽位，type=leds）', async () => {
    insertTask(ctx.fixture, {
      task_id: 'sess_shape',
      task_status: 'running',
      updated_at: Date.now(),
    })
    const led = await (async () => {
      await ctx.ws.waitFor(
        (msgs) => msgs.some((m) => m.type === 'leds'),
        { timeoutMs: 1500 },
      )
      return lastOfType<LedFrame>(ctx.ws.messages, 'leds')!
    })()

    expect(led.type).toBe('leds')
    expect(led.slots).toHaveLength(5)
    for (const slot of led.slots) {
      expect(slot).toHaveProperty('i')
      expect(slot).toHaveProperty('br')
      expect(slot).toHaveProperty('fx')
      expect(['solid', 'breathe', 'blink_slow', 'blink_fast']).toContain(slot.fx)
      if (slot.rgb === null) {
        expect(slot.br).toBe(0)
      } else {
        expect(Array.isArray(slot.rgb)).toBe(true)
        expect(slot.rgb).toHaveLength(3)
        for (const v of slot.rgb) {
          expect(v).toBeGreaterThanOrEqual(0)
          expect(v).toBeLessThanOrEqual(255)
        }
      }
    }
  })
})
