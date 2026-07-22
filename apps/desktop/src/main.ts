import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import './style.css'

type DeckStatus = 'off' | 'idle' | 'working' | 'waiting' | 'done' | 'error'
type LedFx = 'solid' | 'breathe' | 'blink_slow' | 'blink_fast'
type BackendId = 'zcode' | 'codex'

interface SlotBinding {
  i: number
  backend?: BackendId
  sessionId?: string
  title?: string
  status: DeckStatus
  detail?: string
  focused?: boolean
  pinned?: boolean
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

interface SessionInfo {
  backend: BackendId
  sessionId: string
  title: string
  status: DeckStatus
  workspacePath?: string
  detail?: string
  updatedAt: number
}

interface SettingsView {
  autoFill: boolean
}

const STATUS_LABEL: Record<DeckStatus, string> = {
  off: 'off',
  idle: 'idle',
  working: 'run',
  waiting: 'wait',
  done: 'done',
  error: 'err',
}

const BACKEND_LABEL: Record<BackendId, string> = {
  zcode: 'ZCode',
  codex: 'Codex',
}

const app = document.querySelector<HTMLDivElement>('#app')!

let latestBoard: BoardState | null = null
let latestLeds: LedFrame | null = null
let settings: SettingsView = { autoFill: false }
let view: 'keyboard' | 'settings' | 'bind' = 'keyboard'
let bindSlot: number | null = null
let bindStep: 'backend' | 'project' | 'session' = 'backend'
let bindBackend: BackendId | null = null
let bindProject: string | null = null
let sessionsCache: SessionInfo[] = []
let longPressTimer: number | null = null
let longPressFired = false

function rgbCss(rgb: [number, number, number] | null, br: number): string {
  if (!rgb) return 'transparent'
  const a = Math.max(0.25, br / 255)
  return `rgba(${rgb[0]}, ${rgb[1]}, ${rgb[2]}, ${a})`
}

function escapeHtml(s: string): string {
  return s
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
}

function projectName(path?: string): string {
  if (!path) return '(unknown project)'
  const parts = path.replace(/\\/g, '/').split('/').filter(Boolean)
  return parts[parts.length - 1] || path
}

function clearLongPress() {
  if (longPressTimer != null) {
    window.clearTimeout(longPressTimer)
    longPressTimer = null
  }
}

async function hideWindow() {
  await invoke('hide_window')
}

async function startDragging() {
  try {
    await invoke('start_dragging')
  } catch (err) {
    console.error('start_dragging failed', err)
  }
}

function wireTitlebarDrag(root: ParentNode = document) {
  root.querySelectorAll<HTMLElement>('[data-tauri-drag-region]').forEach((el) => {
    el.addEventListener('mousedown', (ev) => {
      if (ev.button !== 0) return
      // Don't start drag when clicking buttons inside the titlebar.
      const t = ev.target as HTMLElement | null
      if (t?.closest('button, input, a, .icon-btn, .title-actions')) return
      void startDragging()
    })
  })
}

async function refreshSettings() {
  settings = await invoke<SettingsView>('get_settings')
}

async function openBindPicker(slotI: number) {
  bindSlot = slotI
  bindStep = 'backend'
  bindBackend = null
  bindProject = null
  view = 'bind'
  try {
    sessionsCache = await invoke<SessionInfo[]>('list_sessions')
  } catch (err) {
    sessionsCache = []
    console.error(err)
  }
  paint()
}

async function bindSession(sessionId: string) {
  if (bindSlot == null) return
  await invoke('pin_slot', { i: bindSlot, sessionId })
  view = 'keyboard'
  bindSlot = null
  paint()
}

async function unbindSlot(slotI: number) {
  await invoke('pin_slot', { i: slotI, sessionId: null })
  view = 'keyboard'
  paint()
}

function paintKeyboard(board: BoardState, leds: LedFrame) {
  const keys = board.slots
    .map((slot) => {
      const led = leds.slots.find((s) => s.i === slot.i)
      const rgb = led?.rgb ?? null
      const br = led?.br ?? 0
      const fx = led?.fx ?? 'solid'
      const title = slot.title ?? (slot.sessionId ? slot.sessionId.slice(0, 8) : '空')
      const occupied = !!(slot.sessionId && slot.status !== 'off')
      const focused = slot.focused ? ' focused' : ''
      const empty = occupied ? '' : ' empty'
      const pinned = slot.pinned ? ' pinned' : ''
      const pinBadge = slot.pinned ? '<span class="pin-badge">📌</span>' : ''
      const backend = slot.backend ? BACKEND_LABEL[slot.backend] : ''
      return `
        <div class="key${focused}${empty}${pinned}" data-i="${slot.i}" data-session-id="${slot.sessionId ?? ''}">
          ${pinBadge}
          <div class="key-led fx-${fx}" style="background:${rgbCss(rgb, br)}"></div>
          <div class="key-content">
            <span class="key-label">A${slot.i + 1}${backend ? ' · ' + backend : ''}</span>
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
        <div class="brand" data-tauri-drag-region>Agent Deck</div>
        <div class="title-actions">
          <button class="icon-btn" id="btn-settings" title="设置">⚙</button>
          <button class="icon-btn" id="btn-hide" title="隐藏到托盘">—</button>
        </div>
      </div>
      <div class="resize-hint" aria-hidden="true"></div>
      <div class="row-keys">${keys}</div>
      <div class="row-actions">
        <button class="key-action accept" data-action="accept">OK</button>
        <button class="key-action reject" data-action="reject">NO</button>
        <button class="key-action stop" data-action="stop">STP</button>
      </div>
      <div class="hint">拖标题栏移动 · 点键打开会话 · 长按绑定 · 拖边角缩放 · 左键托盘恢复</div>
    </div>
  `

  wireTitlebarDrag()
  document.getElementById('btn-hide')?.addEventListener('click', (ev) => {
    ev.stopPropagation()
    void hideWindow()
  })
  document.getElementById('btn-settings')?.addEventListener('click', async (ev) => {
    ev.stopPropagation()
    await refreshSettings()
    view = 'settings'
    paint()
  })

  app.querySelectorAll<HTMLElement>('.key[data-i]').forEach((el) => {
    const i = Number(el.dataset.i)

    el.addEventListener('pointerdown', (ev) => {
      if (ev.button !== 0) return
      longPressFired = false
      clearLongPress()
      longPressTimer = window.setTimeout(() => {
        longPressFired = true
        void openBindPicker(i)
      }, 450)
    })
    el.addEventListener('pointerup', () => clearLongPress())
    el.addEventListener('pointerleave', () => clearLongPress())
    el.addEventListener('pointercancel', () => clearLongPress())

    el.addEventListener('click', async (ev) => {
      if (longPressFired) {
        longPressFired = false
        return
      }
      // Cmd/Ctrl+click still toggles pin of current session if any.
      if (ev.metaKey || ev.ctrlKey) {
        const alreadyPinned = el.classList.contains('pinned')
        const sessionId = el.dataset.sessionId || ''
        await invoke('pin_slot', {
          i,
          sessionId: alreadyPinned || !sessionId ? null : sessionId,
        })
        return
      }
      const sessionId = el.dataset.sessionId || ''
      if (!sessionId) {
        // Empty key: open bind picker instead of a no-op focus.
        void openBindPicker(i)
        return
      }
      try {
        await invoke('open_slot_session', { i })
      } catch (err) {
        console.error(err)
        // Fallback: at least focus the slot.
        await invoke('set_focus', { i })
      }
    })

    el.addEventListener('contextmenu', (ev) => {
      ev.preventDefault()
      void openBindPicker(i)
    })
  })

  app.querySelectorAll<HTMLButtonElement>('button[data-action]').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const action = btn.dataset.action!
      await invoke('dispatch_action', { action })
    })
  })
}

