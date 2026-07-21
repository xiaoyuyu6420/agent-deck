/**
 * @agent-deck/protocol
 *
 * host / simulator / firmware 三方共用的唯一真相源。
 * 任何跨进程/跨设备的数据结构都在这里定义。
 */

import { z } from 'zod'

// ─────────────────────────────────────────────────────────────────────────────
// 后端标识
// ─────────────────────────────────────────────────────────────────────────────

export const BACKENDS = ['zcode', 'codex'] as const
export type BackendId = (typeof BACKENDS)[number]

export const backendIdSchema = z.enum(BACKENDS)

// ─────────────────────────────────────────────────────────────────────────────
// 状态模型
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Deck 状态枚举（与 Codex Micro 官方语义对齐）
 *
 * 优先级：waiting > error > working > done(recent) > idle > off
 */
export const DECK_STATUSES = [
  'off',
  'idle',
  'working',
  'waiting',
  'done',
  'error',
] as const
export type DeckStatus = (typeof DECK_STATUSES)[number]

export const deckStatusSchema = z.enum(DECK_STATUSES)

/**
 * 状态优先级数值（越大越优先占前槽）
 */
export const STATUS_PRIORITY: Record<DeckStatus, number> = {
  waiting: 5,
  error: 4,
  working: 3,
  done: 2,
  idle: 1,
  off: 0,
}

/**
 * 风险等级（影响 urgency 下限）
 */
export const RISKS = ['low', 'medium', 'high'] as const
export type Risk = (typeof RISKS)[number]
export const riskSchema = z.enum(RISKS)

export const RISK_BOOST: Record<Risk, number> = {
  low: 0,
  medium: 0.25,
  high: 0.5,
}

// ─────────────────────────────────────────────────────────────────────────────
// Session 快照（观察层输出，Board 消费）
// ─────────────────────────────────────────────────────────────────────────────

export interface SessionSnapshot {
  backend: BackendId
  sessionId: string
  title: string
  status: DeckStatus
  risk?: Risk
  /** 如 "Bash: git push"，用于 UI 展示 */
  detail?: string
  /** 进入 waiting 的时间戳（ms），用于 urgency 计算 */
  waitingSince?: number
  /** 最后更新时间（ms） */
  updatedAt: number
  /** 工作区路径（用于排除自指） */
  workspacePath?: string
}

export const sessionSnapshotSchema = z.object({
  backend: backendIdSchema,
  sessionId: z.string(),
  title: z.string(),
  status: deckStatusSchema,
  risk: riskSchema.optional(),
  detail: z.string().optional(),
  waitingSince: z.number().optional(),
  updatedAt: z.number(),
  workspacePath: z.string().optional(),
})

// ─────────────────────────────────────────────────────────────────────────────
// LED 帧（Board 输出，device/simulator 消费）
// ─────────────────────────────────────────────────────────────────────────────

export const LED_FX = ['solid', 'breathe', 'blink_slow', 'blink_fast'] as const
export type LedFx = (typeof LED_FX)[number]
export const ledFxSchema = z.enum(LED_FX)

/** 单个槽位的灯指令。rgb=null 表示关灯 */
export interface LedSlot {
  /** 槽位索引 0..N-1 */
  i: number
  /** RGB 0-255 三元组，或 null 关灯 */
  rgb: [number, number, number] | null
  /** 亮度 0-255 */
  br: number
  fx: LedFx
}

export interface LedFrame {
  type: 'leds'
  slots: LedSlot[]
}

export const ledSlotSchema = z.object({
  i: z.number().int().min(0),
  rgb: z.union([z.tuple([z.number(), z.number(), z.number()]), z.null()]),
  br: z.number().min(0).max(255),
  fx: ledFxSchema,
})

export const ledFrameSchema = z.object({
  type: z.literal('leds'),
  slots: z.array(ledSlotSchema),
})

