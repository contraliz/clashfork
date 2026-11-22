import { useEffect, useState } from 'react'
import { api } from '../api'
import type {
  ConditionKind,
  Destination,
  MatchGroup,
  Ruleset,
  RulesetEntry,
  Snapshot,
} from '../types'
import {
  Button,
  EmptyState,
  Field,
  IconButton,
  Modal,
  PageHeader,
  SourceBadge,
} from '../components/ui'

interface Props {
  snapshot: Snapshot
  onSnapshot: (snapshot: Snapshot) => void
  onError: (error: unknown) => void
}

interface RulesetDraft {
  id: string | null
  name: string
  entries: RulesetEntry[]
  miscellaneous: 'direct' | 'block'
}

const emptyDraft = (): RulesetDraft => ({
  id: null,
  name: '',
  entries: [],
  miscellaneous: 'direct',
})

const conditionLabels: Record<ConditionKind, string> = {
  process_name: 'Process name',
  process_path: 'Process path',
  domain: 'Domain exact',
  domain_suffix: 'Domain suffix',
  ipv4_cidr: 'IPv4 CIDR',
  ipv6_cidr: 'IPv6 CIDR',
}

export function RulesetsPage({ snapshot, onSnapshot, onError }: Props) {
  const { state } = snapshot
  const [selectedId, setSelectedId] = useState<string | null>(state.selection.rulesetId ?? state.rulesets[0]?.id ?? null)
  const [creatingNew, setCreatingNew] = useState(false)
  const [draft, setDraft] = useState<RulesetDraft>(() => {
    const first = state.rulesets.find((ruleset) => ruleset.id === state.selection.rulesetId) ?? state.rulesets[0]
    return first ? fromRuleset(first) : emptyDraft()
  })
  const [busy, setBusy] = useState<string | null>(null)
  const [groupEditor, setGroupEditor] = useState<MatchGroup | 'new' | null>(null)
  const [deleteConfirm, setDeleteConfirm] = useState<Ruleset | null>(null)

  useEffect(() => {
    if (creatingNew) return
    if (!selectedId || !state.rulesets.some((ruleset) => ruleset.id === selectedId)) {
      const first = state.rulesets[0]
      setSelectedId(first?.id ?? null)
      setDraft(first ? fromRuleset(first) : emptyDraft())
    }
  }, [creatingNew, selectedId, state.rulesets])

  const selected = state.rulesets.find((ruleset) => ruleset.id === selectedId)
  const selectRuleset = (ruleset: Ruleset) => {
    setCreatingNew(false)
    setSelectedId(ruleset.id)
    setDraft(fromRuleset(ruleset))
  }

  const newRuleset = () => {
    setCreatingNew(true)
    setSelectedId(null)
    setDraft(emptyDraft())
  }

  const run = async (key: string, action: () => Promise<Snapshot>) => {
    setBusy(key)
    try {
      onSnapshot(await action())
    } catch (error) {
      try {
        onSnapshot(await api.getSnapshot())
      } catch {
        // The original error is more useful than a refresh failure.
      }
      onError(error)
    } finally {
      setBusy(null)
    }
  }

  const saveDraft = async () => {
    if (!draft.name.trim()) {
      onError(new Error('Name the ruleset before saving.'))
      return
    }
    setBusy('save')
    try {
      const next = await api.saveRuleset({
        id: draft.id,
        name: draft.name,
        entries: draft.entries,
        miscellaneous: draft.miscellaneous,
      })
      onSnapshot(next)
      const saved =
        next.state.rulesets.find((ruleset) => ruleset.id === draft.id) ??
        [...next.state.rulesets].reverse().find((ruleset) => ruleset.name === draft.name.trim())
      if (saved) {
        setCreatingNew(false)
        setSelectedId(saved.id)
        setDraft(fromRuleset(saved))
      }
    } catch (error) {
      try {
        onSnapshot(await api.getSnapshot())
      } catch {
        // Keep the original failure visible.
      }
      onError(error)
    } finally {
      setBusy(null)
    }
  }

  const addEntry = (groupId: string) => {
    if (!groupId) return
    setDraft((current) => ({
      ...current,
      entries: [
        ...current.entries,
        {
          id: `local_${Date.now()}`,
          matchGroupId: groupId,
          destination: { kind: 'direct' },
        },
      ],
    }))
  }

  const updateEntry = (entryId: string, update: Partial<RulesetEntry>) => {
    setDraft((current) => ({
      ...current,
      entries: current.entries.map((entry) => (entry.id === entryId ? { ...entry, ...update } : entry)),
    }))
  }

  const moveEntry = (index: number, direction: -1 | 1) => {
    setDraft((current) => {
      const next = [...current.entries]
      const target = index + direction
      if (target < 0 || target >= next.length) return current
      ;[next[index], next[target]] = [next[target], next[index]]
      return { ...current, entries: next }
    })
  }

  const unusedGroups = state.matchGroups.filter(
    (group) => !draft.entries.some((entry) => entry.matchGroupId === group.id),
  )

  return (
    <div>
      <PageHeader
        title="Rulesets"
        actions={
          <>
            <Button variant="secondary" icon="plus" onClick={() => setGroupEditor('new')}>
              New match group
            </Button>
            <Button variant="primary" onClick={newRuleset}>
              New ruleset
            </Button>
          </>
        }
      />

      <div className="rulesets-layout">
        <aside className="panel ruleset-index" aria-label="Rulesets">
          {state.rulesets.length === 0 ? (
            <p className="muted index-empty">No rulesets yet.</p>
          ) : (
            <ul>
              {state.rulesets.map((ruleset) => (
                <li key={ruleset.id}>
                  <button
                    type="button"
                    className={selectedId === ruleset.id ? 'is-active' : ''}
                    aria-current={selectedId === ruleset.id ? 'true' : undefined}
                    onClick={() => selectRuleset(ruleset)}
                  >
                    <span>{ruleset.name}</span>
                    {state.selection.rulesetId === ruleset.id ? <SourceBadge tone="green">Active</SourceBadge> : null}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </aside>

        <section className="panel ruleset-detail">
          {!creatingNew && selected == null && !draft.name && draft.entries.length === 0 ? (
            <EmptyState
              title="Create a ruleset"
              description="Group match conditions into destinations, then pick the ruleset on the Dashboard. The final Miscellaneous row is always last."
              action={<Button variant="primary" onClick={newRuleset}>New ruleset</Button>}
            />
          ) : (
            <>
              <div className="ruleset-detail-head">
                <div className="ruleset-title-field">
                  <label htmlFor="ruleset-name" className="sr-only">Ruleset name</label>
                  <input
                    id="ruleset-name"
                    value={draft.name}
                    maxLength={80}
                    placeholder="Ruleset name"
                    onChange={(event) => setDraft({ ...draft, name: event.target.value })}
                  />
                  {state.selection.rulesetId === draft.id ? <SourceBadge tone="green">Active</SourceBadge> : null}
                </div>
                <div className="ruleset-actions">
                  {draft.id ? (
                    <IconButton
                      icon="trash"
                      label="Delete ruleset"
                      disabled={busy !== null || state.selection.rulesetId === draft.id}
                      onClick={() => selected && setDeleteConfirm(selected)}
                    />
                  ) : null}
                  <Button variant="primary" busy={busy === 'save'} disabled={busy !== null} onClick={() => void saveDraft()}>
                    Save ruleset
                  </Button>
                </div>
              </div>

              <div className="rules-table" role="table" aria-label="Ruleset entries">
                <div className="rules-table-head" role="row">
                  <span role="columnheader">Group</span>
                  <span role="columnheader">Action</span>
                  <span role="columnheader">Destination</span>
                  <span className="sr-only" role="columnheader">Order</span>
                </div>
                {draft.entries.length === 0 ? (
                  <div className="rules-empty">Add a match group to this ruleset. All conditions in a group are OR; the first matching group wins.</div>
                ) : null}
                {draft.entries.map((entry, index) => {
                  const group = state.matchGroups.find((candidate) => candidate.id === entry.matchGroupId)
                  return (
                    <div className="rules-row" role="row" key={entry.id}>
                      <div className="rules-cell" role="cell">
                        <strong>{group?.name ?? 'Missing group'}</strong>
                        <span>{group?.conditions.map((condition) => `${conditionLabels[condition.kind]}: ${condition.value}`).join(' · ') || ''}</span>
                      </div>
                      <div className="rules-cell" role="cell">
                        <select
                          aria-label={`Action for ${group?.name ?? 'group'}`}
                          value={entry.destination.kind}
                          onChange={(event) => {
                            const kind = event.target.value
                            const destination: Destination =
                              kind === 'direct'
                                ? { kind: 'direct' }
                                : kind === 'block'
                                  ? { kind: 'block' }
                                  : { kind: 'node', nodeId: firstAvailableNode(state)?.id ?? '' }
                            updateEntry(entry.id, { destination })
                          }}
                        >
                          <option value="direct">Direct</option>
                          <option value="block">Block</option>
                          <option value="node">Node</option>
                        </select>
                      </div>
                      <div className="rules-cell" role="cell">
                        {entry.destination.kind === 'node' ? (
                          <select
                            aria-label={`Node destination for ${group?.name ?? 'group'}`}
                            value={entry.destination.nodeId}
                            onChange={(event) =>
                              updateEntry(entry.id, {
                                destination: { kind: 'node', nodeId: event.target.value },
                              })
                            }
                          >
                            <option value="">Select node</option>
                            {groupNodes(state.nodes).map(([service, nodes]) => (
                              <optgroup key={service} label={service}>
                                {nodes.filter((node) => !node.missing).map((node) => (
                                  <option key={node.id} value={node.id}>{node.displayName} · {node.protocol}</option>
                                ))}
                              </optgroup>
                            ))}
                          </select>
                        ) : (
                          <span className="muted">{entry.destination.kind === 'block' ? 'Rejected before proxying' : 'Out of tunnel'}</span>
                        )}
                      </div>
                      <div className="order-buttons" role="cell">
                        <IconButton icon="arrow-up" label="Move group up" disabled={index === 0} onClick={() => moveEntry(index, -1)} />
                        <IconButton icon="arrow-down" label="Move group down" disabled={index === draft.entries.length - 1} onClick={() => moveEntry(index, 1)} />
                        <IconButton
                          icon="close"
                          label="Remove group from ruleset"
                          onClick={() =>
                            setDraft((current) => ({
                              ...current,
                              entries: current.entries.filter((candidate) => candidate.id !== entry.id),
                            }))
                          }
                        />
                      </div>
                    </div>
                  )
                })}
                <div className="rules-row rules-row-fixed" role="row">
                  <div className="rules-cell" role="cell"><strong>Miscellaneous</strong><span>Final catch-all</span></div>
                  <div className="rules-cell" role="cell">
                    <select
                      aria-label="Miscellaneous action"
                      value={draft.miscellaneous}
                      onChange={(event) => setDraft({ ...draft, miscellaneous: event.target.value as 'direct' | 'block' })}
                    >
                      <option value="direct">Direct</option>
                      <option value="block">Block</option>
                    </select>
                  </div>
                  <div className="rules-cell" role="cell"><span className="muted">Always last</span></div>
                  <div />
                </div>
              </div>

              <div className="add-group-row">
                <label>
                  <span className="sr-only">Add match group to ruleset</span>
                  <select value="" disabled={unusedGroups.length === 0} onChange={(event) => addEntry(event.target.value)}>
                    <option value="">{unusedGroups.length === 0 ? 'All match groups added' : 'Add match group…'}</option>
                    {unusedGroups.map((group) => <option key={group.id} value={group.id}>{group.name}</option>)}
                  </select>
                </label>
                <span className="field-hint">First match wins. OR applies within a group.</span>
              </div>

              <div className="match-groups-head">
                <div>
                  <h2>Match groups</h2>
                  <p>Each group is OR across its conditions and reusable across rulesets.</p>
                </div>
                <Button variant="secondary" icon="plus" onClick={() => setGroupEditor('new')}>New match group</Button>
              </div>
              <div className="match-groups">
                {state.matchGroups.length === 0 ? <p className="muted">Create a match group to begin.</p> : null}
                {state.matchGroups.map((group) => (
                  <article className="match-group-card" key={group.id}>
                    <header>
                      <h3>{group.name}</h3>
                      <div>
                        <IconButton icon="edit" label={`Edit ${group.name}`} onClick={() => setGroupEditor(group)} />
                        <IconButton
                          icon="trash"
                          label={`Delete ${group.name}`}
                          disabled={state.rulesets.some((ruleset) => ruleset.entries.some((entry) => entry.matchGroupId === group.id))}
                          onClick={() => void run(`delete-group-${group.id}`, () => api.deleteMatchGroup(group.id))}
                        />
                      </div>
                    </header>
                    <ul>
                      {group.conditions.map((condition) => (
                        <li key={condition.id}>
                          <span>{conditionLabels[condition.kind]}</span>
                          <code>{condition.value}</code>
                        </li>
                      ))}
                    </ul>
                  </article>
                ))}
              </div>
            </>
          )}
        </section>
      </div>

      {groupEditor ? (
        <MatchGroupEditor
          group={groupEditor === 'new' ? null : groupEditor}
          busy={busy === 'group'}
          onClose={() => setGroupEditor(null)}
          onSave={async (input) => {
            setBusy('group')
            try {
              onSnapshot(await api.saveMatchGroup(input))
              setGroupEditor(null)
            } catch (error) {
              onError(error)
            } finally {
              setBusy(null)
            }
          }}
        />
      ) : null}

      {deleteConfirm ? (
        <Modal
          title="Delete ruleset"
          description={`Delete “${deleteConfirm.name}”? This cannot be undone.`}
          onClose={() => setDeleteConfirm(null)}
          footer={
            <>
              <Button onClick={() => setDeleteConfirm(null)}>Cancel</Button>
              <Button
                variant="danger"
                busy={busy === 'delete'}
                onClick={() => {
                  void run('delete', async () => {
                    const next = await api.deleteRuleset(deleteConfirm.id)
                    setDeleteConfirm(null)
                    setCreatingNew(false)
                    setSelectedId(null)
                    setDraft(emptyDraft())
                    return next
                  })
                }}
              >
                Delete ruleset
              </Button>
            </>
          }
        >
          <p>The active ruleset and rulesets referenced by a saved run configuration cannot be deleted.</p>
        </Modal>
      ) : null}
    </div>
  )
}

function fromRuleset(ruleset: Ruleset): RulesetDraft {
  return {
    id: ruleset.id,
    name: ruleset.name,
    entries: structuredClone(ruleset.entries),
    miscellaneous: ruleset.miscellaneous,
  }
}

function firstAvailableNode(state: Snapshot['state']) {
  return state.nodes.find((node) => !node.missing)
}

function groupNodes(nodes: Snapshot['state']['nodes']): Array<[string, Snapshot['state']['nodes']]> {
  const map = new Map<string, Snapshot['state']['nodes']>()
  for (const node of nodes) {
    const list = map.get(node.serviceTitle) ?? []
    list.push(node)
    map.set(node.serviceTitle, list)
  }
  return [...map.entries()]
}

function MatchGroupEditor({
  group,
  busy,
  onClose,
  onSave,
}: {
  group: MatchGroup | null
  busy: boolean
  onClose: () => void
  onSave: (input: {
    id: string | null
    name: string
    conditions: Array<{ kind: ConditionKind; value: string }>
  }) => Promise<void>
}) {
  const [name, setName] = useState(group?.name ?? '')
  const [conditions, setConditions] = useState(
    group?.conditions.map((condition) => ({ kind: condition.kind, value: condition.value })) ?? [
      { kind: 'process_name' as ConditionKind, value: '' },
    ],
  )

  return (
    <Modal
      title={group ? 'Edit match group' : 'New match group'}
      description="Conditions within this group are OR: any one match routes to the group's destination."
      onClose={onClose}
      wide
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            busy={busy}
            onClick={() => void onSave({ id: group?.id ?? null, name, conditions })}
          >
            Save match group
          </Button>
        </>
      }
    >
      <Field label="Group name">
        <input data-autofocus value={name} maxLength={80} placeholder="OpenAI" onChange={(event) => setName(event.target.value)} />
      </Field>
      <div className="condition-editor">
        <div className="condition-editor-head"><span>Conditions</span><span>Any match</span></div>
        {conditions.map((condition, index) => (
          <div className="condition-row" key={index}>
            <select
              aria-label={`Condition ${index + 1} type`}
              value={condition.kind}
              onChange={(event) =>
                setConditions((current) =>
                  current.map((item, itemIndex) =>
                    itemIndex === index ? { ...item, kind: event.target.value as ConditionKind } : item,
                  ),
                )
              }
            >
              {Object.entries(conditionLabels).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
            </select>
            <input
              aria-label={`Condition ${index + 1} value`}
              value={condition.value}
              placeholder={placeholderFor(condition.kind)}
              onChange={(event) =>
                setConditions((current) =>
                  current.map((item, itemIndex) =>
                    itemIndex === index ? { ...item, value: event.target.value } : item,
                  ),
                )
              }
            />
            <IconButton
              icon="trash"
              label={`Remove condition ${index + 1}`}
              disabled={conditions.length === 1}
              onClick={() => setConditions((current) => current.filter((_, itemIndex) => itemIndex !== index))}
            />
          </div>
        ))}
        <Button variant="quiet" icon="plus" onClick={() => setConditions((current) => [...current, { kind: 'domain_suffix', value: '' }])}>
          Add condition
        </Button>
      </div>
    </Modal>
  )
}

function placeholderFor(kind: ConditionKind): string {
  switch (kind) {
    case 'process_name': return 'ChatGPT.exe'
    case 'process_path': return 'C:\\Program Files\\App\\app.exe'
    case 'domain': return 'example.com'
    case 'domain_suffix': return 'chatgpt.com'
    case 'ipv4_cidr': return '203.0.113.0/24'
    case 'ipv6_cidr': return '2001:db8::/32'
  }
}
