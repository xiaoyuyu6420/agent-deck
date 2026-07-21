import { describe, it, expect } from 'vitest'

import {
  paint,
  hexToRgb,
  lerp,
  lerpHex,
  clamp01,
  CODEX_THEME as THEME,
} from '../src/board/theme.js'

// ─────────────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────────────

/** 颜色近似相等（逐通道允许 ±tol） */
function rgbCloseTo(
  actual: [number, number, number] | null,
  expected: [number, number, number],
  tol = 1,
): boolean {
  if (actual === null) return false
  return actual.every((c, i) => Math.abs(c - expected[i]) <= tol)
}

// ─────────────────────────────────────────────────────────────────────────────
// 工具函数
// ─────────────────────────────────────────────────────────────────────────────

describe('hexToRgb', () => {
  it('11. #FF6D00 → [255, 109, 0]', () => {
    expect(hexToRgb('#FF6D00')).toEqual([255, 109, 0])
  })

  it('短格式 #F00 → [255, 0, 0]', () => {
    expect(hexToRgb('#F00')).toEqual([255, 0, 0])
  })

  it('大小写不敏感', () => {
    expect(hexToRgb('#ff6d00')).toEqual([255, 109, 0])
  })

  it('非法输入抛错', () => {
    expect(() => hexToRgb('#GGGGGG')).toThrow()
    expect(() => hexToRgb('#12')).toThrow()
  })
})

describe('clamp01', () => {
  it('把数值限制到 [0,1]', () => {
    expect(clamp01(-1)).toBe(0)
    expect(clamp01(0)).toBe(0)
    expect(clamp01(0.5)).toBe(0.5)
    expect(clamp01(1)).toBe(1)
    expect(clamp01(2)).toBe(1)
  })

  it('NaN 兜底为 0', () => {
    expect(clamp01(Number.NaN)).toBe(0)
  })
})

describe('lerp', () => {
  it('端点正确', () => {
    expect(lerp(0, 100, 0)).toBe(0)
    expect(lerp(0, 100, 1)).toBe(100)
    expect(lerp(0, 100, 0.5)).toBe(50)
  })
})