function paintSettings() {
  app.innerHTML = `
    <div class="panel-page">
      <div class="titlebar" data-tauri-drag-region>
        <div class="brand" data-tauri-drag-region>设置</div>
        <div class="title-actions">
          <button class="icon-btn" id="btn-back" title="返回">←</button>
          <button class="icon-btn" id="btn-hide" title="隐藏到托盘">—</button>
        </div>
      </div>
      <div class="settings-body">
        <label class="setting-row">
          <div>
            <div class="setting-title">自动填充空槽</div>
            <div class="setting-desc">关闭后只有手动绑定的键会显示会话（推荐）</div>
          </div>
          <input type="checkbox" id="auto-fill" ${settings.autoFill ? 'checked' : ''} />
        </label>
        <div class="setting-help">
          <p><b>绑定会话：</b>在键盘上<strong>长按</strong>任意键（如 A3）→ 选 ZCode/Codex → 选项目 → 选会话。</p>
          <p><b>打开会话：</b>点绑定后的键 → 打开 ZCode 并跳到对应项目（会话栏）。</p>
          <p><b>解绑：</b>在绑定面板点「解绑此键」。</p>
          <p><b>隐藏/恢复：</b>点右上角 — 隐藏到托盘；<strong>左键点菜单栏托盘图标</strong>可再打开。右键托盘出菜单。</p>
          <p><b>移动/缩放：</b>拖标题栏移动；拖窗口边角缩放。</p>
        </div>
      </div>
    </div>
  `
  wireTitlebarDrag()
  document.getElementById('btn-back')?.addEventListener('click', (ev) => {
    ev.stopPropagation()
    view = 'keyboard'
    paint()
  })
  document.getElementById('btn-hide')?.addEventListener('click', (ev) => {
    ev.stopPropagation()
    void hideWindow()
  })
  document.getElementById('auto-fill')?.addEventListener('change', async (ev) => {
    const enabled = (ev.target as HTMLInputElement).checked
    await invoke('set_auto_fill', { enabled })
    settings.autoFill = enabled
  })
}

