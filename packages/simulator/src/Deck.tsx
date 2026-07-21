/**
 * Deck 主组件：连接 WS、显示 5 个灯、处理键盘
 *
 * 自动检测 raw mode 是否可用：
 *   - 可用（真终端）：完整交互，1-5/a/r/s/f/u/q 全生效
 *   - 不可用（管道/CI）：纯显示模式，只亮灯、不接收键盘
 */

import React, { useState, useEffect, useRef } from 'react'
import { render, Text, Box, useInput, useApp, useStdin } from 'ink'
import type {
  ServerMessage,
  LedSlot,
  SlotBinding,
  Action,
} from '@agent-deck/protocol'
import { WsClient, type ConnStatus } from './wsClient.js'
import { Led } from './Led.js'

const EMPTY_SLOTS = 5

// ─── WS + 状态逻辑（共用）─────────────────────────────────────────────────

interface DeckProps {
  url: string
}

export function Deck({ url }: DeckProps): React.ReactElement {
  const [ledSlots, setLedSlots] = useState<(LedSlot | null)[]>(
    Array.from({ length: EMPTY_SLOTS }, () => null),
  )
  const [bindings, setBindings] = useState<(SlotBinding | null)[]>(
    Array.from({ length: EMPTY_SLOTS }, () => null),
  )
  const [focus, setFocus] = useState(0)
  const [status, setStatus] = useState<ConnStatus>('connecting')
  const [lastAction, setLastAction] = useState<string>('')
  const clientRef = useRef<WsClient | null>(null)

  useEffect((): (() => void) | undefined => {
    const client = new WsClient({
      url,
      onStatus: setStatus,
      onMessage: (msg: ServerMessage) => {
        if (msg.type === 'leds') {
          const arr: (LedSlot | null)[] = Array.from(
            { length: EMPTY_SLOTS },
            () => null,
          )
          for (const s of msg.slots) {
            if (s.i >= 0 && s.i < EMPTY_SLOTS) arr[s.i] = s
          }
          setLedSlots(arr)
        } else if (msg.type === 'board') {
          const arr: (SlotBinding | null)[] = Array.from(
            { length: EMPTY_SLOTS },
            () => null,
          )
          for (const s of msg.slots) {
            if (s.i >= 0 && s.i < EMPTY_SLOTS) arr[s.i] = s
          }
          setBindings(arr)
          setFocus(msg.focus)
        } else if (msg.type === 'focus') {
          setFocus(msg.i)
        }
      },
    })
    clientRef.current = client
    client.start()
    return () => {
      client.dispose()
      clientRef.current = null
    }
  }, [url])

  const sendAction = (action: Action): void => {
    clientRef.current?.send({ t: 'action', action })
    setLastAction(`${action.op}${'i' in action ? ` #${action.i}` : ''}`)
  }

  return (
    <DeckWithInput
      ledSlots={ledSlots}
      bindings={bindings}
      focus={focus}
      status={status}
      lastAction={lastAction}
      sendAction={sendAction}
    />
  )
}

// ─── 条件渲染：检测 raw mode ──────────────────────────────────────────────

interface DeckCoreProps {
  ledSlots: (LedSlot | null)[]
  bindings: (SlotBinding | null)[]
  focus: number
  status: ConnStatus
  lastAction: string
  sendAction: (action: Action) => void
}

/**
 * 根据 isRawModeSupported 拆到 Interactive 或 ReadOnly 子组件。
 * 两个子组件各自无条件调用自己的 hooks（useInput 只在 Interactive 里）。
 */
function DeckWithInput(props: DeckCoreProps): React.ReactElement {
  const { isRawModeSupported } = useStdin()
  return isRawModeSupported ? (
    <DeckInteractive {...props} />
  ) : (
    <DeckReadOnly
      ledSlots={props.ledSlots}
      bindings={props.bindings}
      focus={props.focus}
      status={props.status}
    />
  )
}

// ─── 完整交互版（真终端）─────────────────────────────────────────────────

function DeckInteractive({
  ledSlots,
  bindings,
  focus,
  status,
  lastAction,
  sendAction,
}: DeckCoreProps): React.ReactElement {
  const { exit } = useApp()

  useInput((input: string) => {
    if (input === 'q' || input === 'Q') {
      exit()
      return
    }
    if (input >= '1' && input <= '5') {
      const i = parseInt(input, 10) - 1
      sendAction({ op: 'focus', i })
      return
    }
    if (input === 'a') {
      sendAction({ op: 'accept' })
      return
    }
    if (input === 'r') {
      sendAction({ op: 'reject' })
      return
    }
    if (input === 's') {
      sendAction({ op: 'stop' })
      return
    }
    if (input === 'f') {
      sendAction({ op: 'freeze_all' })
      return
    }
    if (input === 'u') {
      sendAction({ op: 'unfreeze' })
      return
    }
  })

  return (
    <Box flexDirection="column" padding={1}>
      <Box marginBottom={1}>
        <Text bold>Agent Deck Simulator</Text>
        <Text> </Text>
        <StatusBadge status={status} />
      </Box>

      <Box flexDirection="column" marginBottom={1}>
        {Array.from({ length: EMPTY_SLOTS }, (_, i) => (
          <Led
            key={i}
            index={i}
            rgb={ledSlots[i]?.rgb ?? null}
            br={ledSlots[i]?.br ?? 0}
            fx={ledSlots[i]?.fx ?? 'solid'}
            title={bindings[i]?.title}
            status={bindings[i]?.status ?? 'off'}
            detail={bindings[i]?.detail}
            focused={i === focus}
          />
        ))}
      </Box>

      <Box marginTop={1} flexDirection="column">
        <Text dimColor>keys: 1-5 focus | a accept | r reject | s stop | f freeze | u unfreeze | q quit</Text>
        {lastAction ? <Text dimColor>last: {lastAction}</Text> : null}
      </Box>
    </Box>
  )
}

// ─── 只读版（无 raw mode，管道/CI 安全）───────────────────────────────────

interface DeckReadOnlyProps {
  ledSlots: (LedSlot | null)[]
  bindings: (SlotBinding | null)[]
  focus: number
  status: ConnStatus
}

function DeckReadOnly({
  ledSlots,
  bindings,
  focus,
  status,
}: DeckReadOnlyProps): React.ReactElement {
  return (
    <Box flexDirection="column" padding={1}>
      <Box marginBottom={1}>
        <Text bold>Agent Deck Simulator</Text>
        <Text> </Text>
        <StatusBadge status={status} />
        <Text dimColor> (display only — no TTY)</Text>
      </Box>

      <Box flexDirection="column" marginBottom={1}>
        {Array.from({ length: EMPTY_SLOTS }, (_, i) => (
          <Led
            key={i}
            index={i}
            rgb={ledSlots[i]?.rgb ?? null}
            br={ledSlots[i]?.br ?? 0}
            fx={ledSlots[i]?.fx ?? 'solid'}
            title={bindings[i]?.title}
            status={bindings[i]?.status ?? 'off'}
            detail={bindings[i]?.detail}
            focused={i === focus}
          />
        ))}
      </Box>
    </Box>
  )
}

// ─── 通用组件 ────────────────────────────────────────────────────────────

function StatusBadge({ status }: { status: ConnStatus }): React.ReactElement {
  const color = status === 'connected' ? 'green' : status === 'connecting' ? 'yellow' : 'red'
  return <Text color={color}>[{status}]</Text>
}

// 保持 render import 用于 main.tsx 直接复用
export { render }
