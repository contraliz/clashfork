import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { api } from '../api'
import { formatBytes, formatTime, prettyDateRange } from '../lib/format'
import { sortDiagnosticsNewestFirst } from '../lib/diagnostics'
import { isDocumentVisible, useDocumentVisible } from '../hooks'
import {
  resetLogsFilters,
  updateLogsSession,
  useLogsSession,
  type HistoryBreakdown,
  type HistoryRange,
  type HistoryRouteFilter,
} from '../lib/logsSession'
import type {
  CustomAppGroup,
  DiagnosticEntry,
  HistoryBar,
  HistoryReport,
  HistorySubjectFilter,
  Snapshot,
} from '../types'
import { Icon } from '../components/Icon'
import {
  Button,
  EmptyState,
  Field,
  Grid,
  GridItem,
  IconButton,
  Modal,
  PageHeader,
  Panel,
  PanelHeading,
  Toggle,
  Toolbar,
} from '../components/ui'

interface Props {
  snapshot: Snapshot
  onSnapshot: (snapshot: Snapshot) => void
  onError: (error: unknown) => void
}

function historySubjectValue(filter: HistorySubjectFilter | null): string {
  return filter ? JSON.stringify(filter) : 'all'
}

function historySubjectFromValue(value: string): HistorySubjectFilter | null {
  if (value === 'all') return null
  try {
    const parsed = JSON.parse(value) as Partial<HistorySubjectFilter>
    if (parsed.kind === 'app' && typeof parsed.name === 'string') {
      return { kind: 'app', name: parsed.name }
    }
    if (parsed.kind === 'group' && typeof parsed.id === 'string') {
      return { kind: 'group', id: parsed.id }
    }
  } catch {
    return null
  }
  return null
}

function historyPeriodLabel(bar: HistoryBar, range: HistoryRange): string {
  const date = new Date(bar.start * 1000)
  const options: Intl.DateTimeFormatOptions = range === 'today'
    ? { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }
    : { year: 'numeric', month: 'short', day: 'numeric' }
  return new Intl.DateTimeFormat(undefined, options).format(date)
}

