/**
 * 单个灯位的渲染
 */

import React from 'react'
import { Text, Box } from 'ink'

export interface LedProps {
  index: number
  rgb: [number, number, number] | null
  br: number
  fx: string
  title?: string
  status: string
  detail?: string
  focused?: boolean
}

export function Led({
  index,
  rgb,
  br,
  fx,
  title,
  status,
  detail,
  focused,
}: LedProps): React.ReactElement {
  const colorHex = rgb ? rgbToHex(rgb) : undefined
  const label = title ?? '(empty)'
  const dimmed = !rgb || br < 60

  const fxTag = rgb ? `[${fx}]` : ''
  const detailTag = detail ? `  ⚠ ${detail.slice(0, 40)}` : ''
  const focusTag = focused ? ' ◀ focus' : ''
  const statusText = status.padEnd(8)

  return (
    <Box flexDirection="row" alignItems="center">
      <Box width={2}>
        <Text color={colorHex} dimColor={dimmed}>
          ●
        </Text>
      </Box>
      <Box width={4}>
        <Text dimColor>A{index + 1}</Text>
      </Box>
      <Box width={10}>
        <Text color={colorHex} dimColor={dimmed}>
          {statusText}
        </Text>
      </Box>
      <Box width={38}>
        <Text>{label.slice(0, 36)}</Text>
      </Box>
      <Box>
        <Text dimColor>
          {fxTag}
          {detailTag}
          {focusTag}
        </Text>
      </Box>
    </Box>
  )
}

function rgbToHex(rgb: [number, number, number]): string {
  const [r, g, b] = rgb
  const toHex = (v: number): string =>
    Math.max(0, Math.min(255, Math.round(v)))
      .toString(16)
      .padStart(2, '0')
  return '#' + toHex(r) + toHex(g) + toHex(b)
}
