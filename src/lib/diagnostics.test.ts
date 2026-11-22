import { describe, expect, it } from 'vitest'
import type { DiagnosticEntry } from '../types'
import { sortDiagnosticsNewestFirst } from './diagnostics'

const entry = (id: number, at: number, message: string): DiagnosticEntry => ({
  id,
  at,
  source: 'app',
  level: 'info',
  message,
})

describe('sortDiagnosticsNewestFirst', () => {
  it('orders newest first and breaks equal timestamps by descending id', () => {
    const sorted = sortDiagnosticsNewestFirst([
      entry(1, 100, 'old'),
      entry(2, 200, 'same second, first id'),
      entry(3, 200, 'same second, second id'),
      entry(4, 150, 'middle'),
    ])

    expect(sorted.map((item) => item.message)).toEqual([
      'same second, second id',
      'same second, first id',
      'middle',
      'old',
    ])
  })

  it('does not mutate the input snapshot', () => {
    const input = [entry(1, 100, 'old'), entry(2, 200, 'new')]
    sortDiagnosticsNewestFirst(input)
    expect(input.map((item) => item.message)).toEqual(['old', 'new'])
  })
})
