import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import './style.css'

type DeckStatus = 'off' | 'idle' | 'working' | 'waiting' | 'done' | 'error'
type LedFx = 'solid' | 'breathe' | 'blink_slow' | 'blink_fast'
type BackendId = 'zcode' | 'codex' | 'workbuddy'

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

type ProjectCategory = 'project' | 'task' | 'automation'

interface SessionInfo {
  backend: BackendId
  sessionId: string
  title: string
  status: DeckStatus
  workspacePath?: string
  detail?: string
  updatedAt: number
  /** WorkBuddy bind-picker section; other backends usually omit. */
  projectCategory?: ProjectCategory
  /** Human label for the group row (task title / automation name / folder). */
  projectLabel?: string
}

interface SettingsView {
  autoFill: boolean
  /** Keep Done green this long after the user opens the key (ms). */
  doneTtlAfterOpenMs: number
  /** Force Idle if Done is never opened (ms). */
  doneTtlUnopenedMs: number
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
  workbuddy: 'WorkBuddy',
}

const app = document.querySelector<HTMLDivElement>('#app')!

let latestBoard: BoardState | null = null
let latestLeds: LedFrame | null = null
let settings: SettingsView = {
  autoFill: false,
  doneTtlAfterOpenMs: 5 * 60 * 1000,
  doneTtlUnopenedMs: 12 * 60 * 60 * 1000,
}
let view: 'keyboard' | 'settings' | 'bind' = 'keyboard'
let bindSlot: number | null = null
let bindStep: 'backend' | 'project' | 'session' = 'backend'
let bindBackend: BackendId | null = null
let bindProject: string | null = null
let sessionsCache: SessionInfo[] = []
let longPressTimer: number | null = null
let longPressFired = false

/**
 * Build the LED glow background for a key.
 *
 * The theme layer (theme.ts) emits semantic colors + brightness that are also
 * the truth source for real hardware LED strips; we deliberately do NOT touch
 * those values. Instead the on-screen rendering is shaped here so a bound key
 * reads as a glowing indicator light rather than a flat color block:
 *
 *   - radial gradient: hot core fading to transparent at the edges
 *   - brightness maps to overall opacity, but full-brightness (done/error)
 *     is capped so it never blows out into a neon slab
 *   - saturation is softened in CSS (see .key-led) to take the edge off the
 *     pure #00FF4C / #FF0033 without losing recognizability
 */
function rgbCss(rgb: [number, number, number] | null, br: number): string {
  if (!rgb) return 'transparent'
  // brightness → opacity, but cap at ~0.8 so 255 doesn't overexpose.
  const a = Math.min(0.8, Math.max(0.22, br / 255))
  const [r, g, b] = rgb
  const core = `rgba(${r}, ${g}, ${b}, ${a})`
  const mid = `rgba(${r}, ${g}, ${b}, ${a * 0.55})`
  const edge = `rgba(${r}, ${g}, ${b}, 0)`
  return `radial-gradient(circle at 50% 42%, ${core} 0%, ${mid} 38%, ${edge} 78%)`
}

function escapeHtml(s: string): string {
  return s
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
}

const CATEGORY_ORDER: ProjectCategory[] = ['task', 'project', 'automation']
const CATEGORY_LABEL: Record<ProjectCategory, string> = {
  task: '任务',
  project: '项目',
  automation: '自动化',
}

/** Fallback leaf-name label when backend didn't set projectLabel. */
function projectName(path?: string): string {
  if (!path) return '(unknown project)'
  const norm = path.replace(/\\/g, '/')
  const parts = norm.split('/').filter(Boolean)
  return parts[parts.length - 1] || path
}

/**
 * Group key for bind step 2 — one row per workspace, mirroring WorkBuddy's UI:
 * - 任务 / 项目 / 自动化 all dedupe by workspace path (cwd).
 *   A playground task cwd holds one session; an automation cwd holds many runs;
 *   a project cwd holds many sessions — but the bind row is the workspace.
 */
