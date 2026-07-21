import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

import {
  SLOT_COUNT,
  DONE_TTL_MS,
  type DeckStatus,
} from '@agent-deck/protocol'

import {
  allocateSlots,
  type ScoredSession,
} from '../src/board/slotAllocator.js'

// ─────────────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────────────

function makeSession(overrides: Partial<ScoredSession>): ScoredSession {
  return {
    backend: 'zcode',
    sessionId: 'sess_test',
    title: 'test',
    status: 'idle',
    urgency: 0,
    updatedAt: Date.now(),
    ...overrides,
  }
}

const ALL_STATUSES: DeckStatus[] = [
  'off',
  'idle',
  'working',
  'waiting',
  'done',
  'error',
]

// ─────────────────────────────────────────────────────────────────────────────
// 固定 now，避免 DONE_TTL 边界受真实时间影响
// ─────────────────────────────────────────────────────────────────────────────

const NOW = 1_700_000_000_000

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(NOW)
})

afterEach(() => {
  vi.useRealTimers()
})

// ─────────────────────────────────────────────────────────────────────────────
// tests
// ─────────────────────────────────────────────────────────────────────────────

describe('allocateSlots', () => {
  it('1. 空 sessions → 5 个空槽', () => {
    const slots = allocateSlots([])
    expect(slots).toHaveLength(SLOT_COUNT)
    expect(slots.map((s) => s.i)).toEqual([0, 1, 2, 3, 4])
    for (const s of slots) {
      expect(s.session).toBeUndefined()
      expect(s.pinned).toBe(false)
    }
  })

  it('2. 1 个 waiting session → 占 slot 0', () => {
    const sess = makeSession({ sessionId: 'a', status: 'waiting' })
    const slots = allocateSlots([sess])
    expect(slots[0]?.session?.sessionId).toBe('a')
    expect(slots[0]?.pinned).toBe(false)
    for (let i = 1; i < SLOT_COUNT; i++) {
      expect(slots[i]?.session).toBeUndefined()
    }
  })

  it('3. 5 个不同 status 的 sessions → 按 priority 排序占 0-4', () => {
    // priority: waiting(5) > error(4) > working(3) > done(2) > idle(1)
    const sessions = [
      makeSession({ sessionId: 'idle', status: 'idle', updatedAt: NOW }),
      makeSession({ sessionId: 'done', status: 'done', updatedAt: NOW }),
      makeSession({ sessionId: 'working', status: 'working', updatedAt: NOW }),
      makeSession({ sessionId: 'error', status: 'error', updatedAt: NOW }),
      makeSession({ sessionId: 'waiting', status: 'waiting', updatedAt: NOW }),
    ]
    const slots = allocateSlots(sessions)
    expect(slots.map((s) => s.session?.sessionId)).toEqual([
      'waiting',
      'error',
      'working',
      'done',
      'idle',
    ])
  })

  it('4. 6 个 sessions → 只有前 5 个占槽，第 6 个被丢弃', () => {
    // 构造 6 个会话，priority 各不同（off 最低应被丢）
    const sessions: ScoredSession[] = ALL_STATUSES.map((status, idx) =>
      makeSession({
        sessionId: `s${idx}-${status}`,
        status,
        // 给相同的 updatedAt 以便 status 是唯一决定因素
        updatedAt: NOW,
        urgency: 0,
      }),
    )
    // ALL_STATUSES 包含 6 种状态：off, idle, working, waiting, done, error
    expect(sessions).toHaveLength(6)

    const slots = allocateSlots(sessions)
    const ids = slots.map((s) => s.session?.sessionId)
    // off (priority 0) 应该被丢弃
    expect(ids).not.toContain(undefined)
    expect(ids).toHaveLength(SLOT_COUNT)
    expect(ids).not.toContain(expect.stringContaining('-off'))
    // 顺序应为 priority 降序
    expect(ids).toEqual([
      's3-waiting',
      's5-error',
      's2-working',
      's4-done',
      's1-idle',
    ])
  })

  it('5. pinned session → 即使优先级最低也占指定槽', () => {
    // 高优先级的 waiting 抢前槽，但 pin 一个 off 状态到 slot 2
    const sessions = [
      makeSession({ sessionId: 'w1', status: 'waiting', updatedAt: NOW }),
      makeSession({ sessionId: 'w2', status: 'waiting', updatedAt: NOW - 1 }),
      makeSession({ sessionId: 'pinned-off', status: 'off', updatedAt: NOW }),
      makeSession({ sessionId: 'e1', status: 'error', updatedAt: NOW }),
    ]
    const pins = new Map<number, string>([[2, 'pinned-off']])
    const slots = allocateSlots(sessions, { pins })

    expect(slots[2]?.session?.sessionId).toBe('pinned-off')
    expect(slots[2]?.pinned).toBe(true)

    // 其余按 priority 填到剩余空槽（waiting=5 在 error=4 前）
    // slot 0=w1, slot 1=w2, slot 2=pinned, slot 3=e1, slot 4=空
    expect(slots[0]?.session?.sessionId).toBe('w1')
    expect(slots[1]?.session?.sessionId).toBe('w2')
    expect(slots[3]?.session?.sessionId).toBe('e1')
    expect(slots[4]?.session).toBeUndefined()
    expect(slots[0]?.pinned).toBe(false)
  })

  it('6. pinned 但 sessionId 不存在 → 槽位留空但 pinned=true', () => {
    const sessions = [
      makeSession({ sessionId: 'a', status: 'waiting', updatedAt: NOW }),
    ]
    const pins = new Map<number, string>([[3, 'ghost-id']])
    const slots = allocateSlots(sessions, { pins })

    expect(slots[3]?.session).toBeUndefined()
    expect(slots[3]?.pinned).toBe(true)
    // 普通会话仍按规则占 slot 0
    expect(slots[0]?.session?.sessionId).toBe('a')
  })

  it('7. 过期 done（updatedAt 很早）→ 不占槽', () => {
    const freshDone = makeSession({
      sessionId: 'fresh-done',
      status: 'done',
      updatedAt: NOW - 1000, // 1s 前，远小于 TTL
    })
    const expiredDone = makeSession({
      sessionId: 'expired-done',
      status: 'done',
      updatedAt: NOW - DONE_TTL_MS - 1000, // 超过 TTL
    })
    const idle = makeSession({
      sessionId: 'idle1',
      status: 'idle',
      updatedAt: NOW,
    })

    const slots = allocateSlots([expiredDone, freshDone, idle])
    const ids = slots.map((s) => s.session?.sessionId)

    expect(ids).not.toContain('expired-done')
    expect(ids).toContain('fresh-done')
    expect(ids).toContain('idle1')
    // fresh-done (priority 2) 应在 idle1 (priority 1) 前
    expect(ids.indexOf('fresh-done')).toBeLessThan(ids.indexOf('idle1'))
  })

  it('8. focus 不影响槽位分配，只是透传', () => {
    const sessions = [
      makeSession({ sessionId: 'a', status: 'waiting', updatedAt: NOW }),
      makeSession({ sessionId: 'b', status: 'idle', updatedAt: NOW }),
    ]

    const withoutFocus = allocateSlots(sessions)
    const withFocus = allocateSlots(sessions, { focus: 2 })

    // 分配结果应完全相同
    expect(withFocus.map((s) => s.session?.sessionId)).toEqual(
      withoutFocus.map((s) => s.session?.sessionId),
    )
    // waiting a 占 slot 0，idle b 占 slot 1，slot 2 空
    expect(withFocus[0]?.session?.sessionId).toBe('a')
    expect(withFocus[1]?.session?.sessionId).toBe('b')
    expect(withFocus[2]?.session).toBeUndefined()
    expect(withFocus[1]?.pinned).toBe(false)
  })

  it('9. 同 priority 时按 urgency 降序', () => {
    const sessions = [
      makeSession({
        sessionId: 'low',
        status: 'waiting',
        urgency: 0.1,
        updatedAt: NOW,
      }),
      makeSession({
        sessionId: 'high',
        status: 'waiting',
        urgency: 0.9,
        updatedAt: NOW,
      }),
      makeSession({
        sessionId: 'mid',
        status: 'waiting',
        urgency: 0.5,
        updatedAt: NOW,
      }),
    ]
    const slots = allocateSlots(sessions)
    expect(slots.map((s) => s.session?.sessionId)).toEqual([
      'high',
      'mid',
      'low',
      undefined,
      undefined,
    ])
  })

  it('10. 同 priority + urgency 时按 updatedAt 降序', () => {
    const sessions = [
      makeSession({
        sessionId: 'old',
        status: 'working',
        urgency: 0.5,
        updatedAt: NOW - 2000,
      }),
      makeSession({
        sessionId: 'newest',
        status: 'working',
        urgency: 0.5,
        updatedAt: NOW,
      }),
      makeSession({
        sessionId: 'mid',
        status: 'working',
        urgency: 0.5,
        updatedAt: NOW - 1000,
      }),
    ]
    const slots = allocateSlots(sessions)
    expect(slots.map((s) => s.session?.sessionId)).toEqual([
      'newest',
      'mid',
      'old',
      undefined,
      undefined,
    ])
  })

  // ─────────────────────────────────────────────────────────────────────────
  // 边界补充
  // ─────────────────────────────────────────────────────────────────────────

  it('同一个 sessionId 不能占两个槽（去重）', () => {
    const same = makeSession({
      sessionId: 'dup',
      status: 'waiting',
      updatedAt: NOW,
    })
    const slots = allocateSlots([same, same, same])
    const ids = slots.map((s) => s.session?.sessionId).filter(Boolean)
    expect(ids).toEqual(['dup'])
  })

  it('slotCount 自定义（例如 3）', () => {
    const sessions = [
      makeSession({ sessionId: 'a', status: 'waiting', updatedAt: NOW }),
      makeSession({ sessionId: 'b', status: 'error', updatedAt: NOW }),
    ]
    const slots = allocateSlots(sessions, { slotCount: 3 })
    expect(slots).toHaveLength(3)
    expect(slots.map((s) => s.i)).toEqual([0, 1, 2])
    expect(slots[0]?.session?.sessionId).toBe('a')
    expect(slots[1]?.session?.sessionId).toBe('b')
    expect(slots[2]?.session).toBeUndefined()
  })

  it('越界 pin 被忽略（不影响分配）', () => {
    const sessions = [
      makeSession({ sessionId: 'a', status: 'waiting', updatedAt: NOW }),
    ]
    const pins = new Map<number, string>([
      [-1, 'a'],
      [99, 'a'],
      [0, 'a'],
    ])
    const slots = allocateSlots(sessions, { pins })
    // 只有 pin=0 生效
    expect(slots[0]?.session?.sessionId).toBe('a')
    expect(slots[0]?.pinned).toBe(true)
    expect(slots.length).toBe(SLOT_COUNT)
  })

  it('pin 占住一个槽后，剩余 sessions 不再占用该槽', () => {
    // pin 一个不存在的 session 到 slot 0（占位）
    const sessions = [
      makeSession({ sessionId: 'real-1', status: 'waiting', updatedAt: NOW }),
      makeSession({ sessionId: 'real-2', status: 'error', updatedAt: NOW }),
    ]
    const pins = new Map<number, string>([[0, 'ghost']])
    const slots = allocateSlots(sessions, { pins })

    // slot 0 留空但 pinned
    expect(slots[0]?.session).toBeUndefined()
    expect(slots[0]?.pinned).toBe(true)
    // 真实会话从 slot 1 开始填
    expect(slots[1]?.session?.sessionId).toBe('real-1')
    expect(slots[2]?.session?.sessionId).toBe('real-2')
  })
})
