/**
 * 槽位抢位算法（纯函数）
 *
 * Agent Deck 物理键盘只有 SLOT_COUNT 个 RGB 灯位（默认 5），
 * 但可能有 N 个会话。本模块负责决定「哪几个会话占哪几个槽位」。
 *
 * 算法概要：
 *   1. 初始化 slotCount 个空槽
 *   2. 应用 pins（手动绑定，强制占位）
 *   3. 对剩余会话按 (status 优先级, urgency, updatedAt) 稳定降序排序
 *   4. 过期的 done 会话不占槽
 *   5. 把排序后的会话按顺序填进剩余空槽
 *
 * 本函数为纯函数，输入相同时输出严格确定。
 */

import {
  type DeckStatus,
  type SessionSnapshot,
  STATUS_PRIORITY,
  SLOT_COUNT,
  DONE_TTL_MS,
} from '@agent-deck/protocol'

// ─────────────────────────────────────────────────────────────────────────────
// 类型
// ─────────────────────────────────────────────────────────────────────────────

/**
 * 带 urgency score 的会话快照，urgency 由调用方算好。
 * urgency ∈ [0, 1]，越大越优先占前槽。
 */
export interface ScoredSession extends SessionSnapshot {
  urgency: number
}

export interface SlotAllocatorOptions {
  /** 槽位数，默认 SLOT_COUNT */
  slotCount?: number
  /** 当前焦点槽（不会被挤掉；slotAllocator 仅透传，不影响分配） */
  focus?: number
  /** 钉死的槽位 → sessionId 映射（手动绑定） */
  pins?: Map<number, string>
}

/** 单个槽位的输出 */
export interface AllocatedSlot {
  /** 槽位索引 0..slotCount-1 */
  i: number
  /** 占据该槽的会话；无则 undefined（空槽） */
  session?: ScoredSession
  /** 是否手动绑定（pin） */
  pinned: boolean
}

// ─────────────────────────────────────────────────────────────────────────────
// 内部工具
// ─────────────────────────────────────────────────────────────────────────────

/**
 * 判断会话是否已经「过期」需要从槽位中剔除：
 * 处于 done 状态且 updatedAt 距今已超过 DONE_TTL_MS。
 */
function isExpiredDone(session: ScoredSession, now: number): boolean {
  if (session.status !== 'done') return false
  return now - session.updatedAt > DONE_TTL_MS
}

/**
 * 会话排序比较器：
 *   主键 STATUS_PRIORITY[status] 降序
 *   次键 urgency 降序
 *   三键 updatedAt 降序
 *
 * Array.prototype.sort 在 V8/Node 中已稳定，
 * 同优先级同 urgency 同 updatedAt 时保持输入相对顺序。
 */
function compareSessions(a: ScoredSession, b: ScoredSession): number {
  const pa = STATUS_PRIORITY[a.status as DeckStatus] ?? 0
  const pb = STATUS_PRIORITY[b.status as DeckStatus] ?? 0
  if (pb !== pa) return pb - pa
  if (b.urgency !== a.urgency) return b.urgency - a.urgency
  return b.updatedAt - a.updatedAt
}

// ─────────────────────────────────────────────────────────────────────────────
// 主函数
// ─────────────────────────────────────────────────────────────────────────────

/**
 * 计算 slotCount 个槽位的分配结果。
 *
 * 返回长度恒为 slotCount 的数组，i 从 0 到 slotCount-1。
 */
export function allocateSlots(
  sessions: ScoredSession[],
  opts?: SlotAllocatorOptions,
): AllocatedSlot[] {
  const slotCount = opts?.slotCount ?? SLOT_COUNT
  const focus = opts?.focus
  const pins = opts?.pins
  const now = Date.now()

  // 1. 初始化 slotCount 个空槽
  const slots: AllocatedSlot[] = Array.from(
    { length: slotCount },
    (_, i): AllocatedSlot => ({ i, session: undefined, pinned: false }),
  )

  // 用 Map 做 sessionId -> session 查询（同时去重，后出现覆盖先出现）
  const byId = new Map<string, ScoredSession>()
  for (const s of sessions) {
    if (s && typeof s.sessionId === 'string') {
      byId.set(s.sessionId, s)
    }
  }

  // 已被 pin 占用的 sessionId 集合（这些会话不再参与普通分配）
  const pinnedSessionIds = new Set<string>()

  // 2. 应用 pins：把对应 session 强行塞到指定槽位
  if (pins && pins.size > 0) {
    for (const [slotI, sessionId] of pins) {
      // 越界 pin（不在 0..slotCount-1 范围内）直接忽略，避免越界写入
      if (!Number.isInteger(slotI) || slotI < 0 || slotI >= slotCount) continue

      const target = slots[slotI]
      if (!target) continue

      const session = byId.get(sessionId)
      target.pinned = true
      if (session) {
        target.session = session
        pinnedSessionIds.add(sessionId)
      } else {
        // pin 指向不存在的 session：槽位留空但标记 pinned=true（占位防被占）
        target.session = undefined
      }
    }
  }

  // 3. 对剩余非 pinned 的 sessions 排序，并过滤过期 done
  const remaining: ScoredSession[] = []
  for (const s of byId.values()) {
    if (pinnedSessionIds.has(s.sessionId)) continue
    if (isExpiredDone(s, now)) continue
    remaining.push(s)
  }
  remaining.sort(compareSessions)

  // 4. 按顺序填到剩余空槽（i 从小到大），pinned 槽（即使空）也算占用不可填
  let cursor = 0
  for (const s of remaining) {
    // 找下一个未被占用且未 pinned 的槽
    while (
      cursor < slotCount &&
      (slots[cursor]?.session !== undefined || slots[cursor]?.pinned === true)
    ) {
      cursor++
    }
    if (cursor >= slotCount) break // 槽已满，剩余会话被丢弃
    const slot = slots[cursor]
    if (slot) {
      slot.session = s
      cursor++
    }
  }

  // 5. focus 仅透传，不影响位置（focused 标记由 SessionBoard 上层加）
  // 这里显式引用一下 focus，便于静态分析/未来扩展，且不引入副作用。
  void focus

  return slots
}