function bindGroupKey(s: SessionInfo): string {
  const cat = s.projectCategory
  const ws = s.workspacePath || '(unknown)'
  if (cat === 'task') return `task:${ws}`
  if (cat === 'automation') return `auto:${ws}`
  return `proj:${ws}`
}

function bindGroupLabel(s: SessionInfo, peers: SessionInfo[]): string {
  if (s.projectLabel) return s.projectLabel
  if (s.projectCategory === 'task') return s.title || projectName(s.workspacePath)
  if (s.projectCategory === 'automation') {
    return s.projectLabel || projectName(s.workspacePath)
  }
  // Prefer a non-empty label from any peer in the group.
  for (const p of peers) {
    if (p.projectLabel) return p.projectLabel
  }
  return projectName(s.workspacePath)
}

function bindGroupCategory(peers: SessionInfo[]): ProjectCategory {
  return peers.find((p) => p.projectCategory)?.projectCategory ?? 'project'
}

function clearLongPress() {
  if (longPressTimer != null) {
    window.clearTimeout(longPressTimer)
    longPressTimer = null
  }
}

async function minimizeWindow() {
  await invoke('minimize_window')
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
          <div class="key-led fx-${fx} led-${slot.status}" style="background:${rgbCss(rgb, br)}"></div>
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
          <button class="icon-btn" id="btn-hide" title="最小化">—</button>
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
    void minimizeWindow()
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
      btn.classList.add('firing')
      try {
        const result = await invoke<string>('dispatch_action', { action })
        flashActionResult(action, result)
      } catch (err) {
        flashActionResult(action, `error:${String(err)}`)
      } finally {
        setTimeout(() => btn.classList.remove('firing'), 220)
      }
    })
  })
}

/** 把后端返回的 ok:/unsupported:/error: 状态字符串翻译成一句话提示。 */
function flashActionResult(action: string, result: string): void {
  const label = { accept: 'OK', reject: 'NO', stop: 'STP' }[action] ?? action.toUpperCase()
  let msg: string
  if (result.startsWith('ok:')) {
    msg = `${label} 已发出`
  } else if (result.includes(':no_target') || result.includes(':empty_slot')) {
    msg = `${label}：当前焦点无会话`
  } else if (result.includes(':no_request_id')) {
    msg = `${label}：暂不支持（需捕获 requestId）`
  } else if (result.startsWith('unsupported:')) {
    msg = `${label}：该后端暂不支持`
  } else if (result.startsWith('error:')) {
    msg = `${label} 失败：${result.slice(6)}`
  } else {
    msg = `${label}：${result}`
  }
  showToast(msg)
}

/** 轻量 toast：固定定位，2 秒后淡出。多次触发会替换。 */
function showToast(msg: string): void {
  let toast = document.getElementById('action-toast')
  if (!toast) {
    toast = document.createElement('div')
    toast.id = 'action-toast'
    toast.className = 'action-toast'
    document.body.appendChild(toast)
  }
  toast.textContent = msg
  toast.classList.remove('fade-out')
  // restart animation
  void toast.offsetWidth
  toast.classList.add('show')
  clearTimeout((toast as HTMLElement & { _t?: number })._t)
  ;(toast as HTMLElement & { _t?: number })._t = window.setTimeout(() => {
    toast?.classList.add('fade-out')
    setTimeout(() => toast?.classList.remove('show', 'fade-out'), 300)
  }, 1800)
}

