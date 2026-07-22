import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import './style.css'

type DeckStatus = 'off' | 'idle' | 'working' | 'waiting' | 'done' | 'error'
type LedFx = 'solid' | 'breathe' | 'blink_slow' | 'blink_fast'

interface SlotBinding {
  i: number
  backend?: string
  sessionId?: string
  title?: string
  status: DeckStatus
  detail?: string
  focused?: boolean
}

interface LedSlot {
  i: number
  rgb: [number, number, number] | null
  br: number
  fx: LedFx
}

interface BoardState {
  type: 'board'
  slots: SlotBinding[]
  focus: number
  mode: string
}

interface LedFrame {
  type: 'leds'
  slots: LedSlot[]
}

const STATUS_LABEL: Record<DeckStatus, string> = {
  off: 'off',
  idle: 'idle',
  working: 'run',
  waiting: 'wait',
  done: 'done',
  error: 'err',
}

const app = document.querySelector<HTMLDivElement>('#app')!

/** LED rgb/brightness → rgba background, shared with the physical device palette. */
function rgbCss(rgb: [number, number, number] | null, br: number): string {
  if (!rgb) return 'transparent'
  const a = Math.max(0.25, br / 255)
  return `rgba(${rgb[0]}, ${rgb[1]}, ${rgb[2]}, ${a})`
}

/**
 * Render the virtual keyboard.
 *
 * Row 0 = status keys (A1..An), one per software slot; the keycap glows with
 * the slot's LED colour/animation like the physical A1-A5 keys.
 * Row 1 = action keys OK / NO / STP (the V1-soldered subset of the hardware
 * action row OK/NO/STOP/NEW/PTT).
 */
function render(board: BoardState, leds: LedFrame) {
  const keys = board.slots
    .map((slot) => {
      const led = leds.slots.find((s) => s.i === slot.i)
      const rgb = led?.rgb ?? null
      const br = led?.br ?? 0
      const fx = led?.fx ?? 'solid'
      const title = slot.title ?? (slot.sessionId ? slot.sessionId.slice(0, 8) : '—')
      const occupied = slot.status !== 'off' && slot.sessionId
      const focused = slot.focused ? ' focused' : ''
      const empty = occupied ? '' : ' empty'
      // The LED glow fills the keycap; the key IS the LED, as on the hardware.
      return `
        <div class="key${focused}${empty}" data-i="${slot.i}">
          <div class="key-led fx-${fx}" style="background:${rgbCss(rgb, br)}"></div>
          <div class="key-content">
            <span class="key-label">A${slot.i + 1}</span>
            <span class="key-title">${escapeHtml(title)}</span>
            <span class="key-status status-${slot.status}">${STATUS_LABEL[slot.status]}</span>
          </div>
        </div>
      `
    })
    .join('')

  app.innerHTML = `
    <div class="keyboard">
      <div class="titlebar" data-tauri-drag-region>
        <div class="brand">Agent Deck</div>
        <div class="mode">${escapeHtml(board.mode)}</div>
      </div>
      <div class="row-keys">${keys}</div>
      <div class="row-actions">
        <button class="key-action accept" data-action="accept">OK</button>
        <button class="key-action reject" data-action="reject">NO</button>
        <button class="key-action stop" data-action="stop">STP</button>
      </div>
      <div class="hint">拖动标题栏移动 · 点状态键聚焦 · 关窗后仍在托盘</div>
    </div>
  `

  app.querySelectorAll<HTMLElement>('.key[data-i]').forEach((el) => {
    el.addEventListener('click', async () => {
      const i = Number(el.dataset.i)
      await invoke('set_focus', { i })
    })
  })

  app.querySelectorAll<HTMLButtonElement>('button[data-action]').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const action = btn.dataset.action!
      await invoke('dispatch_action', { action })
    })
  })
}

function escapeHtml(s: string): string {
  return s
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
}

async function bootstrap() {
  app.innerHTML = `<div class="keyboard"><div class="loading">连接 host…</div></div>`
  try {
    const [board, leds] = await Promise.all([
      invoke<BoardState>('get_board_state'),
      invoke<LedFrame>('get_led_frame'),
    ])
    render(board, leds)
  } catch (err) {
    app.innerHTML = `<div class="keyboard"><div class="error">启动失败：${escapeHtml(String(err))}</div></div>`
  }

  await listen<{ board: BoardState; leds: LedFrame }>('board-updated', (event) => {
    render(event.payload.board, event.payload.leds)
  })
}

bootstrap()
