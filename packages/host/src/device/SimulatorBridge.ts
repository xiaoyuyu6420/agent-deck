/**
 * SimulatorBridge —— 连接 SessionBoard（输出）和 Gateway（传输），
 * 同时把 ClientMessage 转成 Action 给 ActionRouter。
 *
 * V1 没有真硬件，"设备"就是连过来的 WS client（simulator）。
 */

import type {
  ClientMessage,
  Action,
  ServerMessage,
  LedFrame,
  BoardState,
} from '@agent-deck/protocol'
import type { SessionBoard } from '../board/SessionBoard.js'
import type { ActionRouter } from '../actions/ActionRouter.js'
import type { Gateway } from '../gateway/server.js'
import { bus } from '../bus.js'

export interface SimulatorBridgeDeps {
  board: SessionBoard
  router: ActionRouter
  gateway: Gateway
}

export class SimulatorBridge {
  private unsubLed?: () => void
  private unsubBoard?: () => void
  private started = false

  constructor(private deps: SimulatorBridgeDeps) {}

  start(): void {
    if (this.started) return
    this.started = true

    // 订阅 SessionBoard 输出，转发给所有 WS client
    this.unsubLed = this.deps.board.onLedFrame((frame) => {
      this.deps.gateway.broadcast(frame as ServerMessage)
    })
    this.unsubBoard = this.deps.board.onBoardState((state) => {
      this.deps.gateway.broadcast(state as ServerMessage)
    })
  }

  stop(): void {
    this.unsubLed?.()
    this.unsubBoard?.()
    this.unsubLed = undefined
    this.unsubBoard = undefined
    this.started = false
  }

  /**
   * 处理来自 WS client 的消息（由 Gateway.onMessage 调用）
   */
  async handleClientMessage(msg: ClientMessage): Promise<void> {
    try {
      if (msg.t === 'action') {
        await this.deps.router.dispatch(msg.action)
        return
      }
      if (msg.t === 'key') {
        // 只处理 down 边沿
        if (msg.edge !== 'down') return
        const action = translateKey(msg.id)
        if (action) {
          await this.deps.router.dispatch(action)
        }
        return
      }
      // enc / joy / voice 在 V1 暂不映射
      // 静默忽略，不报错
    } catch (err) {
      const error = err instanceof Error ? err : new Error(String(err))
      bus.emit('action.failed', { op: 'handleClientMessage', error })
    }
  }
}

/**
 * 物理键 id → Action 映射
 * 'a1'..'a5' down → focus 槽 N-1
 * 'accept' / 'reject' / 'stop' → 对应 op
 */
function translateKey(keyId: string): Action | undefined {
  // 状态键 a1..a9
  const m = /^a([1-9])$/.exec(keyId)
  if (m) {
    const i = parseInt(m[1]!, 10) - 1
    return { op: 'focus', i }
  }
  switch (keyId) {
    case 'accept':
      return { op: 'accept' }
    case 'reject':
      return { op: 'reject' }
    case 'stop':
      return { op: 'stop' }
    default:
      return undefined
  }
}

// 类型导出供其他模块用
export type { LedFrame, BoardState }