function paintSettings() {
  const afterOpenMin = Math.max(1, Math.round(settings.doneTtlAfterOpenMs / 60_000))
  const unopenedHours = Math.max(1, Math.round(settings.doneTtlUnopenedMs / 3_600_000))
  app.innerHTML = `
    <div class="panel-page">
      <div class="titlebar" data-tauri-drag-region>
        <div class="brand" data-tauri-drag-region>设置</div>
        <div class="title-actions">
          <button class="icon-btn" id="btn-back" title="返回">←</button>
          <button class="icon-btn" id="btn-hide" title="最小化">—</button>
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
        <label class="setting-row">
          <div>
            <div class="setting-title">点开后保持完成态</div>
            <div class="setting-desc">在 Agent Deck 点过该键后，Done（绿）保持多久再变 Idle</div>
          </div>
          <span class="setting-input">
            <input type="number" id="done-ttl-open" min="1" step="1" value="${afterOpenMin}" />
            <span class="setting-unit">分钟</span>
          </span>
        </label>
        <label class="setting-row">
          <div>
            <div class="setting-title">未点开最长保持完成态</div>
            <div class="setting-desc">从未点开时，Done 最多保持多久后强制变 Idle（WorkBuddy）</div>
          </div>
          <span class="setting-input">
            <input type="number" id="done-ttl-unopened" min="1" step="1" value="${unopenedHours}" />
            <span class="setting-unit">小时</span>
          </span>
        </label>
        <div class="setting-help">
          <p><b>绑定会话：</b>在键盘上<strong>长按</strong>任意键（如 A3）→ 选 ZCode/Codex → 选项目 → 选会话。</p>
          <p><b>打开会话：</b>点绑定后的键 → 打开对应工具；同时开始 Done 短倒计时。</p>
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
    void minimizeWindow()
  })
  document.getElementById('auto-fill')?.addEventListener('change', async (ev) => {
    const enabled = (ev.target as HTMLInputElement).checked
    await invoke('set_auto_fill', { enabled })
    settings.autoFill = enabled
  })

  const persistDoneTtl = async () => {
    const openEl = document.getElementById('done-ttl-open') as HTMLInputElement | null
    const unopenedEl = document.getElementById('done-ttl-unopened') as HTMLInputElement | null
    if (!openEl || !unopenedEl) return
    const afterOpenMs = Math.max(1, Math.round(Number(openEl.value) || 5)) * 60_000
    const unopenedMs = Math.max(1, Math.round(Number(unopenedEl.value) || 12)) * 3_600_000
    await invoke('set_done_ttl', { afterOpenMs, unopenedMs })
    settings.doneTtlAfterOpenMs = afterOpenMs
    settings.doneTtlUnopenedMs = unopenedMs
  }
  document.getElementById('done-ttl-open')?.addEventListener('change', () => {
    void persistDoneTtl()
  })
  document.getElementById('done-ttl-unopened')?.addEventListener('change', () => {
    void persistDoneTtl()
  })
}

function paintBind() {
  const slotLabel = bindSlot == null ? '?' : `A${bindSlot + 1}`
  let body = ''

  if (bindStep === 'backend') {
    const backends: BackendId[] = ['zcode', 'codex', 'workbuddy']
    body = `
      <div class="bind-step">1. 选择工具 · 共 ${sessionsCache.length} 条历史</div>
      <div class="bind-list">
        ${backends
          .map((b) => {
            const list = sessionsCache.filter((s) => s.backend === b)
            const groups = new Set(list.map((s) => bindGroupKey(s)))
            return `<button class="bind-item" data-backend="${b}">
              <span>${BACKEND_LABEL[b]}</span>
              <span class="muted">${groups.size} 分组 · ${list.length} 会话</span>
            </button>`
          })
          .join('')}
      </div>
    `
  } else if (bindStep === 'project' && bindBackend) {
    const byGroup = new Map<string, SessionInfo[]>()
    for (const s of sessionsCache.filter((x) => x.backend === bindBackend)) {
      const key = bindGroupKey(s)
      const list = byGroup.get(key) ?? []
      list.push(s)
      byGroup.set(key, list)
    }
    type Group = { key: string; list: SessionInfo[]; cat: ProjectCategory; label: string; maxAt: number }
    const groups: Group[] = [...byGroup.entries()].map(([key, list]) => {
      const sorted = list.slice().sort((a, b) => b.updatedAt - a.updatedAt)
      const cat = bindGroupCategory(sorted)
      const label = bindGroupLabel(sorted[0], sorted)
      const maxAt = Math.max(...sorted.map((s) => s.updatedAt))
      return { key, list: sorted, cat, label, maxAt }
    })
    groups.sort((a, b) => {
      const ca = CATEGORY_ORDER.indexOf(a.cat)
      const cb = CATEGORY_ORDER.indexOf(b.cat)
      return ca - cb || b.maxAt - a.maxAt
    })
    const hasSections = groups.some((g) => g.cat !== 'project')
      || groups.some((g) => g.list.some((s) => s.projectCategory))
    const sectionsHtml = hasSections
      ? CATEGORY_ORDER.map((cat) => {
          const section = groups.filter((g) => g.cat === cat)
          if (!section.length) return ''
          return `
            <div class="bind-section">
              <div class="bind-section-title">${CATEGORY_LABEL[cat]} · ${section.length}</div>
              ${section
                .map((g) => {
                  const latest = g.list[0]
                  const secondary =
                    g.cat === 'project'
                      ? g.list[0]?.workspacePath || ''
                      : latest
                        ? `${STATUS_LABEL[latest.status]}${g.list.length > 1 ? ` · ${g.list.length} 会话` : ''}`
                        : ''
                  return `<button class="bind-item" data-project="${escapeHtml(g.key)}">
                    <span>${escapeHtml(g.label)}</span>
                    <div class="bind-meta">
                      <span class="muted">${escapeHtml(secondary)}</span>
                      ${
                        g.cat === 'project'
                          ? `<span class="bind-count">${g.list.length} 会话</span>`
                          : g.list.length > 1
                            ? `<span class="bind-count">${g.list.length}</span>`
                            : ''
                      }
                    </div>
                    ${
                      g.cat === 'project' && latest
                        ? `<span class="muted">最近：${escapeHtml(latest.title)} · ${STATUS_LABEL[latest.status]}</span>`
                        : ''
                    }
                  </button>`
                })
                .join('')}
            </div>`
        }).join('')
      : groups
          .map((g) => {
            const latest = g.list[0]
            return `<button class="bind-item" data-project="${escapeHtml(g.key)}">
              <span>${escapeHtml(g.label)}</span>
              <div class="bind-meta">
                <span class="muted">${escapeHtml(g.list[0]?.workspacePath || '')}</span>
                <span class="bind-count">${g.list.length} 会话</span>
              </div>
              ${
                latest
                  ? `<span class="muted">最近：${escapeHtml(latest.title)} · ${STATUS_LABEL[latest.status]}</span>`
                  : ''
              }
            </button>`
          })
          .join('')
    body = `
      <div class="bind-step">2. 选择${hasSections ? '分组' : '项目'} · ${BACKEND_LABEL[bindBackend]} · ${groups.length} 个</div>
      <div class="bind-list">
        ${
          groups.length
            ? sectionsHtml
            : `<div class="empty-hint">没有来自 ${BACKEND_LABEL[bindBackend]} 的会话</div>`
        }
      </div>
    `
  } else if (bindStep === 'session' && bindBackend) {
    const list = sessionsCache
      .filter(
        (s) => s.backend === bindBackend && bindGroupKey(s) === (bindProject || ''),
      )
      .slice()
      .sort((a, b) => b.updatedAt - a.updatedAt)
    // Header label: prefer projectLabel / title over raw group key.
    const headerLabel =
      list[0]?.projectLabel ||
      (list[0]?.projectCategory === 'task' ? list[0]?.title : undefined) ||
      projectName(list[0]?.workspacePath) ||
      bindProject ||
      ''
    body = `
      <div class="bind-step">3. 选择会话 · ${escapeHtml(headerLabel)} · ${list.length} 条</div>
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
            : `<div class="empty-hint">此分组下没有会话</div>`
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
          <button class="icon-btn" id="btn-hide" title="最小化">—</button>
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
    void minimizeWindow()
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
