import { describe, expect, it } from 'vitest'
import {
  applyNodeOrder,
  defaultNodesUiPreferences,
  loadNodesUiPreferences,
  moveNodeByOffset,
  NODES_UI_STORAGE_KEY,
  parseNodesUiPreferences,
  placeNodeId,
  sameNodeOrder,
  saveNodesUiPreferences,
  type PreferenceStorage,
} from './nodesUiState'

const nodes = [
  { id: 'a', name: 'Alpha' },
  { id: 'b', name: 'Beta' },
  { id: 'c', name: 'Gamma' },
]

function memoryStorage(initial: Record<string, string> = {}): PreferenceStorage & { values: Record<string, string> } {
  return {
    values: { ...initial },
    getItem(key) {
      return this.values[key] ?? null
    },
    setItem(key, value) {
      this.values[key] = value
    },
  }
}

describe('nodes UI preferences', () => {
  it('defaults when storage is missing or malformed', () => {
    expect(loadNodesUiPreferences(null)).toEqual(defaultNodesUiPreferences())
    expect(parseNodesUiPreferences(null)).toEqual(defaultNodesUiPreferences())
    expect(parseNodesUiPreferences('{')).toEqual(defaultNodesUiPreferences())
    expect(parseNodesUiPreferences(JSON.stringify({ version: 2, collapsedServiceIds: ['service'] })))
      .toEqual(defaultNodesUiPreferences())
    expect(parseNodesUiPreferences(JSON.stringify({ version: 1, collapsedServiceIds: [1, '', 'service'], nodeOrderByService: [] })))
      .toEqual({ version: 1, collapsedServiceIds: ['service'], serviceOrder: [], nodeOrderByService: {} })
  })

  it('loads existing version 1 order and collapse without a group order', () => {
    const legacy = JSON.stringify({
      version: 1,
      collapsedServiceIds: ['service_a'],
      nodeOrderByService: { service_a: ['b', 'a'] },
    })
    expect(parseNodesUiPreferences(legacy)).toEqual({
      version: 1,
      collapsedServiceIds: ['service_a'],
      serviceOrder: [],
      nodeOrderByService: { service_a: ['b', 'a'] },
    })
  })

  it('deduplicates stored IDs and round-trips versioned preferences', () => {
    const storage = memoryStorage()
    const preferences = {
      version: 1 as const,
      collapsedServiceIds: ['service_a', 'service_a'],
      serviceOrder: ['service_b', 'service_b', 'service_a'],
      nodeOrderByService: {
        service_a: ['b', 'a'],
        service_empty: [],
      },
    }
    expect(parseNodesUiPreferences(JSON.stringify(preferences))).toEqual({
      version: 1,
      collapsedServiceIds: ['service_a'],
      serviceOrder: ['service_b', 'service_a'],
      nodeOrderByService: { service_a: ['b', 'a'] },
    })
    expect(saveNodesUiPreferences(preferences, storage)).toBe(true)
    expect(Object.keys(storage.values)).toContain(NODES_UI_STORAGE_KEY)
    expect(loadNodesUiPreferences(storage)).toEqual({
      version: 1,
      collapsedServiceIds: ['service_a'],
      serviceOrder: ['service_b', 'service_a'],
      nodeOrderByService: { service_a: ['b', 'a'] },
    })
  })

  it('retains surviving saved order, appends new nodes in provider order, and ignores stale IDs', () => {
    expect(applyNodeOrder(nodes, ['c', 'stale', 'a']).map((node) => node.id))
      .toEqual(['c', 'a', 'b'])
    expect(applyNodeOrder(nodes, ['a', 'a', 'c']).map((node) => node.id))
      .toEqual(['a', 'c', 'b'])
    expect(applyNodeOrder(nodes, undefined).map((node) => node.id))
      .toEqual(['a', 'b', 'c'])
  })

  it('applies saved group order with the same survivor and append semantics', () => {
    const services = [{ id: 'service_a' }, { id: 'service_b' }, { id: 'service_c' }]
    expect(applyNodeOrder(services, ['service_b', 'stale', 'service_a']).map((service) => service.id))
      .toEqual(['service_b', 'service_a', 'service_c'])
  })

  it('moves a node up and down and clamps at the first and last positions', () => {
    expect(moveNodeByOffset(['a', 'b', 'c'], 'b', -1)).toEqual(['b', 'a', 'c'])
    expect(moveNodeByOffset(['a', 'b', 'c'], 'b', 1)).toEqual(['a', 'c', 'b'])
    expect(moveNodeByOffset(['a', 'b', 'c'], 'a', -1)).toEqual(['a', 'b', 'c'])
    expect(moveNodeByOffset(['a', 'b', 'c'], 'c', 1)).toEqual(['a', 'b', 'c'])
  })

  it('places a node before or after a target across the full list', () => {
    expect(placeNodeId(['a', 'b', 'c'], 'a', 'c', 'after')).toEqual(['b', 'c', 'a'])
    expect(placeNodeId(['a', 'b', 'c'], 'c', 'a', 'before')).toEqual(['c', 'a', 'b'])
    expect(placeNodeId(['a', 'b', 'c'], 'c', 'b', 'before')).toEqual(['a', 'c', 'b'])
    expect(placeNodeId(['a', 'b', 'c'], 'b', 'a', 'after')).toEqual(['a', 'b', 'c'])
    expect(placeNodeId(['a', 'b', 'c'], 'a', 'missing', 'before')).toEqual(['a', 'b', 'c'])
    expect(placeNodeId(['a', 'b', 'c'], 'missing', 'a', 'before')).toEqual(['a', 'b', 'c'])
    expect(sameNodeOrder(['a', 'b'], ['a', 'b'])).toBe(true)
    expect(sameNodeOrder(['a', 'b'], ['b', 'a'])).toBe(false)
  })
})
