/**
 * ActionRouter —— 把按键/语音/WS 消息路由到对应 backend 的动作方法。
 *
 * V1 大部分 backend 未实现 accept/reject/stop，dispatch 会优雅降级返回 unsupported。
 */

import type { Action, PolicyMode } from '@agent-deck/protocol'
import type { AgentBackend } from '../backends/types.js'
import type { SessionBoard } from '../board/SessionBoard.js'
import { bus } from '../bus.js'

export interface ActionResult {
  ok: boolean
  /** 'applied' | 'unsupported' | 'no-target' | 'frozen' | 'error' */
  reason: string
  error?: string
}

export interface SlotBindingLike {
  backend?: string
  sessionId?: string
}

export interface ActionRouterDeps {
  board: SessionBoard
  /** 给定 slot index 返回该槽的 backend + sessionId */
  getSlotBinding: (i: number) => SlotBindingLike | undefined
  /** 按 backend id 取 backend 实例 */
  getBackend: (id: string) => AgentBackend | undefined
  /** 日志 */
  log?: (msg: string, ...args: unknown[]) => void
}

const NON_FROZEN_OPS = new Set(['unfreeze', 'stop_all', 'stop'])

export class ActionRouter {
  private frozen = false
  private mode: PolicyMode = 'act'

  constructor(private deps: ActionRouterDeps) {}

  async dispatch(action: Action): Promise<ActionResult> {
    // 1. freeze 拦截
    if (this.frozen && !NON_FROZEN_OPS.has(action.op)) {
      return { ok: false, reason: 'frozen' }
    }

    // 2. 无需 target 的动作
    if (action.op === 'freeze_all') {
      this.frozen = true
      this.deps.log?.(`[router] frozen=true`)
      bus.emit('action.done', action)
      return { ok: true, reason: 'applied' }
    }
    if (action.op === 'unfreeze') {
      this.frozen = false
      this.deps.log?.(`[router] frozen=false`)
      bus.emit('action.done', action)
      return { ok: true, reason: 'applied' }
    }
    if (action.op === 'set_mode') {
      this.mode = action.mode
      this.deps.log?.(`[router] mode=${action.mode}`)
      bus.emit('action.done', action)
      return { ok: true, reason: 'applied' }
    }
    if (action.op === 'stop_all') {
      // V1 没有 backend.stopAll 接口，仅 stop(sessionId)
      // 标 unsupported 即可，Phase 3 再做
      return { ok: false, reason: 'unsupported' }
    }

    // 3. focus 仅打标，不调 backend
    if (action.op === 'focus') {
      try {
        this.deps.board.setFocus(action.i)
        bus.emit('action.done', action)
        return { ok: true, reason: 'applied' }
      } catch (err) {
        return this.fail(action, err)
      }
    }

    // 4. 解析 target slot
    const slotIdx = 'i' in action && action.i !== undefined ? action.i : this.getFocus()
    if (slotIdx === undefined || slotIdx === null) {
      return { ok: false, reason: 'no-target' }
    }
    const binding = this.deps.getSlotBinding(slotIdx)
    if (!binding?.backend || !binding?.sessionId) {
      return { ok: false, reason: 'no-target' }
    }

    const backend = this.deps.getBackend(binding.backend)
    if (!backend) {
      return { ok: false, reason: 'no-target' }
    }

    // 5. 分派到 backend 方法
    try {
      switch (action.op) {
        case 'accept':
          if (!backend.accept) return { ok: false, reason: 'unsupported' }
          await backend.accept(binding.sessionId)
          break
        case 'reject':
          if (!backend.reject) return { ok: false, reason: 'unsupported' }
          await backend.reject(binding.sessionId)
          break
        case 'stop':
          if (!backend.interrupt) return { ok: false, reason: 'unsupported' }
          await backend.interrupt(binding.sessionId)
          break
        case 'send':
          if (!backend.send) return { ok: false, reason: 'unsupported' }
          await backend.send(binding.sessionId, action.text)
          break
        default: {
          // exhaustive check
          const _exhaustive: never = action
          void _exhaustive
          return { ok: false, reason: 'unsupported' }
        }
      }
      bus.emit('action.done', action)
      return { ok: true, reason: 'applied' }
    } catch (err) {
      return this.fail(action, err)
    }
  }

  private fail(action: Action, err: unknown): ActionResult {
    const error = err instanceof Error ? err : new Error(String(err))
    this.deps.log?.(`[router] action ${action.op} failed: ${error.message}`)
    bus.emit('action.failed', { op: action.op, error })
    return { ok: false, reason: 'error', error: error.message }
  }

  private getFocus(): number | undefined {
    // SessionBoard 应有 getFocus() 方法；如果没有则 undefined
    const board = this.deps.board as unknown as {
      getFocus?: () => number | undefined
    }
    return board.getFocus?.()
  }

  isFrozen(): boolean {
    return this.frozen
  }

  setFrozen(frozen: boolean): void {
    this.frozen = frozen
  }

  getMode(): PolicyMode {
    return this.mode
  }

  setMode(mode: PolicyMode): void {
    this.mode = mode
  }
}
