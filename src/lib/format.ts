import type { Allowance, NodeView } from '../types'

export function formatBytes(value: number, digits = 1): string {
  if (!Number.isFinite(value) || value <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let amount = value
  let unit = 0
  while (amount >= 1024 && unit < units.length - 1) {
    amount /= 1024
    unit += 1
  }
  return `${amount >= 100 || unit === 0 ? amount.toFixed(0) : amount.toFixed(digits)} ${units[unit]}`
}

export function formatRate(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0 B/s'
  return `${formatBytes(value)}/s`
}

export function allowanceSummary(allowance: Allowance): string {
  switch (allowance.kind) {
    case 'full':
      return `${formatBytes(allowance.remaining)} remaining`
    case 'remaining':
      return `${formatBytes(allowance.remaining)} remaining · provider reported`
    default:
      return 'Allowance unavailable'
  }
}

export function allowanceProgress(allowance: Allowance): number | null {
  if (allowance.kind !== 'full' || allowance.total <= 0) return null
  return Math.min(100, Math.max(0, (allowance.used / allowance.total) * 100))
}

export function formatDate(seconds: number | null | undefined): string {
  if (!seconds) return 'Never'
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  }).format(new Date(seconds * 1000))
}

export function formatTime(seconds: number | null | undefined): string {
  if (!seconds) return '—'
  return new Intl.DateTimeFormat(undefined, {
    hour: 'numeric',
    minute: '2-digit',
    second: '2-digit',
  }).format(new Date(seconds * 1000))
}

export function formatExpiry(seconds: number | null | undefined): string {
  if (!seconds) return 'No expiry reported'
  return new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  }).format(new Date(seconds * 1000))
}

export function routeLabel(node: NodeView): string {
  return `${node.serviceTitle} / ${node.displayName}`
}

export function nodeSelectionValue(value: string): { kind: 'direct' | 'block' | 'node'; nodeId?: string } {
  if (value === 'direct') return { kind: 'direct' }
  if (value === 'block') return { kind: 'block' }
  return { kind: 'node', nodeId: value.replace(/^node:/, '') }
}

export function destinationValue(destination: {
  kind: 'direct' | 'block' | 'node'
  nodeId?: string
}): string {
  if (destination.kind === 'node') return `node:${destination.nodeId ?? ''}`
  return destination.kind
}

export function modeSummary(summary: string | null): string {
  return summary ?? 'Disconnected'
}

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value))
}

export function prettyDateRange(from: number, to: number): string {
  const formatter = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' })
  return `${formatter.format(new Date(from * 1000))} – ${formatter.format(new Date(to * 1000))}`
}

export function safeAppName(value: string | null | undefined): string {
  if (!value) return 'Unattributed'
  const name = value.trim()
  return name || 'Unattributed'
}
