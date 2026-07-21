/**
 * 灯效主题映射（纯函数）
 *
 * Agent Deck 把每个会话状态映射成 RGB / 亮度 / 灯效。
 * 本模块是状态 → 灯指令的唯一真相源，被 SessionBoard 调用。
 *
 * 设计原则：
 *   - 纯函数：相同输入严格相同输出，无副作用、无时钟依赖
 *     （now 由调用方传入，便于测试与回放）
 *   - 调色板可替换：默认对齐 Codex Micro 官方色，但允许自定义
 *   - 状态语义对齐 @agent-deck/protocol 的 DeckStatus
 *
 * 状态 → 灯 效果速查：
 *   off      关灯
 *   idle     白色 dim，solid
 *   working  蓝色 breathe；长跑时偏紫提示可能卡住
 *   waiting  橙色，随 urgency 偏红 + 加速 blink
 *   done     绿色 solid
 *   error    红色 solid
 */

import {
  type DeckStatus,
  type Risk,
  type LedFx,
  RISK_BOOST,
  URGENCY_FULL_WAIT_MS,
  WORKING_LONG_MS,
} from '@agent-deck/protocol'

// ─────────────────────────────────────────────────────────────────────────────
// 类型
// ─────────────────────────────────────────────────────────────────────────────

/** paint 的输入。now / status 必填，其余可选。 */
export interface ThemeInput {
  status: DeckStatus
  /** 风险等级，影响 waiting 的 urgency 下限 */
  risk?: Risk
  /** 进入当前状态的时间戳（ms），用于 age 计算 */
  waitingSince?: number
  /** 当前时间戳（ms），由调用方传入 */
  now: number
}

/**
 * paint 的输出，与 protocol 的 LedSlot 一致但去掉 i 字段
 * （槽位索引由上层 slotAllocator 决定，主题只负责"画"一盏灯）。
 */
export interface ThemeOutput {
  /** RGB 0-255 三元组，或 null 关灯 */
  rgb: [number, number, number] | null
  /** 亮度 0-255 */
  br: number
  fx: LedFx
}

/** 主题调色板（hex 字符串）。waiting 为基色，实际颜色随 urgency 偏红。 */
export interface ThemePalette {
  off: string
  idle: string
  working: string
  waiting: string
  done: string
  error: string
}

// ─────────────────────────────────────────────────────────────────────────────
// 默认主题（对齐 Codex Micro 官方色）
// ─────────────────────────────────────────────────────────────────────────────

export const CODEX_THEME: ThemePalette = {
  off: '#000000',
  idle: '#FFFFFF',
  working: '#304FFE',
  waiting: '#FF6D00',
  done: '#00FF4C',
  error: '#FF0033',
}

// ─────────────────────────────────────────────────────────────────────────────
// 工具函数
// ─────────────────────────────────────────────────────────────────────────────

/** 把 x 限制到 [0, 1] */
export function clamp01(x: number): number {
  if (Number.isNaN(x)) return 0
  if (x < 0) return 0
  if (x > 1) return 1
  return x
}

/** 线性插值：a + (b - a) * t，t 不做 clamp（由调用方决定是否需要） */
export function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t
}

/**
 * hex 字符串 → RGB 三元组。
 * 支持 '#RGB' 与 '#RRGGBB' 两种形式，大小写不敏感。
 *
 * 例：hexToRgb('#FF6D00') → [255, 109, 0]
 */
export function hexToRgb(hex: string): [number, number, number] {
  let h = hex.trim()
  if (h.startsWith('#')) h = h.slice(1)
  // 短格式 #RGB → #RRGGBB
  if (h.length === 3) {
    h = h
      .split('')
      .map((c) => c + c)
      .join('')
  }
  if (h.length !== 6) {
    throw new Error(`hexToRgb: invalid hex "${hex}"`)
  }
  const r = Number.parseInt(h.slice(0, 2), 16)
  const g = Number.parseInt(h.slice(2, 4), 16)
  const b = Number.parseInt(h.slice(4, 6), 16)
  if (Number.isNaN(r) || Number.isNaN(g) || Number.isNaN(b)) {
    throw new Error(`hexToRgb: invalid hex "${hex}"`)
  }
  return [r, g, b]
}