describe('lerpHex', () => {
  it('12. lerpHex(#000000, #FFFFFF, 0.5) ≈ [127,127,127] (±1)', () => {
    const r = lerpHex('#000000', '#FFFFFF', 0.5)
    expect(r[0]).toBeGreaterThanOrEqual(126)
    expect(r[0]).toBeLessThanOrEqual(128)
    expect(r[1]).toBeGreaterThanOrEqual(126)
    expect(r[1]).toBeLessThanOrEqual(128)
    expect(r[2]).toBeGreaterThanOrEqual(126)
    expect(r[2]).toBeLessThanOrEqual(128)
  })

  it('13. lerpHex(#FFB074, #FF2200, 0) === hexToRgb(#FFB074)', () => {
    expect(lerpHex('#FFB074', '#FF2200', 0)).toEqual(hexToRgb('#FFB074'))
  })

  it('14. lerpHex(#FFB074, #FF2200, 1) === hexToRgb(#FF2200)', () => {
    expect(lerpHex('#FFB074', '#FF2200', 1)).toEqual(hexToRgb('#FF2200'))
  })

  it('中间值逐通道独立 lerp', () => {
    // t=0.5: R=(255+255)/2=255, G=(176+34)/2=105, B=(116+0)/2=58
    expect(lerpHex('#FFB074', '#FF2200', 0.5)).toEqual([255, 105, 58])
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// paint —— 静态状态
// ─────────────────────────────────────────────────────────────────────────────

describe('paint — 静态状态', () => {
  it('1. off → rgb=null, br=0, fx=solid', () => {
    const out = paint({ status: 'off', now: 0 })
    expect(out.rgb).toBeNull()
    expect(out.br).toBe(0)
    expect(out.fx).toBe('solid')
  })

  it('2. idle → 白色, br≈60, fx=solid', () => {
    const out = paint({ status: 'idle', now: 0 })
    expect(out.rgb).toEqual([255, 255, 255])
    expect(out.br).toBe(60)
    expect(out.fx).toBe('solid')
  })

  it('9. done → 绿色, br=255, fx=solid', () => {
    const out = paint({ status: 'done', now: 0 })
    expect(out.rgb).toEqual(hexToRgb(THEME.done))
    expect(out.br).toBe(255)
    expect(out.fx).toBe('solid')
  })

  it('10. error → 红色, br=255, fx=solid', () => {
    const out = paint({ status: 'error', now: 0 })
    expect(out.rgb).toEqual(hexToRgb(THEME.error))
    expect(out.br).toBe(255)
    expect(out.fx).toBe('solid')
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// paint —— working
// ─────────────────────────────────────────────────────────────────────────────

describe('paint — working', () => {
  it('3. working 无 longRun → 纯蓝, br=180, fx=breathe', () => {
    const out = paint({
      status: 'working',
      now: 1000,
      waitingSince: 1000, // age=0
    })
    expect(out.rgb).toEqual(hexToRgb(THEME.working))
    expect(out.br).toBe(180)
    expect(out.fx).toBe('breathe')
  })

  it('working 没有 waitingSince → 纯蓝 breathe', () => {
    const out = paint({ status: 'working', now: 1000 })
    expect(out.rgb).toEqual(hexToRgb(THEME.working))
    expect(out.br).toBe(180)
    expect(out.fx).toBe('breathe')
  })

  it('4. working 长跑 6 分钟 → 偏紫（rgb ≠ 纯蓝）', () => {
    const out = paint({
      status: 'working',
      now: 1000,
      waitingSince: 1000 - 6 * 60 * 1000, // age = 6 min > WORKING_LONG_MS(5min)
    })
    const pureBlue = hexToRgb(THEME.working)
    // 应已偏移：至少一个通道变化超过阈值
    expect(out.rgb).not.toEqual(pureBlue)
    // longRun 被 clamp 到 1，应等于纯紫 #7B1FA2
    expect(out.rgb).toEqual(hexToRgb('#7B1FA2'))
    expect(out.br).toBe(180)
    expect(out.fx).toBe('breathe')
  })

  it('working 中等时长（2.5min, longRun=0.5）→ 蓝紫之间', () => {
    const out = paint({
      status: 'working',
      now: 0,
      waitingSince: -(2.5 * 60 * 1000), // age = 2.5min = WORKING_LONG_MS/2
    })
    const expected = lerpHex(THEME.working, '#7B1FA2', 0.5)
    expect(out.rgb).toEqual(expected)
    expect(out.fx).toBe('breathe')
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// paint —— waiting（关键路径）
// ─────────────────────────────────────────────────────────────────────────────

describe('paint — waiting', () => {
  it('5. waiting risk=low age=0 → 浅橙 (u=0), fx=solid, br=80', () => {
    const out = paint({
      status: 'waiting',
      risk: 'low',
      waitingSince: 0,
      now: 0,
    })
    // u = max(0, 0) = 0 → 浅橙端点
    expect(out.rgb).toEqual(hexToRgb('#FFB074'))
    expect(out.fx).toBe('solid')
    expect(out.br).toBe(80) // lerp(80,255,0)
  })

  it('6. waiting risk=low age=60s → 中等橙 (u=0.5), fx=blink_slow', () => {
    const out = paint({
      status: 'waiting',
      risk: 'low',
      waitingSince: 0,
      now: 60 * 1000, // age=60s, timeUrgency=60/120=0.5
    })
    // u = max(0.5, 0) = 0.5
    expect(out.fx).toBe('blink_slow')
    // 颜色 = lerpHex(#FFB074, #FF2200, 0.5) = [255,105,58]
    expect(out.rgb).toEqual([255, 105, 58])
    // 亮度 = round(lerp(80,255,0.5)) = round(167.5) = 168
    expect(out.br).toBe(168)
  })

  it('7. waiting risk=low age=120s → 急红 (u=1), fx=blink_fast, br=255', () => {
    const out = paint({
      status: 'waiting',
      risk: 'low',
      waitingSince: 0,
      now: 2 * 60 * 1000, // age=120s, timeUrgency=1
    })
    // u = max(1, 0) = 1 → 急红端点
    expect(out.fx).toBe('blink_fast')
    expect(out.rgb).toEqual(hexToRgb('#FF2200'))
    expect(out.br).toBe(255)
  })

  it('8. waiting risk=high age=0 → u=0.5（risk 抬升）, fx=blink_slow', () => {
    const out = paint({
      status: 'waiting',
      risk: 'high',
      waitingSince: 0,
      now: 0,
    })
    // u = max(0, 0.5) = 0.5
    expect(out.fx).toBe('blink_slow')
    expect(out.rgb).toEqual([255, 105, 58])
    expect(out.br).toBe(168)
  })

  it('waiting risk=medium → u 下限 0.25，fx=solid', () => {
    const out = paint({
      status: 'waiting',
      risk: 'medium',
      waitingSince: 0,
      now: 0,
    })
    // u = max(0, 0.25) = 0.25 < 0.33 → solid
    expect(out.fx).toBe('solid')
    expect(out.rgb).toEqual(lerpHex('#FFB074', '#FF2200', 0.25))
  })

  it('waiting 无 waitingSince → age 按 0 处理', () => {
    const out = paint({
      status: 'waiting',
      risk: 'low',
      now: 999999,
      // 故意不传 waitingSince
    })
    expect(out.fx).toBe('solid')
    expect(out.rgb).toEqual(hexToRgb('#FFB074'))
  })

  it('waiting 边界 u≥0.33 → blink_slow（半开区间）', () => {
    // 选 u 明确 > 0.33 但 < 0.66 的点：age = 48s → timeUrgency = 0.4
    const out = paint({
      status: 'waiting',
      risk: 'low',
      waitingSince: 0,
      now: 48 * 1000,
    })
    // u = 0.4 → 不再 solid，进入 blink_slow
    expect(out.fx).toBe('blink_slow')
  })

  it('waiting age 超过满程 → timeUrgency clamp 到 1', () => {
    const out = paint({
      status: 'waiting',
      risk: 'low',
      waitingSince: 0,
      now: 10 * 60 * 1000, // 10 min, 远超 2min
    })
    expect(out.fx).toBe('blink_fast')
    expect(out.rgb).toEqual(hexToRgb('#FF2200'))
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// paint —— palette 自定义
// ─────────────────────────────────────────────────────────────────────────────

describe('paint — 自定义 palette', () => {
  it('自定义 palette 影响静态状态颜色', () => {
    const palette = {
      ...THEME,
      idle: '#00FFFF',
      error: '#123456',
    }
    const idle = paint({ status: 'idle', now: 0 }, palette)
    expect(idle.rgb).toEqual(hexToRgb('#00FFFF'))

    const err = paint({ status: 'error', now: 0 }, palette)
    expect(err.rgb).toEqual(hexToRgb('#123456'))
  })

  it('自定义 working 色 → working 状态使用新色', () => {
    const palette = { ...THEME, working: '#AABBCC' }
    const out = paint({ status: 'working', now: 0 }, palette)
    expect(out.rgb).toEqual(hexToRgb('#AABBCC'))
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// 回归 / 不变性
// ─────────────────────────────────────────────────────────────────────────────

describe('paint — 不变性', () => {
  it('相同输入产生相同输出', () => {
    const input = {
      status: 'waiting' as const,
      risk: 'medium' as const,
      waitingSince: 1000,
      now: 70000,
    }
    const a = paint(input)
    const b = paint(input)
    expect(a).toEqual(b)
  })

  it('所有状态的 br 都在 [0, 255]', () => {
    const statuses = [
      'off',
      'idle',
      'working',
      'waiting',
      'done',
      'error',
    ] as const
    for (const status of statuses) {
      const out = paint({ status, now: 100000, waitingSince: 0 })
      expect(out.br).toBeGreaterThanOrEqual(0)
      expect(out.br).toBeLessThanOrEqual(255)
    }
  })

  it('所有 rgb 非 null 时各通道在 [0,255]', () => {
    const statuses = [
      'off',
      'idle',
      'working',
      'waiting',
      'done',
      'error',
    ] as const
    for (const status of statuses) {
      const out = paint({ status, now: 100000, waitingSince: 0 })
      if (out.rgb !== null) {
        for (const c of out.rgb) {
          expect(c).toBeGreaterThanOrEqual(0)
          expect(c).toBeLessThanOrEqual(255)
        }
      }
    }
  })

  it('off 永远是 rgb=null / br=0，与 waitingSince 无关', () => {
    const out = paint({ status: 'off', now: 0, waitingSince: -999999 })
    expect(out.rgb).toBeNull()
    expect(out.br).toBe(0)
    expect(rgbCloseTo([0, 0, 0], [0, 0, 0])).toBe(true) // sanity helper
  })
})
