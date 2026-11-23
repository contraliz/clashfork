import { useEffect, useMemo, useState } from 'react'
import { api, subscribeNodeTestProgress } from '../api'
import type { ImportPreview, NodeTestProgress, Snapshot, SourceType } from '../types'
import { allowanceProgress, allowanceSummary, formatBytes, formatDate, formatExpiry } from '../lib/format'
import { Icon } from '../components/Icon'
import { LatencyValue } from '../components/NodeLatency'
import { useNowMs } from '../hooks'
import {
  Button,
  EmptyState,
  Field,
  IconButton,
  Modal,
  PageHeader,
  Panel,
  Progress,
  SourceBadge,
  Toolbar,
} from '../components/ui'

interface Props {
  snapshot: Snapshot
  onSnapshot: (snapshot: Snapshot) => void
  onError: (error: unknown) => void
}

export function NodesPage({ snapshot, onSnapshot, onError }: Props) {
  const { state } = snapshot
  const now = useNowMs(1000)
  const [search, setSearch] = useState('')
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({})
  const [addOpen, setAddOpen] = useState(false)
  const [input, setInput] = useState('')
  const [preview, setPreview] = useState<ImportPreview | null>(null)
  const [title, setTitle] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const [testingNodeId, setTestingNodeId] = useState<string | null>(null)
  const [testingAll, setTestingAll] = useState(false)
  const [sweepProgress, setSweepProgress] = useState<NodeTestProgress | null>(null)
  const [testFeedback, setTestFeedback] = useState<Record<string, string>>({})
  const [sweepFeedback, setSweepFeedback] = useState('')
  const [renameService, setRenameService] = useState<{ id: string; title: string } | null>(null)

  useEffect(() => {
    let mounted = true
    let unlisten: (() => void) | null = null
    void subscribeNodeTestProgress((progress) => {
      if (!mounted) return
      setSweepProgress(progress)
      if (progress.nodeId) {
        setTestFeedback((current) => {
          const next = { ...current }
          if (progress.latencyMs == null) next[progress.nodeId!] = 'No response'
          else delete next[progress.nodeId!]
          return next
        })
      }
      if (progress.finished) setTestingAll(false)
    }).then((stop) => {
      if (!mounted) stop?.()
      else unlisten = stop
    })
    return () => {
      mounted = false
      unlisten?.()
    }
  }, [])

  const filtered = useMemo(() => {
    const query = search.trim().toLowerCase()
    if (!query) return state.nodes
    return state.nodes.filter((node) =>
      [node.displayName, node.serviceTitle, node.protocol].some((value) =>
        value.toLowerCase().includes(query),
      ),
    )
  }, [search, state.nodes])

  const run = async (key: string, action: () => Promise<Snapshot>) => {
    setBusy(key)
    try {
      onSnapshot(await action())
    } catch (error) {
      onError(error)
    } finally {
      setBusy(null)
    }
  }

  const closeAdd = () => {
    if (preview) void api.cancelImport(preview.id)
    setPreview(null)
    setInput('')
    setTitle('')
    setAddOpen(false)
  }

  const fetchPreview = async () => {
    if (!input.trim()) return
    setBusy('preview')
    try {
      const next = await api.previewImport(input.trim())
      setPreview(next)
      setTitle(next.sourceType === 'subscription' ? next.title : 'Manual nodes')
    } catch (error) {
      onError(error)
    } finally {
      setBusy(null)
    }
  }

  const confirm = async () => {
    if (!preview) return
    setBusy('confirm')
    try {
      onSnapshot(await api.confirmImport(preview.id, title.trim() || undefined))
      closeAdd()
    } catch (error) {
      onError(error)
    } finally {
      setBusy(null)
    }
  }

  return (
    <div>
      <PageHeader
        title="Nodes"
        actions={
          <div className="nodes-header-actions">
            <Button
              variant="secondary"
              busy={testingAll}
              disabled={!snapshot.runtime.vpnActive || !state.nodes.some((node) => !node.missing)}
              onClick={() => {
                setTestingAll(true)
                setSweepFeedback('')
                void api.testAllNodes().then(async (summary) => {
                  onSnapshot(await api.getSnapshot())
                  if (summary.cancelled) setSweepFeedback(`Stopped · ${summary.responded} of ${summary.total} responded`)
                  else setSweepFeedback(`${summary.responded} of ${summary.total} responded`)
                  setTestingAll(false)
                }).catch((error: unknown) => {
                  setSweepFeedback(nodeTestFailureText(error))
                  setTestingAll(false)
                })
              }}
              title={snapshot.runtime.vpnActive ? 'Test available nodes one at a time per route, up to four in parallel' : 'Connect Clashfork to test node latency'}
            >
              Test all nodes
            </Button>
            <Button variant="primary" icon="plus" onClick={() => setAddOpen(true)}>
              Add Nodes
            </Button>
          </div>
        }
      />

      <Toolbar className="search-row">
        <label className="search-field">
          <Icon name="search" size={18} />
          <span className="sr-only">Search nodes</span>
          <input
            value={search}
            placeholder="Search nodes"
            onChange={(event) => setSearch(event.target.value)}
          />
        </label>
        <span className="toolbar-meta">{filtered.length} {filtered.length === 1 ? 'node' : 'nodes'}</span>
      </Toolbar>
      {testingAll && sweepProgress ? (
        <Toolbar className="node-test-progress" role="status" aria-live="polite">
          <span>Testing {sweepProgress.completed} of {sweepProgress.total}</span>
          <span>{sweepProgress.responded} responded</span>
          {sweepProgress.failed > 0 ? <span>{sweepProgress.failed} no response</span> : null}
          <Button variant="quiet" onClick={() => void api.cancelNodeTestSweep()}>
            Cancel
          </Button>
        </Toolbar>
      ) : null}
      {!snapshot.runtime.vpnActive ? (
        <p className="node-test-hint">Connect Clashfork to test node latency.</p>
      ) : null}
      {!testingAll && sweepFeedback ? <p className="node-test-hint" role="status">{sweepFeedback}</p> : null}

      {state.services.length === 0 ? (
        <Panel>
          <EmptyState
            title="No nodes yet"
            description="Add an HTTPS Clash/Mihomo subscription or an individual share link. Only node definitions and provider metadata are imported."
            action={
              <Button variant="primary" icon="plus" onClick={() => setAddOpen(true)}>
                Add Nodes
              </Button>
            }
          />
        </Panel>
      ) : (
        <div className="service-stack">
          {state.services.map((service) => {
            const nodes = filtered.filter((node) => node.serviceId === service.id)
            const hasAvailableNodes = state.nodes.some((node) => node.serviceId === service.id && !node.missing)
            const isCollapsed = collapsed[service.id]
            const progress = allowanceProgress(service.allowance)
            return (
              <Panel className="service-card" key={service.id}>
                <div className="service-head">
                  <button
                    type="button"
                    className="collapse-button"
                    aria-expanded={!isCollapsed}
                    aria-label={`${isCollapsed ? 'Expand' : 'Collapse'} ${service.title}`}
                    onClick={() => setCollapsed((current) => ({ ...current, [service.id]: !isCollapsed }))}
                  >
                    <Icon name={isCollapsed ? 'chevron-right' : 'chevron-down'} size={22} />
                  </button>
                  <div className="service-title-wrap">
                    <div className="service-title-line">
                      <h2>{service.title}</h2>
                      {service.sourceType === 'subscription' ? (
                        <SourceBadge>Subscription</SourceBadge>
                      ) : (
                        <SourceBadge>Manual</SourceBadge>
                      )}
                      {service.stale && !hasAvailableNodes ? <SourceBadge tone="red">Unavailable</SourceBadge> : null}
                      {service.omittedNodeIds.length > 0 ? <SourceBadge tone="amber">Omitted once</SourceBadge> : null}
                    </div>
                    <div className="service-meta">
                      <span>{service.nodeCount} {service.nodeCount === 1 ? 'node' : 'nodes'}</span>
                      {service.missingCount > 0 ? <span className="danger-text">{service.missingCount} missing</span> : null}
                      <span aria-hidden="true">·</span>
                      <span>{allowanceSummary(service.allowance)}</span>
                      {service.allowance.kind === 'full' ? <span>Expires {formatExpiry(service.allowance.expiresAt)}</span> : null}
                      <span>Last refresh {formatDate(service.lastRefreshAt)}</span>
                    </div>
                  </div>
                  <div className="service-actions">
                    <IconButton
                      icon="edit"
                      label={`Rename ${service.title}`}
                      onClick={() => setRenameService({ id: service.id, title: service.title })}
                    />
                    {service.sourceType === 'subscription' ? (
                      <Button
                        variant="secondary"
                        icon="refresh"
                        busy={busy === `refresh-${service.id}`}
                        onClick={() => void run(`refresh-${service.id}`, () => api.refreshService(service.id))}
                      >
                        Refresh
                      </Button>
                    ) : null}
                  </div>
                </div>
                {progress !== null ? (
                  <div className="quota-row">
                    <Progress value={progress} label={`${service.title} allowance used`} />
                    <span className="quota-copy">
                      {formatProgress(service.allowance)} used
                    </span>
                  </div>
                ) : null}
                {service.lastError ? <p className="service-error">{service.lastError}</p> : null}
                {snapshot.runtime.pendingReconnectServices.includes(service.id) ? (
                  <p className="service-refresh-notice">A selected node was refreshed. The running session keeps its existing endpoint until reconnect.</p>
                ) : null}
                {!isCollapsed ? (
                  <div className="node-table-wrap">
                    {nodes.length === 0 ? (
                      <p className="muted node-empty">No nodes match this search.</p>
                    ) : (
                      <table className="node-table">
                        <thead>
                          <tr>
                            <th scope="col">Node</th>
                            <th scope="col">Protocol</th>
                            <th scope="col">Latency</th>
                            <th scope="col">State</th>
                          </tr>
                        </thead>
                        <tbody>
                          {nodes.map((node) => (
                            <tr key={node.id} data-node-id={node.id} className={node.missing ? 'is-missing' : ''}>
                              <td><strong>{node.displayName}</strong></td>
                              <td>{node.protocol}</td>
                              <td>
                                <div className="node-latency-cell">
                                  <LatencyValue node={node} now={now} />
                                  {testFeedback[node.id] ? (
                                    <span className="node-test-feedback" role="status">{testFeedback[node.id]}</span>
                                  ) : null}
                                  {!node.missing ? (
                                    <Button
                                      variant="quiet"
                                      className="node-test-button"
                                      busy={testingNodeId === node.id}
                                      disabled={!snapshot.runtime.vpnActive || testingNodeId !== null || testingAll || busy !== null}
                                      aria-label={`Test ${node.displayName} latency`}
                                      title={snapshot.runtime.vpnActive ? `Test ${node.displayName} latency` : 'Connect Clashfork to test node latency'}
                                      onClick={() => {
                                        setTestingNodeId(node.id)
                                        setTestFeedback((current) => {
                                          const next = { ...current }
                                          delete next[node.id]
                                          return next
                                        })
                                        void api.testNode(node.id).then(onSnapshot).catch((error: unknown) => {
                                          setTestFeedback((current) => ({ ...current, [node.id]: nodeTestFailureText(error) }))
                                        }).finally(() => setTestingNodeId(null))
                                      }}
                                    >
                                      Test
                                    </Button>
                                  ) : null}
                                </div>
                              </td>
                              <td>
                                {node.missing ? (
                                  <SourceBadge tone="red">Missing · choose replacement</SourceBadge>
                                ) : node.omittedOnce ? (
                                  <SourceBadge tone="amber">Omitted once · awaiting confirmation</SourceBadge>
                                ) : (
                                  <SourceBadge tone="accent" dot>Available</SourceBadge>
                                )}
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    )}
                  </div>
                ) : null}
              </Panel>
            )
          })}
        </div>
      )}

      {addOpen ? (
        <Modal
          title={preview ? 'Preview nodes' : 'Add nodes'}
          description={
            preview
              ? 'Only supported nodes and provider metadata will be imported. Credentials are encrypted for your Windows user.'
              : 'Paste an HTTPS Clash/Mihomo subscription or a Trojan, Shadowsocks, VMess, or VLESS share link.'
          }
          onClose={closeAdd}
          wide={Boolean(preview)}
          footer={
            preview ? (
              <>
                <Button onClick={() => { setPreview(null); setTitle('') }}>Back</Button>
                <Button variant="primary" busy={busy === 'confirm'} disabled={preview.nodeCount === 0} onClick={() => void confirm()}>
                  Add {preview.nodeCount} {preview.nodeCount === 1 ? 'node' : 'nodes'}
                </Button>
              </>
            ) : (
              <>
                <Button onClick={closeAdd}>Cancel</Button>
                <Button variant="primary" busy={busy === 'preview'} disabled={!input.trim()} onClick={() => void fetchPreview()}>
                  Preview
                </Button>
              </>
            )
          }
        >
          {!preview ? (
            <Field label="Subscription URL or share link" hint="HTTPS only. URLs are stored with DPAPI and are never shown in lists or diagnostics.">
              <textarea
                data-autofocus
                className="source-input"
                rows={5}
                spellCheck={false}
                value={input}
                placeholder="https://… or trojan://…"
                onChange={(event) => setInput(event.target.value)}
              />
            </Field>
          ) : (
            <div className="import-preview">
              <div className="preview-summary">
                <div>
                  <div className="panel-kicker">{sourceTypeLabel(preview.sourceType)}</div>
                  <h3>{preview.title}</h3>
                  {preview.sourceHost ? <p className="muted">{preview.sourceHost}</p> : null}
                </div>
                <div className="preview-stats">
                  <span><strong>{preview.nodeCount}</strong> supported</span>
                  {preview.unsupportedCount > 0 ? <span><strong>{preview.unsupportedCount}</strong> skipped</span> : null}
                  {preview.noticeCount > 0 ? <span><strong>{preview.noticeCount}</strong> notices filtered</span> : null}
                  <span>{allowanceSummary(preview.allowance)}</span>
                </div>
              </div>
              <Field label="Service display name">
                <input value={title} maxLength={80} onChange={(event) => setTitle(event.target.value)} />
              </Field>
              <div className="preview-list-wrap">
                <table className="node-table">
                  <thead><tr><th scope="col">Node</th><th scope="col">Protocol</th></tr></thead>
                  <tbody>
                    {preview.nodes.map((node, index) => (
                      <tr key={`${node.displayName}-${index}`}>
                        <td>{node.displayName}</td>
                        <td>{node.protocol}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <p className="field-hint">
                This imports nodes and service information. Routing rules are managed locally in Rulesets.
              </p>
            </div>
          )}
        </Modal>
      ) : null}

      {renameService ? (
        <Modal
          title="Rename service"
          onClose={() => setRenameService(null)}
          footer={
            <>
              <Button onClick={() => setRenameService(null)}>Cancel</Button>
              <Button
                variant="primary"
                busy={busy === 'rename'}
                onClick={async () => {
                  setBusy('rename')
                  try {
                    onSnapshot(await api.renameService(renameService.id, renameService.title))
                    setRenameService(null)
                  } catch (error) {
                    onError(error)
                  } finally {
                    setBusy(null)
                  }
                }}
              >
                Save name
              </Button>
            </>
          }
        >
          <Field label="Service name">
            <input
              data-autofocus
              value={renameService.title}
              maxLength={80}
              onChange={(event) => setRenameService({ ...renameService, title: event.target.value })}
            />
          </Field>
        </Modal>
      ) : null}
    </div>
  )
}

function sourceTypeLabel(type: SourceType): string {
  if (type === 'subscription') return 'Subscription'
  if (type === 'link') return 'Share link'
  return 'Manual'
}

function formatProgress(allowance: ImportPreview['allowance']): string {
  if (allowance.kind !== 'full') return ''
  return `${formatBytes(allowance.used)} of ${formatBytes(allowance.total)}`
}

function nodeTestFailureText(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error)
  if (message.includes('Connect Clashfork')) return 'Connect Clashfork to test'
  if (message.includes('Reconnect Clashfork')) return 'Reconnect to test this update'
  return 'No response'
}
