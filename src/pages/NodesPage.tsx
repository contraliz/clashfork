import { useMemo, useState } from 'react'
import { api } from '../api'
import type { ImportPreview, Snapshot, SourceType } from '../types'
import { allowanceProgress, allowanceSummary, formatBytes, formatDate, formatExpiry } from '../lib/format'
import { Icon } from '../components/Icon'
import {
  Button,
  EmptyState,
  Field,
  IconButton,
  Modal,
  PageHeader,
  Progress,
  SourceBadge,
} from '../components/ui'

interface Props {
  snapshot: Snapshot
  onSnapshot: (snapshot: Snapshot) => void
  onError: (error: unknown) => void
}

export function NodesPage({ snapshot, onSnapshot, onError }: Props) {
  const { state } = snapshot
  const [search, setSearch] = useState('')
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({})
  const [addOpen, setAddOpen] = useState(false)
  const [input, setInput] = useState('')
  const [preview, setPreview] = useState<ImportPreview | null>(null)
  const [title, setTitle] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const [renameService, setRenameService] = useState<{ id: string; title: string } | null>(null)

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
          <Button variant="primary" icon="plus" onClick={() => setAddOpen(true)}>
            Add Nodes
          </Button>
        }
      />

      <div className="search-row">
        <label className="search-field">
          <Icon name="search" size={21} />
          <span className="sr-only">Search nodes</span>
          <input
            value={search}
            placeholder="Search nodes"
            onChange={(event) => setSearch(event.target.value)}
          />
        </label>
      </div>

      {state.services.length === 0 ? (
        <div className="panel">
          <EmptyState
            title="No nodes yet"
            description="Add an HTTPS Clash/Mihomo subscription or an individual share link. Only node definitions and provider metadata are imported."
            action={
              <Button variant="primary" icon="plus" onClick={() => setAddOpen(true)}>
                Add Nodes
              </Button>
            }
          />
        </div>
      ) : (
        <div className="service-stack">
          {state.services.map((service) => {
            const nodes = filtered.filter((node) => node.serviceId === service.id)
            const isCollapsed = collapsed[service.id]
            const progress = allowanceProgress(service.allowance)
            return (
              <section className="panel service-card" key={service.id}>
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
                      {service.stale ? <SourceBadge tone="amber">Stale</SourceBadge> : null}
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
                            <th scope="col">State</th>
                          </tr>
                        </thead>
                        <tbody>
                          {nodes.map((node) => (
                            <tr key={node.id} className={node.missing ? 'is-missing' : ''}>
                              <td><strong>{node.displayName}</strong></td>
                              <td>{node.protocol}</td>
                              <td>
                                {node.missing ? (
                                  <SourceBadge tone="red">Missing · choose replacement</SourceBadge>
                                ) : (
                                  <span className="available-label"><span className="status-dot" />Available</span>
                                )}
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    )}
                  </div>
                ) : null}
              </section>
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
