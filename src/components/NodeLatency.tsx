import type { NodeView } from '../types'

export function LatencyValue({ node, now }: { node: NodeView | undefined; now: number }) {
  if (!node || node.latencyMs == null) {
    return <span className="latency-value latency-unknown" aria-label="Latency not measured" data-latency-ms="">—</span>
  }
  const stale =
    node.latencyMeasuredAtMs == null || now - node.latencyMeasuredAtMs >= 15 * 60 * 1000
  const label = stale ? `Stale latency ${node.latencyMs} milliseconds` : `${node.latencyMs} milliseconds`
  const attributes = {
    className: `latency-value${stale ? ' latency-stale' : ''}`,
    'aria-label': label,
    'data-latency-ms': String(node.latencyMs),
    'data-latency-stale': stale ? 'true' : 'false',
  }
  return stale ? (
    <del {...attributes}>{node.latencyMs} ms</del>
  ) : (
    <span {...attributes}>{node.latencyMs} ms</span>
  )
}

export function SelectedNodeLatency({ node, now }: { node: NodeView | undefined; now: number }) {
  // The node table keeps an unknown placeholder, but a selected-node summary is
  // only useful once a measurement exists. Rendering an empty dash here adds a
  // line under pickers without conveying any state.
  if (!node || node.latencyMs == null) return null
  return (
    <div className="selected-node-latency">
      <span className="sr-only">Selected node latency: </span>
      <LatencyValue node={node} now={now} />
    </div>
  )
}