/**
 * 两个 hex 颜色之间做线性插值，分别对 r/g/b 三通道 lerp 后四舍五入到整数。
 * t 不做 clamp（由调用方决定），但 t=0 返回 a，t=1 返回 b。
 */
export function lerpHex(a: string, b: string, t: number): [number, number, number] {
  const ca = hexToRgb(a)
  const cb = hexToRgb(b)
  return [
    Math.round(lerp(ca[0], cb[0], t)),
    Math.round(lerp(ca[1], cb[1], t)),
    Math.round(lerp(ca[2], cb[2], t)),
  ]
}

/**
 * 从 RISK_BOOST 安全取值（兼容 noUncheckedIndexedAccess）。
 * risk 缺省按 'low' 处理；未知值兜底为 0。
 */
function riskBoost(risk: Risk | undefined): number {
  const key = risk ?? 'low'
  return RISK_BOOST[key] ?? 0
}

// ─────────────────────────────────────────────────────────────────────────────
// 主函数
// ─────────────────────────────────────────────────────────────────────────────

/**
 * 把会话状态映射成单盏灯的指令（rgb / 亮度 / 灯效）。
 *
 * 规则见文件顶部注释。palette 缺省为 CODEX_THEME。
 */
export function paint(
  input: ThemeInput,
  palette: ThemePalette = CODEX_THEME,
): ThemeOutput {
  switch (input.status) {
    case 'off':
      return { rgb: null, br: 0, fx: 'solid' }

    case 'idle':
      return { rgb: hexToRgb(palette.idle), br: 60, fx: 'solid' }

    case 'done':
      return { rgb: hexToRgb(palette.done), br: 255, fx: 'solid' }

    case 'error':
      return { rgb: hexToRgb(palette.error), br: 255, fx: 'solid' }

    case 'working': {
      // 没有 waitingSince：纯 working 色
      if (input.waitingSince === undefined) {
        return { rgb: hexToRgb(palette.working), br: 180, fx: 'breathe' }
      }
      // 有 waitingSince：随长跑时长从 working 色偏紫（提示可能卡住）
      const ageSec = (input.now - input.waitingSince) / 1000
      const longRun = clamp01(ageSec / (WORKING_LONG_MS / 1000))
      const rgb = lerpHex(palette.working, '#7B1FA2', longRun)
      return { rgb, br: 180, fx: 'breathe' }
    }

    case 'waiting': {
      // ageSec：没有 waitingSince 则视为 0（刚进入 waiting）
      const ageSec =
        input.waitingSince === undefined ? 0 : (input.now - input.waitingSince) / 1000
      // timeUrgency：等待时长占满程的比例，clamp 到 [0,1]
      const timeUrgency = clamp01(ageSec / (URGENCY_FULL_WAIT_MS / 1000))
      // u = max(timeUrgency, riskBoost)：风险等级抬高 urgency 下限
      const u = Math.max(timeUrgency, riskBoost(input.risk))
      // 颜色：浅橙 → 急红
      const rgb = lerpHex('#FFB074', '#FF2200', u)
      // 亮度：随 urgency 从 80 抬到 255
      const br = Math.round(lerp(80, 255, u))
      // 灯效：越急越快
      const fx: LedFx = u < 0.33 ? 'solid' : u < 0.66 ? 'blink_slow' : 'blink_fast'
      return { rgb, br, fx }
    }

    default: {
      // 穷尽性兜底：DeckStatus 只有上述 6 种，走到这里说明类型被外力破坏。
      // 退化为关灯，保证设备永远收到合法帧。
      const _exhaustive: never = input.status
      void _exhaustive
      return { rgb: null, br: 0, fx: 'solid' }
    }
  }
}