// ─────────────────────────────────────────────────────────────────────────────
// Board 完整状态（host → client 推送）
// ─────────────────────────────────────────────────────────────────────────────

/** 单个槽位绑定信息（给 simulator 显示文字用） */
export interface SlotBinding {
  i: number
  backend?: BackendId
  sessionId?: string
  title?: string
  status: DeckStatus
  detail?: string
  /** 焦点槽（用户当前关注） */
  focused?: boolean
}

export interface BoardState {
  type: 'board'
  slots: SlotBinding[]
  /** 当前焦点槽 */
  focus: number
  /** 当前策略模式 */
  mode: PolicyMode
}

// ─────────────────────────────────────────────────────────────────────────────
// 策略模式
// ─────────────────────────────────────────────────────────────────────────────

export const POLICY_MODES = ['plan', 'act', 'review'] as const
export type PolicyMode = (typeof POLICY_MODES)[number]
export const policyModeSchema = z.enum(POLICY_MODES)

// ─────────────────────────────────────────────────────────────────────────────
// 动作（client → host）
// ─────────────────────────────────────────────────────────────────────────────

export type Action =
  | { op: 'focus'; i: number }
  | { op: 'accept'; i?: number }
  | { op: 'reject'; i?: number }
  | { op: 'stop'; i?: number }
  | { op: 'stop_all' }
  | { op: 'freeze_all' }
  | { op: 'unfreeze' }
  | { op: 'set_mode'; mode: PolicyMode }
  | { op: 'send'; i?: number; text: string }

// ─────────────────────────────────────────────────────────────────────────────
// WS / USB 消息（wire format）
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Host → Client 推送的消息
 */
export type ServerMessage =
  | LedFrame
  | BoardState
  | { type: 'focus'; i: number }
  | { type: 'event'; event: string; data?: unknown }

/**
 * Client → Host 发送的消息（动作）
 */
export type ClientMessage =
  | { t: 'action'; action: Action }
  | { t: 'key'; id: string; edge: 'down' | 'up' }
  | { t: 'enc'; delta: number }
  | { t: 'joy'; dir: 'up' | 'down' | 'left' | 'right' | 'center'; edge: 'down' | 'up' }
  | { t: 'voice'; op: 'ptt'; edge: 'down' | 'up' }

// ─────────────────────────────────────────────────────────────────────────────
// USB CDC JSON Lines（device ↔ host）
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Host → Device（推灯帧）
 * 例：{"t":"leds","slots":[{"i":0,"rgb":[255,109,0],"br":255,"fx":"blink_fast"}]}
 */
export type DeviceInbound =
  | {
      t: 'leds'
      slots: Array<{
        i: number
        rgb: [number, number, number] | null
        br: number
        fx: LedFx
      }>
    }
  | { t: 'focus'; i: number }

/**
 * Device → Host（按键事件）
 * 例：{"t":"key","id":"a1","edge":"down"}
 */
export type DeviceOutbound =
  | { t: 'key'; id: string; edge: 'down' | 'up'; fn?: boolean }
  | { t: 'enc'; delta: number }
  | {
      t: 'joy'
      dir: 'up' | 'down' | 'left' | 'right' | 'center'
      edge: 'down' | 'up'
    }
  | { t: 'ptt'; edge: 'down' | 'up' }

// ─────────────────────────────────────────────────────────────────────────────
// 槽位配置
// ─────────────────────────────────────────────────────────────────────────────

/** V1 槽位总数（100×100mm 物理塞键上限） */
export const SLOT_COUNT = 5

/** 历史完成态保留时长（ms），超过则降为 idle */
export const DONE_TTL_MS = 5 * 60 * 1000

/** urgency 拉满的等待时长（ms） */
export const URGENCY_FULL_WAIT_MS = 2 * 60 * 1000

/** working 长跑阈值（ms），超过偏紫提示可能卡住 */
export const WORKING_LONG_MS = 5 * 60 * 1000
