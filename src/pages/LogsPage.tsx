import { useEffect, useMemo, useState } from 'react'
import { api } from '../api'
import { formatBytes, formatTime, prettyDateRange } from '../lib/format'
import type { CustomAppGroup, DiagnosticEntry, HistoryReport, Snapshot } from '../types'
import { Icon } from '../components/Icon'
import {
  Button,
  EmptyState,
  Field,
  IconButton,
  Modal,
  PageHeader,
} from '../components/ui'

interface Props {
  snapshot: Snapshot
  onSnapshot: (snapshot: Snapshot) => void
  onError: (error: unknown) => void
}

type Range = 'today' | '7d' | '30d'
type Breakdown = 'app' | 'customGroup' | 'route'

export function LogsPage({ snapshot, onSnapshot, onError }: Props) {
  const [tab, setTab] = useState<'history' | 'diagnostics'>('history')
  const [range, setRange] = useState<Range>('7d')
  const [breakdown, setBreakdown] = useState<Breakdown>('app')
  const [report, setReport] = useState<HistoryReport | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [clearOpen, setClearOpen] = useState(false)
  const [groupOpen, setGroupOpen] = useState(false)
  const [diagnostics, setDiagnostics] = useState<DiagnosticEntry[]>([])

  const loadReport = async () => {
    setBusy('history')
    try {
      setReport(await api.getHistory({ range, breakdown }))
    } catch (error) {
      onError(error)
    } finally {
      setBusy(null)
    }
  }

  useEffect(() => {
    void loadReport()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [range, breakdown, snapshot.runtime.vpnActive, snapshot.state.historyPaused])

  useEffect(() => {
    if (tab !== 'diagnostics') return
    let cancelled = false
    const load = async () => {
      try {
        const entries = await api.getDiagnostics()
        if (!cancelled) setDiagnostics(entries)
      } catch (error) {
        if (!cancelled) onError(error)
      }
    }
    void load()
    const timer = window.setInterval(() => void load(), 2000)
    return () => {
      cancelled = true
      window.clearInterval(timer)
    }
  }, [onError, tab])

  const max = useMemo(
    () => Math.max(1, ...(report?.bars.flatMap((bar) => [bar.download, bar.upload]) ?? [1])),
    [report],
  )

  return (
    <div>
      <PageHeader title="Logs" />

      <div className="tabs" role="tablist" aria-label="Logs sections">
        <button type="button" role="tab" aria-selected={tab === 'history'} className={tab === 'history' ? 'is-active' : ''} onClick={() => setTab('history')}>
          Traffic History
        </button>
        <button type="button" role="tab" aria-selected={tab === 'diagnostics'} className={tab === 'diagnostics' ? 'is-active' : ''} onClick={() => setTab('diagnostics')}>
          Diagnostics
        </button>
      </div>

      {tab === 'history' ? (
        <div className="history-view">
          <div className="history-controls">
            <div className="range-tabs" role="group" aria-label="History range">
              {(['today', '7d', '30d'] as const).map((option) => (
                <button key={option} type="button" className={range === option ? 'is-selected' : ''} aria-pressed={range === option} onClick={() => setRange(option)}>
                  {option === 'today' ? 'Today' : option === '7d' ? '7 days' : '30 days'}
                </button>
              ))}
            </div>
            <div className="history-actions">
              <Button
                variant="secondary"
                icon={snapshot.state.historyPaused ? 'play' : 'pause'}
                busy={busy === 'pause'}
                onClick={async () => {
                  setBusy('pause')
                  try {
                    onSnapshot(await api.setHistoryPaused(!snapshot.state.historyPaused))
                  } catch (error) {
                    onError(error)
                  } finally {
                    setBusy(null)
                  }
                }}
              >
                {snapshot.state.historyPaused ? 'Resume' : 'Pause'}
              </Button>
              <Button variant="secondary" icon="trash" onClick={() => setClearOpen(true)}>Clear history</Button>
            </div>
          </div>

          {snapshot.state.historyPaused ? <div className="paused-note">History recording is paused. Existing data remains available.</div> : null}

          <div className="history-total">
            <strong>{report ? formatBytes(report.total) : '—'}</strong>
            <span>Total transferred</span>
            {report ? <span className="muted">{prettyDateRange(report.from, report.to)}</span> : null}
          </div>

          <section className="panel chart-panel" aria-labelledby="traffic-history-heading">
            <div className="chart-panel-head">
              <h2 id="traffic-history-heading" className="sr-only">Traffic history chart</h2>
              <div className="chart-legend">
                <span><i className="legend-download" /> Download</span>
                <span><i className="legend-upload" /> Upload</span>
              </div>
            </div>
            {report ? (
              <div className="bar-chart-wrap">
                <div className="bar-y-axis" aria-hidden="true">
                  {[1, 0.75, 0.5, 0.25, 0].map((fraction) => <span key={fraction}>{formatBytes(max * fraction)}</span>)}
                </div>
                <div className="bar-chart-scroll">
                  <div className="bar-chart" style={{ minWidth: `${Math.max(520, report.bars.length * (range === '30d' ? 26 : 58))}px` }}>
                    {report.bars.map((bar) => (
                      <div className="bar-group" key={bar.start} title={`${bar.label}: ↓${formatBytes(bar.download)} ↑${formatBytes(bar.upload)}`}>
                        <div className="bar-stack">
                          <span className="bar bar-download" style={{ height: `${(bar.download / max) * 100}%` }} />
                          <span className="bar bar-upload" style={{ height: `${(bar.upload / max) * 100}%` }} />
                        </div>
                        <span className={`bar-label ${bar.gap ? 'has-gap' : ''}`}>{bar.label}{bar.gap ? ' ·' : ''}</span>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            ) : (
              <div className="chart-loading" aria-live="polite">Loading traffic history…</div>
            )}
            {report && report.gapMinutes > 0 ? <p className="gap-note"><Icon name="alert" size={15} /> {report.gapMinutes} minute{report.gapMinutes === 1 ? '' : 's'} with a data gap</p> : null}
          </section>

          <div className="breakdown-row">
            <label>
              <span className="sr-only">Breakdown</span>
              <select value={breakdown} onChange={(event) => setBreakdown(event.target.value as Breakdown)}>
                <option value="app">By app</option>
                <option value="customGroup">By custom group</option>
                <option value="route">By route</option>
              </select>
            </label>
            <Button variant="secondary" icon="edit" onClick={() => setGroupOpen(true)}>Manage app groups</Button>
          </div>

          <section className="panel history-table-panel">
            {report ? (
              <table className="history-table">
                <thead>
                  <tr>
                    <th scope="col">{breakdown === 'route' ? 'Route' : breakdown === 'customGroup' ? 'Group' : 'App'}</th>
                    <th scope="col" className="numeric">Download</th>
                    <th scope="col" className="numeric">Upload</th>
                    <th scope="col" className="numeric">Transferred</th>
                  </tr>
                </thead>
                <tbody>
                  {report.rows.map((row) => (
                    <tr key={`${row.label}-${row.app ?? ''}-${row.route ?? ''}-${row.routeClass}`}>
                      <td>
                        <strong>{row.label}</strong>
                        {row.app && row.app !== row.label ? <span className="sub-label">{row.app}</span> : null}
                        <span className={`route-class route-class-${row.routeClass}`}>{row.routeClass[0].toUpperCase() + row.routeClass.slice(1)}</span>
                      </td>
                      <td className="numeric">{formatBytes(row.download)}</td>
                      <td className="numeric">{formatBytes(row.upload)}</td>
                      <td className="numeric"><strong>{formatBytes(row.total)}</strong></td>
                    </tr>
                  ))}
                  {report.rows.length === 0 ? <tr><td colSpan={4} className="table-empty">No traffic has been recorded in this range.</td></tr> : null}
                </tbody>
              </table>
            ) : (
              <EmptyState title="No report yet" description="Traffic history loads from the local SQLite database." />
            )}
          </section>
        </div>
      ) : (
        <section className="panel diagnostics-panel">
          <div className="diagnostics-head">
            <div>
              <h2>Live diagnostics</h2>
              <p>Bounded application and core messages. Credentials, URLs, and query tokens are redacted.</p>
            </div>
            <Button variant="secondary" icon="refresh" onClick={() => void api.getDiagnostics().then(setDiagnostics).catch(onError)}>Refresh</Button>
          </div>
          <div className="diagnostics-log" role="log" aria-live="polite">
            {diagnostics.length === 0 ? <p className="muted">No diagnostics yet.</p> : null}
            {diagnostics.map((entry) => (
              <div className={`diagnostic-line level-${entry.level}`} key={entry.id}>
                <span className="diag-time">{formatTime(entry.at)}</span>
                <span className={`diag-source source-${entry.source}`}>{entry.source}</span>
                <span className="diag-message">{entry.message}</span>
              </div>
            ))}
          </div>
        </section>
      )}

      {clearOpen ? (
        <Modal
          title="Clear traffic history"
          description="This permanently deletes the local minute aggregates and custom-group results. It does not change your VPN or subscriptions."
          onClose={() => setClearOpen(false)}
          footer={
            <>
              <Button onClick={() => setClearOpen(false)}>Cancel</Button>
              <Button
                variant="danger"
                busy={busy === 'clear'}
                onClick={async () => {
                  setBusy('clear')
                  try {
                    onSnapshot(await api.clearHistory(true))
                    setClearOpen(false)
                    await loadReport()
                  } catch (error) {
                    onError(error)
                  } finally {
                    setBusy(null)
                  }
                }}
              >
                Clear history
              </Button>
            </>
          }
        >
          <p>Traffic totals and app/route rows older than the current view are deleted as well.</p>
        </Modal>
      ) : null}

      {groupOpen ? (
        <CustomGroupsModal
          groups={snapshot.state.customAppGroups}
          onClose={() => setGroupOpen(false)}
          onError={onError}
          onSaved={async (next) => {
            onSnapshot(next)
            await loadReport()
          }}
        />
      ) : null}
    </div>
  )
}

function CustomGroupsModal({
  groups,
  onClose,
  onSaved,
  onError,
}: {
  groups: CustomAppGroup[]
  onClose: () => void
  onSaved: (snapshot: Snapshot) => Promise<void>
  onError: (error: unknown) => void
}) {
  const [editing, setEditing] = useState<CustomAppGroup | null>(null)
  const [name, setName] = useState('')
  const [apps, setApps] = useState('')
  const [busy, setBusy] = useState(false)

  const startEdit = (group: CustomAppGroup | null) => {
    setEditing(group)
    setName(group?.name ?? '')
    setApps(group?.apps.join(', ') ?? '')
  }

  return (
    <Modal
      title="Custom app groups"
      description="Group historical app names for reporting. Changes apply retroactively to locally stored minutes."
      onClose={onClose}
      wide
      footer={<Button onClick={onClose}>Done</Button>}
    >
      <div className="group-manager">
        <div className="group-list">
          {groups.length === 0 ? <p className="muted">No custom groups yet.</p> : null}
          {groups.map((group) => (
            <div className="group-list-row" key={group.id}>
              <div><strong>{group.name}</strong><span>{group.apps.join(', ')}</span></div>
              <div>
                <IconButton icon="edit" label={`Edit ${group.name}`} onClick={() => startEdit(group)} />
                <IconButton
                  icon="trash"
                  label={`Delete ${group.name}`}
                  onClick={async () => {
                    try {
                      await onSaved(await api.deleteCustomGroup(group.id))
                    } catch (error) {
                      onError(error)
                    }
                  }}
                />
              </div>
            </div>
          ))}
        </div>
        <div className="group-form">
          <h3>{editing ? 'Edit group' : 'New group'}</h3>
          <Field label="Group name"><input value={name} maxLength={80} onChange={(event) => setName(event.target.value)} /></Field>
          <Field label="Apps" hint="Comma-separated process basenames, for example ChatGPT.exe, codex.exe.">
            <textarea rows={3} value={apps} onChange={(event) => setApps(event.target.value)} />
          </Field>
          <div className="modal-inline-actions">
            {editing ? <Button onClick={() => startEdit(null)}>Cancel edit</Button> : null}
            <Button
              variant="primary"
              busy={busy}
              onClick={async () => {
                setBusy(true)
                try {
                  await onSaved(await api.saveCustomGroup({
                    id: editing?.id ?? '',
                    name,
                    apps: apps.split(',').map((app) => app.trim()).filter(Boolean),
                  }))
                  startEdit(null)
                } catch (error) {
                  onError(error)
                } finally {
                  setBusy(false)
                }
              }}
            >
              Save group
            </Button>
          </div>
        </div>
      </div>
    </Modal>
  )
}
