import { describe, it, expect } from 'vitest'
import {
  mapZcodeRow,
  mapZcodeRowAt,
  inferRisk,
  type ZcodeRow,
} from '../src/backends/zcode/mapper.js'

function makeRow(overrides: Partial<ZcodeRow> = {}): ZcodeRow {
  return {
    task_id: 'sess_test',
    title: 'test task',
    task_status: 'running',
    workspace_path: '/tmp/test',
    updated_at: 1000,
    waiting: 0,
    detail: null,
    ...overrides,
  }
}

describe('mapZcodeRow', () => {
  it('maps running + waiting=0 to working', () => {
    const r = mapZcodeRow(makeRow({ task_status: 'running', waiting: 0 }))
    expect(r.status).toBe('working')
    expect(r.backend).toBe('zcode')
    expect(r.sessionId).toBe('sess_test')
    expect(r.risk).toBeUndefined()
    expect(r.waitingSince).toBeUndefined()
  })

  it('maps running + waiting=1 + bash detail to waiting high', () => {
    const r = mapZcodeRow(
      makeRow({
        task_status: 'running',
        waiting: 1,
        detail: 'Bash: shell',
      }),
    )
    expect(r.status).toBe('waiting')
    expect(r.risk).toBe('high')
    expect(r.waitingSince).toBe(1000)
    expect(r.detail).toBe('Bash: shell')
  })

  it('maps completed to done with no risk', () => {
    const r = mapZcodeRow(makeRow({ task_status: 'completed' }))
    expect(r.status).toBe('done')
    expect(r.risk).toBeUndefined()
    expect(r.waitingSince).toBeUndefined()
  })

  it('maps error to error', () => {
    const r = mapZcodeRow(makeRow({ task_status: 'error' }))
    expect(r.status).toBe('error')
  })

  it('maps NULL task_status to idle', () => {
    const r = mapZcodeRow(makeRow({ task_status: null }))
    expect(r.status).toBe('idle')
  })

  it('maps unknown task_status to idle', () => {
    const r = mapZcodeRow(makeRow({ task_status: 'something_new' }))
    expect(r.status).toBe('idle')
  })

  it('waitingSince is updated_at when waiting', () => {
    const r = mapZcodeRow(
      makeRow({
        task_status: 'running',
        waiting: 1,
        updated_at: 5000,
        detail: 'AskUserQuestion: userInteraction',
      }),
    )
    expect(r.waitingSince).toBe(5000)
  })

  it('non-waiting has no waitingSince', () => {
    const r = mapZcodeRow(makeRow({ task_status: 'running', waiting: 0 }))
    expect(r.waitingSince).toBeUndefined()
  })

  it('title null becomes (untitled)', () => {
    const r = mapZcodeRow(makeRow({ title: null }))
    expect(r.title).toBe('(untitled)')
  })

  it('workspacePath passed through', () => {
    const r = mapZcodeRow(makeRow({ workspace_path: '/Users/foo/proj' }))
    expect(r.workspacePath).toBe('/Users/foo/proj')
  })

  it('workspacePath null becomes undefined', () => {
    const r = mapZcodeRow(makeRow({ workspace_path: null }))
    expect(r.workspacePath).toBeUndefined()
  })

  it('detail null becomes undefined', () => {
    const r = mapZcodeRow(makeRow({ detail: null }))
    expect(r.detail).toBeUndefined()
  })

  it('mapZcodeRowAt accepts now param', () => {
    const r = mapZcodeRowAt(makeRow(), 12345)
    expect(r).toBeDefined()
  })
})

describe('inferRisk', () => {
  it('Bash: shell → high', () => {
    expect(inferRisk('Bash: shell')).toBe('high')
  })

  it('rm -rf / → high', () => {
    expect(inferRisk('rm -rf /')).toBe('high')
  })

  it('git push origin main → high', () => {
    expect(inferRisk('git push origin main')).toBe('high')
  })

  it('git reset --hard → high', () => {
    expect(inferRisk('git reset --hard')).toBe('high')
  })

  it('delete file → high', () => {
    expect(inferRisk('delete some file')).toBe('high')
  })

  it('Edit: fileWrite → medium', () => {
    expect(inferRisk('Edit: fileWrite')).toBe('medium')
  })

  it('Write: new file → medium', () => {
    expect(inferRisk('Write: new file')).toBe('medium')
  })

  it('AskUserQuestion: userInteraction → low', () => {
    expect(inferRisk('AskUserQuestion: userInteraction')).toBe('low')
  })

  it('Grep: read → low', () => {
    expect(inferRisk('Grep: read pattern')).toBe('low')
  })

  it('Glob: list → low', () => {
    expect(inferRisk('Glob: list pattern')).toBe('low')
  })

  it('null → medium (conservative default)', () => {
    expect(inferRisk(null)).toBe('medium')
  })

  it('unknown content → medium', () => {
    expect(inferRisk('SomethingUnknown')).toBe('medium')
  })

  it('empty string → medium', () => {
    expect(inferRisk('')).toBe('medium')
  })

  it('high wins over low (priority order)', () => {
    // "read" 是 low，但 "delete" 是 high，high 优先
    expect(inferRisk('delete and then read')).toBe('high')
  })

  it('case insensitive', () => {
    expect(inferRisk('BASH: SHELL')).toBe('high')
    expect(inferRisk('edit: filewrite')).toBe('medium')
  })
})
