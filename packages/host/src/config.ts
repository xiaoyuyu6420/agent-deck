/**
 * Host 配置（~/.agent-deck/config.json）
 */

import { readFileSync, existsSync, writeFileSync, mkdirSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'

export interface HostConfig {
  /** Gateway 端口 */
  port: number

  /** 启用哪些 backend */
  enabledBackends: ('zcode' | 'codex')[]

  /** ZCode 路径覆盖（默认 ~/.zcode） */
  zcodeHome?: string

  /** Codex 路径覆盖（默认 ~/.codex） */
  codexHome?: string

  /** 自指防护：排除哪些 workspace 路径 */
  excludeWorkspaces: string[]

  /** 自指防护：排除哪些 sessionId */
  excludeTaskIds: string[]

  /** 槽位数（V1 = 5） */
  slotCount: number

  /** 灯效主题 */
  theme: 'codex' | 'high-contrast' | 'protanopia-friendly'

  /** 调试日志 */
  debug: boolean
}

const CONFIG_DIR = join(homedir(), '.agent-deck')
const CONFIG_FILE = join(CONFIG_DIR, 'config.json')

export const DEFAULT_CONFIG: HostConfig = {
  port: 8787,
  enabledBackends: ['zcode', 'codex'],
  excludeWorkspaces: [],
  excludeTaskIds: [],
  slotCount: 5,
  theme: 'codex',
  debug: false,
}

export function loadConfig(): HostConfig {
  if (!existsSync(CONFIG_FILE)) {
    return { ...DEFAULT_CONFIG }
  }
  try {
    const raw = readFileSync(CONFIG_FILE, 'utf-8')
    const parsed = JSON.parse(raw)
    return { ...DEFAULT_CONFIG, ...parsed }
  } catch (err) {
    console.warn(`[config] failed to parse ${CONFIG_FILE}, using defaults:`, err)
    return { ...DEFAULT_CONFIG }
  }
}

export function saveConfig(config: HostConfig): void {
  if (!existsSync(CONFIG_DIR)) {
    mkdirSync(CONFIG_DIR, { recursive: true })
  }
  writeFileSync(CONFIG_FILE, JSON.stringify(config, null, 2))
}
