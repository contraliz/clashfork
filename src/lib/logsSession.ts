import { useSyncExternalStore } from 'react'
import type { HistorySubjectFilter } from '../types'

export type LogsTab = 'history' | 'diagnostics'
export type HistoryRange = 'today' | '7d' | '30d'
export type HistoryBreakdown = 'app' | 'customGroup' | 'rule' | 'route'
export type HistoryRouteFilter = 'all' | 'proxy' | 'direct'

export interface LogsSession {
  tab: LogsTab
  range: HistoryRange
  breakdown: HistoryBreakdown
  routeFilter: HistoryRouteFilter
  subjectFilter: HistorySubjectFilter | null
}

const initialSession: LogsSession = {
  tab: 'history',
  range: '7d',
  breakdown: 'app',
  routeFilter: 'all',
  subjectFilter: null,
}

let session = initialSession
const listeners = new Set<() => void>()

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

function getSnapshot() {
  return session
}

export function updateLogsSession(patch: Partial<LogsSession>) {
  session = { ...session, ...patch }
  for (const listener of listeners) listener()
}

export function resetLogsFilters() {
  updateLogsSession({ routeFilter: 'all', subjectFilter: null })
}

export function useLogsSession(): LogsSession {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}
