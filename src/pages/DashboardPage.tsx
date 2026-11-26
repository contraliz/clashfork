import { useEffect, useMemo, useRef, useState } from 'react'
import { api } from '../api'
import { useNowMs } from '../hooks'
import { RATE_WINDOW_SECONDS, useRateRecorderSnapshot } from '../lib/rateRecorder'
import {
  nextStableRateScaleIndex,
  rateChartGeometry,
  rateScaleFromIndex,
  rateScaleIndex,
} from '../lib/rateChart'
import { SelectedNodeLatency } from '../components/NodeLatency'
import { formatRate } from '../lib/format'
import type { Mode, Snapshot } from '../types'
import {
  Button,
  Banner,
  Field,
  Grid,
  GridItem,
  IconButton,
  Modal,
  PageHeader,
  Panel,
  PanelHeading,
  Toggle,
} from '../components/ui'

interface Props {
  snapshot: Snapshot
  onSnapshot: (snapshot: Snapshot) => void
  onError: (error: unknown) => void
}

export function DashboardPage({ snapshot, onSnapshot, onError }: Props) {
  const { state, runtime } = snapshot
  const now = useNowMs(1000)
  const selectedGlobalNode = state.nodes.find((node) => node.id === state.selection.globalNodeId)
  const { latest: live, points } = useRateRecorderSnapshot()
  const [busy, setBusy] = useState<string | null>(null)
  const [presetOpen, setPresetOpen] = useState(false)
  const [presetName, setPresetName] = useState('')
  const [pendingMode, setPendingMode] = useState<Mode | null>(null)
  const [pendingDestination, setPendingDestination] = useState('')
  const pendingGlobalNode = state.nodes.find((node) => node.id === pendingDestination)
  const missingSelection = useMemo(() => {
    if (state.selection.mode === 'rule') {
      return state.rulesets.length === 0
    }
    return state.nodes.length === 0
  }, [state.nodes.length, state.rulesets.length, state.selection.mode])

  const run = async (key: string, action: () => Promise<Snapshot>) => {
    setBusy(key)
    try {
      onSnapshot(await action())
    } catch (error) {
      onError(error)
      try {
        onSnapshot(await api.getSnapshot())
      } catch {
        // Keep the original failure visible.
      }
    } finally {
      setBusy(null)
    }
  }

  const changeSelection = (next: Partial<{ mode: Mode; rulesetId: string | null; globalNodeId: string | null }>) => {
    const selection = {
      mode: next.mode ?? state.selection.mode,
      rulesetId: next.rulesetId !== undefined ? next.rulesetId : state.selection.rulesetId,
      globalNodeId:
        next.globalNodeId !== undefined ? next.globalNodeId : state.selection.globalNodeId,
    }
    void run('selection', () => api.setSelection(selection))
  }

  const chooseMode = (mode: Mode) => {
    if (mode === state.selection.mode) return
    const hasDestination = mode === 'rule'
      ? state.rulesets.some((ruleset) => ruleset.id === state.selection.rulesetId)
      : state.nodes.some((node) => node.id === state.selection.globalNodeId && !node.missing)
    if (hasDestination) {
      changeSelection({ mode })
    } else {
      setPendingDestination('')
      setPendingMode(mode)
    }
  }

  const applyMode = () => {
    if (!pendingMode || !pendingDestination) return
    if (pendingMode === 'rule') {
      changeSelection({ mode: pendingMode, rulesetId: pendingDestination })
    } else {
      changeSelection({ mode: pendingMode, globalNodeId: pendingDestination })
    }
    setPendingMode(null)
  }

  const savePreset = async () => {
    if (!presetName.trim()) return
    setBusy('preset')
    try {
      onSnapshot(await api.savePreset(presetName.trim()))
      setPresetName('')
      setPresetOpen(false)
    } catch (error) {
      onError(error)
    } finally {
      setBusy(null)
    }
  }

  const nowSeconds = now / 1000
  const visiblePoints = useMemo(
    () => points.filter((point) => point.at <= nowSeconds + 1 && nowSeconds - point.at <= RATE_WINDOW_SECONDS),
    [nowSeconds, points],
  )
  const rawMax = useMemo(
    () => Math.max(0, ...visiblePoints.flatMap((point) => [point.download, point.upload])),
    [visiblePoints],
  )
  const desiredScaleIndex = useMemo(() => rateScaleIndex(rawMax), [rawMax])
  const [scaleIndex, setScaleIndex] = useState(desiredScaleIndex)
  const scaleChangedAt = useRef(nowSeconds)
  useEffect(() => {
    const next = nextStableRateScaleIndex(
      scaleIndex,
      desiredScaleIndex,
      nowSeconds - scaleChangedAt.current,
    )
    if (next !== scaleIndex) {
      scaleChangedAt.current = nowSeconds
      setScaleIndex(next)
    }
  }, [desiredScaleIndex, nowSeconds, scaleIndex])
  const chart = useMemo(
    () => rateChartGeometry(points, nowSeconds, rateScaleFromIndex(scaleIndex)),
    [nowSeconds, points, scaleIndex],
  )

  return (
    <div className="dashboard-page">
      <PageHeader title="Dashboard" />

      {runtime.competingTunnel ? (
        <Banner tone="warning">
          Another active tunnel adapter currently owns the selected outbound route. Pause it before connecting Clashfork. Clashfork has not changed system routing.
        </Banner>
      ) : null}
      {runtime.pendingReconnectServices.length > 0 ? (
        <Banner tone="warning">
          A selected node was updated by subscription refresh. The connected session keeps its current endpoint until you reconnect.
        </Banner>
      ) : null}

      <Grid className="dashboard-grid">
        <GridItem span={12}>
          <Panel className="vpn-panel" aria-labelledby="vpn-heading">
            <PanelHeading
              id="vpn-heading"
              kicker="VPN"
              title={busy === 'vpn' ? 'Connecting…' : runtime.vpnActive ? 'Connected' : 'Disconnected'}
              description={runtime.activeSummary ?? 'No active route'}
              actions={
                <Toggle
                  label="VPN connection"
                  checked={runtime.vpnActive}
                  disabled={busy !== null}
                  onChange={(enabled) => void run('vpn', () => api.setVpn(enabled))}
                />
              }
              className="vpn-heading"
            />
            <div className="vpn-divider" />
            <div className="control-row">
              <div className="field mode-field">
                <span className="field-label">Mode</span>
                <div className="segmented" role="group" aria-label="Routing mode">
                  {(['rule', 'global'] as const).map((mode) => (
                    <button
                      key={mode}
                      type="button"
                      className={state.selection.mode === mode ? 'is-selected' : ''}
                      aria-pressed={state.selection.mode === mode}
                      disabled={busy !== null}
                      onClick={() => chooseMode(mode)}
                    >
                      {mode === 'rule' ? 'Rule' : 'Global'}
                    </button>
                  ))}
                </div>
              </div>
              {state.selection.mode === 'rule' ? (
                <Field label="Ruleset" className="picker-field">
                  <select
                    aria-label="Ruleset"
                    value={state.selection.rulesetId ?? ''}
                    disabled={busy !== null || state.rulesets.length === 0}
                    onChange={(event) => changeSelection({ rulesetId: event.target.value || null })}
                  >
                    {state.rulesets.length === 0 ? <option value="">No rulesets yet</option> : null}
                    {state.rulesets.map((ruleset) => (
                      <option key={ruleset.id} value={ruleset.id}>
                        {ruleset.name}
                      </option>
                    ))}
                  </select>
                </Field>
              ) : (
                <Field label="Node" className="picker-field">
                  <select
                    aria-label="Node"
                    value={state.selection.globalNodeId ?? ''}
                    disabled={busy !== null || state.nodes.length === 0}
                    onChange={(event) => changeSelection({ globalNodeId: event.target.value || null })}
                  >
                    {selectedGlobalNode?.missing ? (
                      <option value={selectedGlobalNode.id} disabled>{selectedGlobalNode.displayName} · Missing</option>
                    ) : null}
                    <option value="">Select a node</option>
                    {groupNodesByService(state.nodes).map(([serviceId, nodes]) => (
                      <optgroup key={serviceId} label={nodes[0].serviceTitle}>
                        {nodes.filter((node) => !node.missing).map((node) => (
                          <option key={node.id} value={node.id}>
                            {node.displayName} · {node.protocol}
                          </option>
                        ))}
                      </optgroup>
                    ))}
                  </select>
                  <SelectedNodeLatency node={selectedGlobalNode} now={now} />
                </Field>
              )}
            </div>
            {missingSelection ? (
              <p className="inline-hint">
                {state.selection.mode === 'rule'
                  ? 'Create a ruleset in Rulesets before connecting.'
                  : 'Add nodes in Nodes before connecting in Global mode.'}
              </p>
            ) : null}
          </Panel>
        </GridItem>

        <GridItem span={12}>
          <Panel className="traffic-panel" aria-labelledby="traffic-heading">
            <PanelHeading
              id="traffic-heading"
              title="Proxy traffic"
              actions={
              <div className="chart-legend">
                  <span><i className="legend-download" /> Proxy download</span>
                  <span><i className="legend-upload" /> Proxy upload</span>
                </div>
              }
            />
            <div className="rate-row">
              <div className="rate-block">
                <span className="rate-label"><span className="rate-mark rate-mark-download" aria-hidden="true" />Proxy download</span>
                <strong>{runtime.vpnActive && live.available ? formatRate(live.proxyDownloadRate) : '— B/s'}</strong>
              </div>
              <div className="rate-divider" />
              <div className="rate-block">
                <span className="rate-label"><span className="rate-mark rate-mark-upload" aria-hidden="true" />Proxy upload</span>
                <strong className="is-upload">{runtime.vpnActive && live.available ? formatRate(live.proxyUploadRate) : '— B/s'}</strong>
              </div>
            </div>
            <div
              className="traffic-chart"
              role="img"
              aria-label="Recent proxy download and upload rates"
              data-rate-points={chart.points.length}
              data-rate-window={RATE_WINDOW_SECONDS}
              data-rate-scale={chart.scale}
              data-rate-groups={chart.groups}
              data-rate-first-x={chart.firstX.toFixed(1)}
              data-rate-last-x={chart.lastX.toFixed(1)}
            >
              <div className="chart-y-labels" aria-hidden="true">
                {chart.labels.map((label, index) => <span key={index}>{label}</span>)}
              </div>
              <div className="chart-body">
                <svg viewBox="0 0 820 150" preserveAspectRatio="none" aria-hidden="true">
                  <defs>
                    <linearGradient id="traffic-fill" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="0%" stopColor="#78a9ff" stopOpacity="0.35" />
                      <stop offset="100%" stopColor="#78a9ff" stopOpacity="0.02" />
                    </linearGradient>
                  </defs>
                  {[0, 1, 2, 3, 4].map((line) => (
                    <line key={line} x1="0" x2="820" y1={line * 37.5} y2={line * 37.5} className="chart-gridline" />
                  ))}
                  {chart.area ? <path d={chart.area} className="chart-area" fill="url(#traffic-fill)" /> : null}
                  <path d={chart.downloadPath} className="chart-line chart-download" fill="none" />
                  <path d={chart.uploadPath} className="chart-line chart-upload" fill="none" />
                  {chart.latestPoint ? (
                    <>
                      <circle cx={chart.latestPoint.download.x} cy={chart.latestPoint.download.y} r="4" className="chart-point chart-point-download" />
                      <circle cx={chart.latestPoint.upload.x} cy={chart.latestPoint.upload.y} r="4" className="chart-point chart-point-upload" />
                    </>
                  ) : null}
                </svg>
                <div className="chart-x-labels" aria-hidden="true">
                  <span>60s ago</span>
                  <span>now</span>
                </div>
              </div>
            </div>
          </Panel>
        </GridItem>

        <GridItem span={12}>
          <section className="run-configs" aria-labelledby="presets-heading">
            <div className="section-heading">
              <h2 id="presets-heading">Run configurations</h2>
              <Button variant="primary" icon="plus" disabled={busy !== null} onClick={() => setPresetOpen(true)}>
                Save current
              </Button>
            </div>
            <Panel className="preset-panel">
              {state.presets.length === 0 ? (
                <div className="preset-empty">
                  <div>
                    <h3>No saved configurations</h3>
                    <p>Save the current mode and destination to recall it here or from the tray.</p>
                  </div>
                </div>
              ) : (
                <ul className="preset-list">
                  {state.presets.map((preset) => {
                    const destination =
                      preset.mode === 'global'
                        ? state.nodes.find((node) => node.id === preset.globalNodeId)
                        : null
                    const summary =
                      preset.mode === 'global'
                        ? destination
                          ? `${destination.serviceTitle} / ${destination.displayName}`
                          : 'Node unavailable'
                        : state.rulesets.find((ruleset) => ruleset.id === preset.rulesetId)?.name ?? 'Ruleset unavailable'
                    return (
                      <li key={preset.id}>
                        <div className="preset-name">
                          <strong>{preset.name}</strong>
                          <span>{preset.mode === 'global' ? 'Global' : 'Rule'} · {summary}</span>
                        </div>
                        <div className="preset-actions">
                          <Button
                            variant="secondary"
                            icon="play"
                            disabled={busy !== null}
                            onClick={() => void run(`preset-${preset.id}`, () => api.applyPreset(preset.id))}
                          >
                            Apply
                          </Button>
                          <IconButton
                            icon="trash"
                            label={`Delete ${preset.name}`}
                            disabled={busy !== null}
                            onClick={() => void run(`delete-${preset.id}`, () => api.deletePreset(preset.id))}
                          />
                        </div>
                      </li>
                    )
                  })}
                </ul>
              )}
            </Panel>
          </section>
        </GridItem>
      </Grid>

      {pendingMode ? (
        <Modal
          title={pendingMode === 'rule' ? 'Choose a ruleset' : 'Choose a Global node'}
          onClose={() => setPendingMode(null)}
          footer={
            <>
              <Button onClick={() => setPendingMode(null)}>Cancel</Button>
              <Button variant="primary" disabled={!pendingDestination} onClick={applyMode}>Apply</Button>
            </>
          }
        >
          <Field label={pendingMode === 'rule' ? 'Ruleset' : 'Node'}>
            <select
              data-autofocus
              aria-label={pendingMode === 'rule' ? 'Ruleset' : 'Node'}
              value={pendingDestination}
              onChange={(event) => setPendingDestination(event.target.value)}
            >
              <option value="">{pendingMode === 'rule' ? 'Select a ruleset' : 'Select a node'}</option>
              {pendingMode === 'rule'
                ? state.rulesets.map((ruleset) => (
                    <option key={ruleset.id} value={ruleset.id}>{ruleset.name}</option>
                  ))
                : groupNodesByService(state.nodes.filter((node) => !node.missing)).map(([serviceId, nodes]) => (
                    <optgroup key={serviceId} label={nodes[0].serviceTitle}>
                      {nodes.map((node) => <option key={node.id} value={node.id}>{node.displayName}</option>)}
                    </optgroup>
                  ))}
            </select>
          </Field>
          {pendingMode === 'global' ? <SelectedNodeLatency node={pendingGlobalNode} now={now} /> : null}
          {pendingMode === 'rule' && state.rulesets.length === 0 ? (
            <p className="field-hint">Create a ruleset on the Rulesets page.</p>
          ) : null}
          {pendingMode === 'global' && !state.nodes.some((node) => !node.missing) ? (
            <p className="field-hint">Add a node on the Nodes page.</p>
          ) : null}
        </Modal>
      ) : null}

      {presetOpen ? (
        <Modal
          title="Save run configuration"
          description="This captures the current mode, ruleset, and Global node."
          onClose={() => setPresetOpen(false)}
          footer={
            <>
              <Button onClick={() => setPresetOpen(false)}>Cancel</Button>
              <Button variant="primary" busy={busy === 'preset'} onClick={() => void savePreset()}>
                Save configuration
              </Button>
            </>
          }
        >
          <Field label="Name">
            <input
              value={presetName}
              data-autofocus
              maxLength={80}
              placeholder="AI"
              onChange={(event) => setPresetName(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') void savePreset()
              }}
            />
          </Field>
        </Modal>
      ) : null}

      {runtime.lastError ? (
        <p className="sr-only" role="status">
          {runtime.lastError}
        </p>
      ) : null}
    </div>
  )
}

function groupNodesByService(nodes: Snapshot['state']['nodes']): Array<[string, Snapshot['state']['nodes']]> {
  const grouped = new Map<string, Snapshot['state']['nodes']>()
  for (const node of nodes) {
    const list = grouped.get(node.serviceId) ?? []
    list.push(node)
    grouped.set(node.serviceId, list)
  }
  return [...grouped.entries()]
}
