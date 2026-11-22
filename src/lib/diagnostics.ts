import type { DiagnosticEntry } from '../types'

/** Newest first, with the monotonically assigned backend ID breaking equal-second ties. */
export function sortDiagnosticsNewestFirst(entries: DiagnosticEntry[]): DiagnosticEntry[] {
  return [...entries].sort((left, right) => right.at - left.at || right.id - left.id)
}