export function LogsPage({ snapshot, onSnapshot, onError }: Props) {
  const visible = useDocumentVisible()
  const { tab, range, breakdown, routeFilter, subjectFilter } = useLogsSession()
  const setTab = (next: 'history' | 'diagnostics') => updateLogsSession({ tab: next })
  const setRange = (next: HistoryRange) => updateLogsSession({ range: next })
  const setBreakdown = (next: HistoryBreakdown) => updateLogsSession({ breakdown: next })
  const setRouteFilter = (next: HistoryRouteFilter) => updateLogsSession({ routeFilter: next })
  const setSubjectFilter = (next: HistorySubjectFilter | null) => updateLogsSession({ subjectFilter: next })
  const [historyRevision, setHistoryRevision] = useState(0)
  const [report, setReport] = useState<HistoryReport | null>(null)
  const [availableApps, setAvailableApps] = useState<string[]>([])
  const [inspectedPeriod, setInspectedPeriod] = useState<HistoryBar | null>(null)
  const [chartWidth, setChartWidth] = useState(0)
  const chartRef = useRef<HTMLDivElement | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [clearOpen, setClearOpen] = useState(false)
  const [clearConfirmed, setClearConfirmed] = useState(false)
  const [clearPending, setClearPending] = useState(false)
  const clearPendingRef = useRef(false)
  const [groupOpen, setGroupOpen] = useState(false)
  const [diagnostics, setDiagnostics] = useState<DiagnosticEntry[]>([])

  useEffect(() => {
    if (!visible || !isDocumentVisible()) return
    let cancelled = false
    setReport(null)
    setInspectedPeriod(null)
    setBusy('history')
    void api.getHistory({
      range,
      breakdown,
      routeFilter: routeFilter === 'all' ? null : routeFilter,
      subjectFilter,
    }).then((nextReport) => {
      if (cancelled || !isDocumentVisible()) return
      setReport(nextReport)
      setAvailableApps(nextReport.availableApps)
      if (
        subjectFilter?.kind === 'app' &&
        !nextReport.availableApps.some((app) => app.localeCompare(subjectFilter.name, undefined, { sensitivity: 'accent' }) === 0)
      ) {
        setSubjectFilter(null)
      }
      if (
        subjectFilter?.kind === 'group' &&
        !snapshot.state.customAppGroups.some((group) => group.id === subjectFilter.id && group.apps.length > 0)
      ) {
        setSubjectFilter(null)
      }
    }).catch((error: unknown) => {
      if (!cancelled && isDocumentVisible()) onError(error)
    }).finally(() => {
      if (!cancelled && isDocumentVisible()) setBusy((current) => current === 'history' ? null : current)
    })
    return () => {
      cancelled = true
    }
  }, [
    breakdown,
    onError,
    range,
    visible,
    routeFilter,
    historyRevision,
    snapshot.runtime.vpnActive,
    snapshot.state.customAppGroups,
    snapshot.state.historyPaused,
    subjectFilter,
  ])

  useEffect(() => {
    if (tab !== 'diagnostics' || !visible || !isDocumentVisible()) return
    let cancelled = false
    let timer: number | undefined
    const load = async () => {
      if (cancelled || !isDocumentVisible()) return
      try {
        const entries = await api.getDiagnostics()
        if (!cancelled && isDocumentVisible()) setDiagnostics(entries)
      } catch (error) {
        if (!cancelled && isDocumentVisible()) onError(error)
      }
      if (!cancelled && isDocumentVisible()) timer = window.setTimeout(() => void load(), 2000)
    }
    void load()
    return () => {
      cancelled = true
      if (timer) window.clearTimeout(timer)
    }
  }, [onError, tab, visible])

  useLayoutEffect(() => {
    const node = chartRef.current
    if (!node) return
    const observer = new ResizeObserver(([entry]) => {
      setChartWidth(entry.contentRect.width)
    })
    observer.observe(node)
    setChartWidth(node.getBoundingClientRect().width)
    return () => observer.disconnect()
  }, [report, tab])

  const max = useMemo(
    () => Math.max(1, ...(report?.bars.flatMap((bar) => [bar.proxyTotal, bar.directTotal]) ?? [1])),
    [report],
  )
  const eligibleGroups = useMemo(
    () => snapshot.state.customAppGroups.filter((group) => group.apps.length > 0),
    [snapshot.state.customAppGroups],
  )
  const selectedAppOption = subjectFilter?.kind === 'app'
    && !availableApps.some((app) => app.localeCompare(subjectFilter.name, undefined, { sensitivity: 'accent' }) === 0)
    ? subjectFilter.name
    : null
  const hasHistoryFilters = routeFilter !== 'all' || subjectFilter !== null
  const orderedDiagnostics = useMemo(
    () => sortDiagnosticsNewestFirst(diagnostics),
    [diagnostics],
  )
  const tickEvery = useMemo(() => {
    const barCount = report?.bars.length ?? 0
    if (!barCount || chartWidth <= 0) return 1
    // Keep roughly 44px between visible date ticks; every period keeps its
    // full label in the inspector, title, and accessible name.
    return Math.max(1, Math.ceil(44 / (chartWidth / barCount)))
  }, [chartWidth, report?.bars.length])

  const closeClearDialog = () => {
    if (clearPendingRef.current) return
    setClearConfirmed(false)
    setClearOpen(false)
  }

  const clearHistory = async () => {
    if (!clearConfirmed || clearPendingRef.current) return
    clearPendingRef.current = true
    setClearPending(true)
    try {
      onSnapshot(await api.clearHistory(true))
      setClearConfirmed(false)
      setClearOpen(false)
      setHistoryRevision((revision) => revision + 1)
    } catch (error) {
      onError(error)
    } finally {
      clearPendingRef.current = false
      setClearPending(false)
    }
  }

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
          <Panel className="history-toolbar-panel">
            <Toolbar className="history-controls" role="group" aria-label="Traffic history controls">
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
              <Field label="Route" className="history-filter-route">
                <select
                  aria-label="Route"
                  value={routeFilter}
                  onChange={(event) => setRouteFilter(event.target.value as HistoryRouteFilter)}
                >
                  <option value="all">All routes</option>
                  <option value="proxy">Proxy</option>
                  <option value="direct">Direct</option>
                </select>
              </Field>
              <Field label="App or group" className="history-filter-subject">
                <select
                  aria-label="App or group"
                  value={historySubjectValue(subjectFilter)}
                  onChange={(event) => setSubjectFilter(historySubjectFromValue(event.target.value))}
                >
                  <option value="all">All apps and groups</option>
                  {selectedAppOption ? (
                    <option value={historySubjectValue({ kind: 'app', name: selectedAppOption })}>{selectedAppOption}</option>
                  ) : null}
                  {eligibleGroups.length > 0 ? (
                    <optgroup label="Custom app groups">
                      {eligibleGroups.map((group) => (
                        <option value={historySubjectValue({ kind: 'group', id: group.id })} key={group.id}>
                          {group.name}
                        </option>
                      ))}
                    </optgroup>
                  ) : null}
                  {availableApps.length > 0 ? (
                    <optgroup label="Apps in this range">
                      {availableApps.map((app) => (
                        <option value={historySubjectValue({ kind: 'app', name: app })} key={app}>
                          {app}
                        </option>
                      ))}
                    </optgroup>
                  ) : (
                    <option value="" disabled>No attributed apps in this range</option>
                  )}
                </select>
              </Field>
              {hasHistoryFilters ? (
                <Button
                  variant="secondary"
                  icon="refresh"
                  className="history-filter-reset"
                  onClick={resetLogsFilters}
                >
                  Reset filters
                </Button>
              ) : null}
            </Toolbar>
          </Panel>

          {snapshot.state.historyPaused ? <div className="paused-note">History recording is paused. Existing data remains available.</div> : null}

          <Grid className="history-grid">
            <GridItem span={12}>
              <Panel
                className="metric-panel"
                data-history-total-download={report?.totalDownload ?? 0}
                data-history-total-upload={report?.totalUpload ?? 0}
                data-history-total={report?.total ?? 0}
              >
                <div className="history-summary-main">
                  <span className="metric-label">Total transferred</span>
                  <strong className="metric-value">{report ? formatBytes(report.total) : '—'}</strong>
                  {report ? <span className="muted metric-range">{prettyDateRange(report.from, report.to)}</span> : null}
                </div>
                <div className="history-subtotals" aria-label="Download and upload subtotals">
                  <div>
                    <span><i className="legend-download" /> Download</span>
                    <strong>{report ? formatBytes(report.totalDownload) : '—'}</strong>
                  </div>
                  <div>
                    <span><i className="legend-upload" /> Upload</span>
                    <strong>{report ? formatBytes(report.totalUpload) : '—'}</strong>
                  </div>
                </div>
              </Panel>
            </GridItem>
            <GridItem span={12}>
              <Panel className="chart-panel" aria-labelledby="traffic-history-heading">
                <PanelHeading
                  id="traffic-history-heading"
                  title="Traffic history"
                  actions={
                    <div className="chart-legend">
                      <span><i className="legend-proxy" /> Proxy</span>
                      <span><i className="legend-direct" /> Direct</span>
                    </div>
                  }
                />
                {report ? (
                  <>
                    <div className="bar-period-inspector" role="status" aria-live="polite">
                      {inspectedPeriod ? (
                        <>
                          <strong>{historyPeriodLabel(inspectedPeriod, range)}</strong>
                          <span><i className="legend-proxy" /> Proxy {formatBytes(inspectedPeriod.proxyTotal)}</span>
                          <span><i className="legend-direct" /> Direct {formatBytes(inspectedPeriod.directTotal)}</span>
                          <span className={inspectedPeriod.gap ? 'has-recording-gap' : ''}>
                            {inspectedPeriod.gap ? 'Recording gap' : 'No recording gap'}
                          </span>
                        </>
                      ) : (
                        <span className="muted">Hover over or focus a period to inspect it.</span>
                      )}
                    </div>
                    <div className="bar-chart-wrap">
                      <div className="bar-y-axis" aria-hidden="true">
                        {[1, 0.75, 0.5, 0.25, 0].map((fraction) => <span key={fraction}>{formatBytes(max * fraction)}</span>)}
                      </div>
                      <div className="bar-chart-scroll" ref={chartRef} onMouseLeave={() => setInspectedPeriod(null)}>
                        <div className={`bar-chart${tickEvery > 1 ? ' is-dense' : ''}`}>
                          {report.bars.map((bar, index) => {
                            const periodLabel = historyPeriodLabel(bar, range)
                            const description = `${periodLabel}. Proxy ${formatBytes(bar.proxyTotal)}. Direct ${formatBytes(bar.directTotal)}. ${bar.gap ? 'Recording gap in this period.' : 'No recording gap recorded for this period.'}`
                            const showTick = (report.bars.length - 1 - index) % tickEvery === 0
                            return (
                              <button
                                type="button"
                                className="bar-group"
                                key={bar.start}
                                title={description}
                                aria-label={description}
                                data-history-bar-download={bar.download}
                                data-history-bar-upload={bar.upload}
                                data-history-bar-proxy={bar.proxyTotal}
                                data-history-bar-direct={bar.directTotal}
                                data-history-bar-gap={bar.gap ? 'true' : 'false'}
                                onMouseEnter={() => setInspectedPeriod(bar)}
                                onFocus={() => setInspectedPeriod(bar)}
                                onBlur={() => setInspectedPeriod((current) => current?.start === bar.start ? null : current)}
                                onKeyDown={(event) => {
                                  if (event.key === 'Escape') setInspectedPeriod(null)
                                }}
                              >
                                <span className="bar-route-pair" aria-hidden="true">
                                  <span className="bar-route-slot" data-history-route="proxy"><span className="bar bar-proxy" style={{ height: `${(bar.proxyTotal / max) * 100}%` }} /></span>
                                  <span className="bar-route-slot" data-history-route="direct"><span className="bar bar-direct" style={{ height: `${(bar.directTotal / max) * 100}%` }} /></span>
                                </span>
                                <span className={`bar-label ${bar.gap ? 'has-gap' : ''} ${showTick ? 'is-tick' : 'is-tick-hidden'}`} aria-hidden="true" data-history-tick={showTick ? 'true' : 'false'}>{bar.label}{bar.gap ? ' ·' : ''}</span>
                              </button>
                            )
                          })}
                        </div>
                      </div>
                    </div>
                  </>
                ) : (
                  <div className="chart-loading" aria-live="polite">Loading traffic history…</div>
                )}
                {report && report.gapMinutes > 0 ? <p className="gap-note"><Icon name="alert" size={14} /> {report.gapMinutes} minute{report.gapMinutes === 1 ? '' : 's'} with a data gap</p> : null}
              </Panel>
            </GridItem>
          </Grid>

          <Toolbar className="breakdown-row">
            <label>
              <span className="sr-only">Breakdown</span>
              <select value={breakdown} onChange={(event) => setBreakdown(event.target.value as HistoryBreakdown)}>
                <option value="app">By app</option>
                <option value="customGroup">By custom group</option>
                <option value="rule">By ruleset rule</option>
                <option value="route">By route</option>
              </select>
            </label>
            <Button variant="secondary" icon="edit" onClick={() => setGroupOpen(true)}>Manage app groups</Button>
          </Toolbar>

          <Panel
            className="history-table-panel"
            data-history-total-download={report?.totalDownload ?? 0}
            data-history-total-upload={report?.totalUpload ?? 0}
          >
            {report && report.rows.length > 0 ? (
              <div className="table-wrap">
                <table className="history-table">
                  <thead>
                    <tr>
                      <th scope="col">{breakdown === 'route' ? 'Route' : breakdown === 'customGroup' ? 'Group' : breakdown === 'rule' ? 'Rule' : 'App'}</th>
                      <th scope="col" className="numeric">Download</th>
                      <th scope="col" className="numeric">Upload</th>
                      <th scope="col" className="numeric">Transferred</th>
                    </tr>
                  </thead>
                  <tbody>
                    {report.rows.map((row) => (
                      <tr
                        key={`${row.label}-${row.app ?? ''}-${row.route ?? ''}-${row.routeClass}`}
                        data-history-row-download={row.download}
                        data-history-row-upload={row.upload}
                      >
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
                  </tbody>
                </table>
              </div>
            ) : report ? (
              <EmptyState
                title={hasHistoryFilters ? 'No traffic matches these filters' : 'No traffic has been recorded in this range'}
                description={hasHistoryFilters
                  ? 'Try another route, app, or custom group. You can reset both filters to see all recorded traffic.'
                  : 'Traffic history will appear here after attributed or total traffic is recorded.'}
                action={hasHistoryFilters ? (
                  <Button variant="secondary" onClick={resetLogsFilters}>
                    Reset filters
                  </Button>
                ) : undefined}
              />
            ) : (
              <EmptyState
                title={busy === 'history' ? 'Updating traffic history' : 'No report yet'}
                description="Traffic history loads from the local SQLite database."
              />
            )}
          </Panel>
        </div>
      ) : (
        <Panel className="diagnostics-panel">
          <PanelHeading
            title="Live diagnostics"
            description="Bounded application and core messages, newest first. Credentials, URLs, and query tokens are redacted."
            actions={<Button variant="secondary" icon="refresh" onClick={() => void api.getDiagnostics().then(setDiagnostics).catch(onError)}>Refresh</Button>}
            className="diagnostics-head"
          />
          <div className="diagnostics-log" role="log" aria-live="polite">
            {orderedDiagnostics.length === 0 ? <p className="muted diagnostic-empty">No diagnostics yet.</p> : null}
            {orderedDiagnostics.map((entry) => (
              <div
                className={`diagnostic-line level-${entry.level}`}
                key={entry.id}
                data-diagnostic-id={entry.id}
                data-diagnostic-at={entry.at}
              >
                <span className="diag-time">{formatTime(entry.at)}</span>
                <span className={`diag-source source-${entry.source}`}>{entry.source}</span>
                <span className="diag-message">{entry.message}</span>
              </div>
            ))}
          </div>
        </Panel>
      )}

      {clearOpen ? (
        <Modal
          title="Clear traffic history"
          description="This permanently deletes the local minute aggregates and custom-group results. It does not change your VPN or subscriptions."
          onClose={closeClearDialog}
          footer={
            <>
              <div className="clear-history-confirm">
                <Toggle
                  checked={clearConfirmed}
                  onChange={setClearConfirmed}
                  label="Confirm deletion"
                  disabled={clearPending}
                />
                <span>Confirm deletion</span>
              </div>
              <Button onClick={closeClearDialog} disabled={clearPending}>Cancel</Button>
              <Button
                variant="danger"
                busy={clearPending}
                disabled={!clearConfirmed}
                onClick={() => void clearHistory()}
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
            setHistoryRevision((revision) => revision + 1)
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
