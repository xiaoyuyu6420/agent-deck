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
  working: 'working',
  waiting: 'waiting',
  done: 'done',
  error: 'error',
}

const app = document.querySelector<HTMLDivElement>('#app')!

function rgbCss(rgb: [number, number, number] | null, br: number): string {
  if (!rgb) return 'rgba(40,40,40,0.85)'
  const a = Math.max(0.35, br / 255)
  return `rgba(${rgb[0]}, ${rgb[1]}, ${rgb[2]}, ${a})`
}

function render(board: BoardState, leds: LedFrame) {
  const slots = board.slots
    .map((slot) => {
      const led = leds.slots.find((s) => s.i === slot.i)
      const rgb = led?.rgb ?? null
      const br = led?.br ?? 0
      const fx = led?.fx ?? 'solid'
      const title = slot.title ?? (slot.sessionId ? slot.sessionId.slice(0, 12) : 'empty')
      const detail = slot.detail ? `<div class="detail">${escapeHtml(slot.detail)}</div>` : ''
      const focused = slot.focused ? ' focused' : ''
      return `
        <div class="slot${focused}" data-i="${slot.i}">
          <div class="led fx-${fx}" style="background:${rgbCss(rgb, br)}"></div>
          <div class="meta">
            <div class="row">
              <span class="idx">A${slot.i + 1}</span>
              <span class="status status-${slot.status}">${STATUS_LABEL[slot.status]}</span>
            </div>
            <div class="title">${escapeHtml(title)}</div>
            ${detail}
          </div>
        </div>
      `
    })
    .join('')

  app.innerHTML = `
    <div class="panel">
      <div class="titlebar" data-tauri-drag-region>
        <div class="brand">Agent Deck</div>
        <div class="mode">${escapeHtml(board.mode)}</div>
      </div>
      <div class="slots">${slots}</div>
      <div class="actions">
        <button data-action="accept">Accept</button>
        <button data-action="reject">Reject</button>
        <button data-action="stop">Stop</button>
      </div>
      <div class="hint">拖动标题栏移动 · 关窗后仍在托盘</div>
    </div>
  `

  app.querySelectorAll<HTMLElement>('.slot').forEach((el) => {
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
  app.innerHTML = `<div class="panel loading">连接 host…</div>`
  try {
    const [board, leds] = await Promise.all([
      invoke<BoardState>('get_board_state'),
      invoke<LedFrame>('get_led_frame'),
    ])
    render(board, leds)
  } catch (err) {
    app.innerHTML = `<div class="panel error">启动失败：${escapeHtml(String(err))}</div>`
  }

  await listen<{ board: BoardState; leds: LedFrame }>('board-updated', (event) => {
    render(event.payload.board, event.payload.leds)
  })
}

bootstrap()
