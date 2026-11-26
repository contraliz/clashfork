export const NODES_UI_STORAGE_KEY = 'clashfork.nodes.ui.v1'

const MAX_ID_LENGTH = 200
const MAX_STORED_IDS = 10000

export type NodeDropPlacement = 'before' | 'after'

export interface NodesUiPreferences {
  version: 1
  collapsedServiceIds: string[]
  serviceOrder: string[]
  nodeOrderByService: Record<string, string[]>
}

export interface PreferenceStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

export function defaultNodesUiPreferences(): NodesUiPreferences {
  return {
    version: 1,
    collapsedServiceIds: [],
    serviceOrder: [],
    nodeOrderByService: {},
  }
}

function uniqueIds(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  const seen = new Set<string>()
  const ids: string[] = []
  for (const item of value) {
    if (typeof item !== 'string' || item.length === 0 || item.length > MAX_ID_LENGTH || seen.has(item)) {
      continue
    }
    seen.add(item)
    ids.push(item)
    if (ids.length >= MAX_STORED_IDS) break
  }
  return ids
}

export function parseNodesUiPreferences(raw: string | null): NodesUiPreferences {
  if (!raw) return defaultNodesUiPreferences()
  try {
    const parsed: unknown = JSON.parse(raw)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return defaultNodesUiPreferences()
    }
    const record = parsed as Record<string, unknown>
    if (record.version !== 1) return defaultNodesUiPreferences()

    const nodeOrderByService: Record<string, string[]> = {}
    const savedOrders = record.nodeOrderByService
    if (savedOrders && typeof savedOrders === 'object' && !Array.isArray(savedOrders)) {
      for (const [serviceId, savedOrder] of Object.entries(savedOrders)) {
        if (serviceId.length === 0 || serviceId.length > MAX_ID_LENGTH) continue
        const order = uniqueIds(savedOrder)
        if (order.length > 0) nodeOrderByService[serviceId] = order
      }
    }

    return {
      version: 1,
      collapsedServiceIds: uniqueIds(record.collapsedServiceIds),
      serviceOrder: uniqueIds(record.serviceOrder),
      nodeOrderByService,
    }
  } catch {
    return defaultNodesUiPreferences()
  }
}

function browserStorage(): PreferenceStorage | null {
  if (typeof window === 'undefined') return null
  try {
    return window.localStorage
  } catch {
    return null
  }
}

export function loadNodesUiPreferences(storage?: PreferenceStorage | null): NodesUiPreferences {
  const target = storage === undefined ? browserStorage() : storage
  if (!target) return defaultNodesUiPreferences()
  try {
    return parseNodesUiPreferences(target.getItem(NODES_UI_STORAGE_KEY))
  } catch {
    return defaultNodesUiPreferences()
  }
}

export function saveNodesUiPreferences(
  preferences: NodesUiPreferences,
  storage?: PreferenceStorage | null,
): boolean {
  const target = storage === undefined ? browserStorage() : storage
  if (!target) return false
  try {
    target.setItem(NODES_UI_STORAGE_KEY, JSON.stringify(preferences))
    return true
  } catch {
    return false
  }
}

export function applyNodeOrder<T extends { id: string }>(
  nodes: readonly T[],
  savedOrder: readonly string[] | undefined,
): T[] {
  if (!savedOrder || savedOrder.length === 0) return nodes.slice()

  const byId = new Map(nodes.map((node) => [node.id, node]))
  const placed = new Set<string>()
  const ordered: T[] = []
  for (const id of savedOrder) {
    const node = byId.get(id)
    if (!node || placed.has(id)) continue
    placed.add(id)
    ordered.push(node)
  }
  for (const node of nodes) {
    if (!placed.has(node.id)) ordered.push(node)
  }
  return ordered
}

export function sameNodeOrder(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((id, index) => id === right[index])
}

export function moveNodeByOffset(
  orderedIds: readonly string[],
  nodeId: string,
  offset: number,
): string[] {
  const from = orderedIds.indexOf(nodeId)
  if (from < 0) return orderedIds.slice()
  const to = Math.min(orderedIds.length - 1, Math.max(0, from + offset))
  if (to === from) return orderedIds.slice()
  const next = orderedIds.slice()
  const [moved] = next.splice(from, 1)
  next.splice(to, 0, moved)
  return next
}

export function placeNodeId(
  orderedIds: readonly string[],
  nodeId: string,
  targetId: string,
  placement: NodeDropPlacement,
): string[] {
  const from = orderedIds.indexOf(nodeId)
  if (from < 0 || nodeId === targetId || !orderedIds.includes(targetId)) {
    return orderedIds.slice()
  }
  const next = orderedIds.slice()
  next.splice(from, 1)
  const target = next.indexOf(targetId)
  if (target < 0) return orderedIds.slice()
  next.splice(target + (placement === 'after' ? 1 : 0), 0, nodeId)
  return next
}