function paintBind() {
  const slotLabel = bindSlot == null ? '?' : `A${bindSlot + 1}`
  let body = ''

  if (bindStep === 'backend') {
    const backends: BackendId[] = ['zcode', 'codex']
    body = `
      <div class="bind-step">1. 选择工具 · 共 ${sessionsCache.length} 条历史</div>
      <div class="bind-list">
        ${backends
          .map((b) => {
            const list = sessionsCache.filter((s) => s.backend === b)
            const projects = new Set(list.map((s) => s.workspacePath || '(unknown project)'))
            return `<button class="bind-item" data-backend="${b}">
              <span>${BACKEND_LABEL[b]}</span>
              <span class="muted">${projects.size} 项目 · ${list.length} 会话</span>
            </button>`
          })
          .join('')}
      </div>
    `
  } else if (bindStep === 'project' && bindBackend) {
    const byProject = new Map<string, SessionInfo[]>()
    for (const s of sessionsCache.filter((x) => x.backend === bindBackend)) {
      const key = s.workspacePath || '(unknown project)'
      const list = byProject.get(key) ?? []
      list.push(s)
      byProject.set(key, list)
    }
    const projects = [...byProject.entries()].sort((a, b) => {
      const aMax = Math.max(...a[1].map((s) => s.updatedAt))
      const bMax = Math.max(...b[1].map((s) => s.updatedAt))
      return bMax - aMax
    })
    body = `
      <div class="bind-step">2. 选择项目 · ${BACKEND_LABEL[bindBackend]} · ${projects.length} 个</div>
      <div class="bind-list">
        ${
          projects.length
            ? projects
                .map(([p, list]) => {
                  const latest = list
                    .slice()
                    .sort((a, b) => b.updatedAt - a.updatedAt)[0]
                  return `<button class="bind-item" data-project="${escapeHtml(p)}">
                    <span>${escapeHtml(projectName(p))}</span>
                    <div class="bind-meta">
                      <span class="muted">${escapeHtml(p)}</span>
                      <span class="bind-count">${list.length} 会话</span>
                    </div>
                    ${
                      latest
                        ? `<span class="muted">最近：${escapeHtml(latest.title)} · ${STATUS_LABEL[latest.status]}</span>`
                        : ''
                    }
                  </button>`
                })
                .join('')
            : `<div class="empty-hint">没有来自 ${BACKEND_LABEL[bindBackend]} 的会话</div>`
        }
      </div>
    `
  } else if (bindStep === 'session' && bindBackend) {
    const list = sessionsCache
      .filter(
        (s) =>
          s.backend === bindBackend &&
          (s.workspacePath || '(unknown project)') === (bindProject || '(unknown project)'),
      )
      .slice()
      .sort((a, b) => b.updatedAt - a.updatedAt)
    body = `
      <div class="bind-step">3. 选择会话 · ${escapeHtml(projectName(bindProject || undefined))} · ${list.length} 条</div>
      <div class="bind-list">
        ${
          list.length
            ? list
                .map(
                  (s) => `<button class="bind-item" data-session="${escapeHtml(s.sessionId)}">
                    <span>${escapeHtml(s.title)}</span>
                    <span class="muted status-${s.status}">${STATUS_LABEL[s.status]} · ${escapeHtml(s.sessionId.slice(0, 12))}</span>
                  </button>`,
                )
                .join('')
            : `<div class="empty-hint">此项目下没有会话</div>`
        }
      </div>
    `
  }

  app.innerHTML = `
    <div class="panel-page">
      <div class="titlebar" data-tauri-drag-region>
        <div class="brand" data-tauri-drag-region>绑定 ${slotLabel}</div>
        <div class="title-actions">
          <button class="icon-btn" id="btn-back" title="返回">←</button>
          <button class="icon-btn" id="btn-hide" title="隐藏到托盘">—</button>
        </div>
      </div>
      <div class="settings-body">
        ${body}
        <div class="bind-footer">
          <button class="text-btn" id="btn-unbind">解绑此键</button>
        </div>
      </div>
    </div>
  `

  wireTitlebarDrag()
  document.getElementById('btn-back')?.addEventListener('click', (ev) => {
    ev.stopPropagation()
    if (bindStep === 'session') {
      bindStep = 'project'
      bindProject = null
      paint()
      return
    }
    if (bindStep === 'project') {
      bindStep = 'backend'
      bindBackend = null
      paint()
      return
    }
    view = 'keyboard'
    bindSlot = null
    paint()
  })
  document.getElementById('btn-hide')?.addEventListener('click', (ev) => {
    ev.stopPropagation()
    void hideWindow()
  })
  document.getElementById('btn-unbind')?.addEventListener('click', () => {
    if (bindSlot != null) void unbindSlot(bindSlot)
  })

  app.querySelectorAll<HTMLElement>('[data-backend]').forEach((el) => {
    el.addEventListener('click', () => {
      bindBackend = el.dataset.backend as BackendId
      bindStep = 'project'
      paint()
    })
  })
  app.querySelectorAll<HTMLElement>('[data-project]').forEach((el) => {
    el.addEventListener('click', () => {
      bindProject = el.dataset.project || '(unknown project)'
      bindStep = 'session'
      paint()
    })
  })
  app.querySelectorAll<HTMLElement>('[data-session]').forEach((el) => {
    el.addEventListener('click', () => {
      const id = el.dataset.session
      if (id) void bindSession(id)
    })
  })
}

function paint() {
  if (view === 'settings') {
    paintSettings()
    return
  }
  if (view === 'bind') {
    paintBind()
    return
  }
  if (latestBoard && latestLeds) {
    paintKeyboard(latestBoard, latestLeds)
  }
}

async function bootstrap() {
  app.innerHTML = `<div class="keyboard"><div class="loading">连接 host…</div></div>`
  try {
    const [board, leds, st] = await Promise.all([
      invoke<BoardState>('get_board_state'),
      invoke<LedFrame>('get_led_frame'),
      invoke<SettingsView>('get_settings'),
    ])
    latestBoard = board
    latestLeds = leds
    settings = st
    paint()
  } catch (err) {
    app.innerHTML = `<div class="keyboard"><div class="error">启动失败：${escapeHtml(String(err))}</div></div>`
  }

  await listen<{ board: BoardState; leds: LedFrame }>('board-updated', (event) => {
    latestBoard = event.payload.board
    latestLeds = event.payload.leds
    // Don't clobber settings/bind picker on every poll.
    if (view === 'keyboard') paint()
  })
}

bootstrap()
