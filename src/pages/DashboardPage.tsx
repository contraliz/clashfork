import { useMemo, useState } from 'react'
import { api } from '../api'
import { useLive, useRatePoints } from '../hooks'
import { formatBytes, formatRate } from '../lib/format'
import type { Mode, Snapshot } from '../types'
import { Button, Field, IconButton, Modal, PageHeader, Toggle } from '../components/ui'

interface Props {
  snapshot: Snapshot
  onSnapshot: (snapshot: Snapshot) => void
  onError: (error: unknown) => void
}

export function DashboardPage({ snapshot, onSnapshot, onError }: Props) {
  const { state, runtime } = snapshot
  const live = useLive(true)
  const points = useRatePoints(live)
  const [busy, setBusy] = useState<string | null>(null)
  const [presetOpen, setPresetOpen] = useState(false)
  const [presetName, setPresetName] = useState('')
  const [pendingMode, setPendingMode] = useState<Mode | null>(null)
  const [pendingDestination, setPendingDestination] = useState('')
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

  const chart = useMemo(() => {
    const rawMax = Math.max(0, ...points.flatMap((point) => [point.download, point.upload]))
    const max = Math.max(rawMax, 1024 * 1024)
    const width = 820
    const height = 150
    const path = (key: 'download' | 'upload') => {
      if (points.length === 0) return `M 0 ${height}`
      return points
        .map((point, index) => {
          const x = (index / Math.max(1, points.length - 1)) * width
          const y = height - (point[key] / max) * (height - 10)
          return `${index === 0 ? 'M' : 'L'} ${x.toFixed(1)} ${y.toFixed(1)}`
        })
        .join(' ')
    }
    const area = `${path('download')} L ${width} ${height} L 0 ${height} Z`
    const labels = [1, 0.75, 0.5, 0.25, 0].map((fraction) => formatBytes(max * fraction, 0) + '/s')
    return { path, area, labels, max }
  }, [points])

  return (
    <div className="dashboard-page">
      <PageHeader title="Dashboard" />

      <section className="panel vpn-panel" aria-labelledby="vpn-heading">
        <div className="vpn-head">
          <div>
            <div className="panel-kicker">VPN</div>
            <h2 id="vpn-heading">{busy === 'vpn' ? 'Connecting…' : runtime.vpnActive ? 'Connected' : 'Disconnected'}</h2>
            {runtime.activeSummary ? <p className="muted">{runtime.activeSummary}</p> : null}
          </div>
          <Toggle
            label="VPN connection"
            checked={runtime.vpnActive}
            disabled={busy !== null}
            onChange={(enabled) => void run('vpn', () => api.setVpn(enabled))}
          />
        </div>
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
      </section>

      <section className="panel traffic-panel" aria-labelledby="traffic-heading">
        <div className="panel-heading">
          <h2 id="traffic-heading">Live traffic</h2>
        </div>
        <div className="rate-row">
          <div className="rate-block">
            <span className="rate-label">Download</span>
            <strong>{runtime.vpnActive ? formatRate(live.downloadRate) : '— B/s'}</strong>
          </div>
          <div className="rate-divider" />
          <div className="rate-block">
            <span className="rate-label">Upload</span>
            <strong>{runtime.vpnActive ? formatRate(live.uploadRate) : '— B/s'}</strong>
          </div>
        </div>
        <div className="traffic-chart" role="img" aria-label="Recent download and upload rates">
          <div className="chart-y-labels" aria-hidden="true">
            {chart.labels.map((label, index) => <span key={index}>{label}</span>)}
          </div>
          <div className="chart-body">
            <svg viewBox="0 0 820 150" preserveAspectRatio="none" aria-hidden="true">
              <defs>
                <linearGradient id="traffic-fill" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="#8ecdb0" stopOpacity="0.45" />
                  <stop offset="100%" stopColor="#8ecdb0" stopOpacity="0.03" />
                </linearGradient>
              </defs>
              {[0, 1, 2, 3, 4].map((line) => (
                <line key={line} x1="0" x2="820" y1={line * 37.5} y2={line * 37.5} className="chart-gridline" />
              ))}
              {points.length > 0 ? <path d={chart.area} fill="url(#traffic-fill)" /> : null}
              <path d={chart.path('download')} className="chart-line chart-download" fill="none" />
              <path d={chart.path('upload')} className="chart-line chart-upload" fill="none" />
            </svg>
            <div className="chart-x-labels" aria-hidden="true">
              <span>60s ago</span>
              <span>now</span>
            </div>
          </div>
        </div>
      </section>

      <section className="run-configs" aria-labelledby="presets-heading">
        <div className="section-heading">
          <h2 id="presets-heading">Run configurations</h2>
          <Button variant="primary" icon="plus" disabled={busy !== null} onClick={() => setPresetOpen(true)}>
            Save current
          </Button>
        </div>
        <div className="panel preset-panel">
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
        </div>
      </section>

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
