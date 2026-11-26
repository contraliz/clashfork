import { describe, expect, it } from 'vitest'
import { safeExportFileName } from './exportName'

describe('safe export file names', () => {
  it('replaces Windows-invalid characters and adds a json extension', () => {
    const name = safeExportFileName('clashfork-ruleset-Google:/YouTube*?<>|"\\')
    expect(name.endsWith('.json')).toBe(true)
    expect(name).toContain('clashfork-ruleset-Google')
    expect(name).not.toMatch(/[<>:"/\\|?*]/)
  })

  it('strips trailing dots and spaces and handles reserved names', () => {
    expect(safeExportFileName('clashfork-rule-trailing. . ')).toBe('clashfork-rule-trailing.json')
    expect(safeExportFileName('CON')).toBe('clashfork-CON.json')
    expect(safeExportFileName('LPT1.json', 'clashfork-ruleset')).toBe('clashfork-LPT1.json')
    expect(safeExportFileName('...', 'clashfork-ruleset')).toBe('clashfork-ruleset.json')
    expect(safeExportFileName('***', 'clashfork-ruleset')).toBe('clashfork-ruleset.json')
  })

  it('keeps the final name bounded', () => {
    const name = safeExportFileName(`clashfork-ruleset-${'a'.repeat(200)}`)
    expect(name.length).toBeLessThanOrEqual(120)
    expect(name.endsWith('.json')).toBe(true)
  })
})
