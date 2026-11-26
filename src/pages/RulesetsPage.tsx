import { useEffect, useRef, useState } from 'react'
import { api, isTauri } from '../api'
import { SelectedNodeLatency } from '../components/NodeLatency'
import { useNowMs } from '../hooks'
import { downloadJsonFile } from '../lib/download'
import { safeExportFileName } from '../lib/exportName'
import type { ConditionKind, Destination, MatchCondition, NodeView, Ruleset, RulesetEntry, Snapshot } from '../types'
import { Banner, Button, EmptyState, Field, IconButton, Modal, PageHeader, Panel, Toolbar } from '../components/ui'

interface Props { snapshot: Snapshot; onSnapshot: (snapshot: Snapshot) => void; onError: (error: unknown) => void }
type Draft = { id: string | null; name: string; entries: RulesetEntry[]; miscellaneous: Destination }
type PortableDestination = { kind: 'direct' } | { kind: 'block' } | { kind: 'proxy'; proxyRef: string; label: string }
type PortableRule = { name: string; conditions: Array<{ kind: ConditionKind; value: string }>; destination: PortableDestination }
type PortableDoc = { format: 'clashfork-routing'; version: 1; type: 'rule' | 'ruleset'; name?: string; rules?: PortableRule[]; rule?: PortableRule; miscellaneous?: 'direct' | 'block' | PortableDestination }
const kinds: Record<ConditionKind, string> = { process_name: 'Process name', process_path: 'Process path', domain: 'Domain exact', domain_suffix: 'Domain suffix', ipv4_cidr: 'IPv4 CIDR', ipv6_cidr: 'IPv6 CIDR' }
const newId = () => `local_${crypto.randomUUID()}`
const makeCondition = (kind: ConditionKind = 'domain_suffix', value = ''): MatchCondition => ({ id: newId(), kind, value })
const validPortableCondition = (kind: ConditionKind, input: string) => {
  const value = input.trim()
  if (!value || value.length > 512 || /[,\x00-\x1f\x7f]/.test(value)) return false
  if (kind === 'process_name') return !/[\\/:]/.test(value)
  if (kind === 'process_path') return value.startsWith('/') || value.startsWith('\\\\') || /^[A-Za-z]:/.test(value)
  if (kind === 'domain' || kind === 'domain_suffix') {
    if (value.includes('://') || /[/:]/.test(value)) return false
    const domain = value.replace(/^\.+|\.+$/g, '')
    return domain.length <= 253 && domain.split('.').every(label => label.length > 0 && label.length <= 63 && /^[A-Za-z0-9_-]+$/.test(label) && !label.startsWith('-') && !label.endsWith('-'))
  }
  const [address, prefix, ...extra] = value.split('/')
  if (extra.length || !/^\d+$/.test(prefix ?? '')) return false
  const bits = Number(prefix)
  if (kind === 'ipv4_cidr') {
    const octets = address.split('.')
    return bits <= 32 && octets.length === 4 && octets.every(octet => /^\d{1,3}$/.test(octet) && Number(octet) <= 255)
  }
  if (bits > 128) return false
  try { return new URL(`http://[${address}]/`).hostname.startsWith('[') } catch { return false }
}
const emptyDraft = (): Draft => ({ id: null, name: '', entries: [], miscellaneous: { kind: 'direct' } })
const entryFrom = (name: string, conditions: MatchCondition[] = [makeCondition()]): RulesetEntry => ({ id: newId(), matchGroupId: '', name, conditions, destination: { kind: 'direct' } })
const fromRuleset = (r: Ruleset): Draft => ({ id: r.id, name: r.name, entries: structuredClone(r.entries), miscellaneous: r.miscellaneous })

function destinationValue(destination: Destination): string {
  return destination.kind === 'node' ? `node:${destination.nodeId}` : destination.kind
}

function destinationFromValue(value: string): Destination {
  return value.startsWith('node:') ? { kind: 'node', nodeId: value.slice(5) } : { kind: value as 'direct' | 'block' }
}

function portableDestination(nodeId: string, nodes: NodeView[], refs: Map<string, string>): PortableDestination {
  const node = nodes.find(candidate => candidate.id === nodeId)
  const ref = refs.get(nodeId) ?? `proxy-${refs.size + 1}`
  refs.set(nodeId, ref)
  return { kind: 'proxy', proxyRef: ref, label: `Proxy ${Number(ref.slice(6)) || 1}${node ? ` · ${node.protocol}` : ''}` }
}

function DestinationPicker({ destination, nodes, label, showLabel = true, inlineLatency = false, now, onChange }: {
  destination: Destination
  nodes: NodeView[]
  label: string
  showLabel?: boolean
  inlineLatency?: boolean
  now: number
  onChange: (destination: Destination) => void
}) {
  const selectedNode = destination.kind === 'node' ? nodes.find(node => node.id === destination.nodeId) : undefined
  const groups = new Map<string, NodeView[]>()
  for (const node of nodes.filter(item => !item.missing)) {
    const list = groups.get(node.serviceTitle) ?? []
    list.push(node)
    groups.set(node.serviceTitle, list)
  }
  return <label className={`field unified-destination-field ${showLabel ? '' : 'is-compact'} ${inlineLatency ? 'is-row' : ''}`}>
    <span className={showLabel ? 'field-label' : 'sr-only'}>Destination</span>
    <select aria-label={label} value={destinationValue(destination)} onChange={event => onChange(destinationFromValue(event.target.value))}>
      <option value="direct">Direct</option>
      <option value="block">Block</option>
      {selectedNode?.missing && <option value={`node:${selectedNode.id}`} disabled>{selectedNode.displayName} · Missing</option>}
      {[...groups].map(([service, serviceNodes]) => <optgroup key={service} label={service}>{serviceNodes.map(node => <option key={node.id} value={`node:${node.id}`}>{node.displayName} · {node.protocol}</option>)}</optgroup>)}
      {destination.kind === 'node' && !selectedNode && <option value={`node:${destination.nodeId}`} disabled>Unavailable proxy · missing</option>}
    </select>
    {selectedNode && <SelectedNodeLatency node={selectedNode} now={now} />}
  </label>
}

export function RulesetsPage({ snapshot, onSnapshot, onError }: Props) {
  const { state } = snapshot
  const now = useNowMs(1000)
  const [selectedId, setSelectedId] = useState<string | null>(state.selection.rulesetId ?? state.rulesets[0]?.id ?? null)
  const [draftKey, setDraftKey] = useState<string | null>(null)
  const [draftKeys, setDraftKeys] = useState<string[]>([])
  const [draft, setDraft] = useState<Draft>(() => { const r = state.rulesets.find(x => x.id === state.selection.rulesetId) ?? state.rulesets[0]; return r ? fromRuleset(r) : emptyDraft() })
  const [creating, setCreating] = useState(false)
  const [expanded, setExpanded] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [importDoc, setImportDoc] = useState<PortableDoc | null>(null)
  const [proxyMap, setProxyMap] = useState<Record<string, string>>({})
  const [importName, setImportName] = useState('')
  const [exportNotice, setExportNotice] = useState<string | null>(null)
  const fileRef = useRef<HTMLInputElement>(null)
  const draftCache = useRef<Record<string, Draft>>({})

  useEffect(() => {
    if (!creating && selectedId && !state.rulesets.some(r => r.id === selectedId)) { const first = state.rulesets[0]; setSelectedId(first?.id ?? null); setDraft(first ? fromRuleset(first) : emptyDraft()) }
  }, [creating, selectedId, state.rulesets])
  const selected = state.rulesets.find(r => r.id === selectedId)

  const cacheDraft = () => { draftCache.current[creating ? draftKey ?? '__new__' : selectedId ?? '__new__'] = structuredClone(draft) }
  const newRuleset = () => { cacheDraft(); const key = `draft-${crypto.randomUUID()}`; draftCache.current[key] = emptyDraft(); setDraftKeys(keys => [...keys, key]); setDraftKey(key); setCreating(true); setSelectedId(null); setDraft(emptyDraft()); setExpanded(null) }
  const selectRuleset = (id: string) => {
    if (id.startsWith('draft-')) { cacheDraft(); setCreating(true); setSelectedId(null); setDraftKey(id); setDraft(draftCache.current[id] ?? emptyDraft()); setExpanded(null); return }
    const r = state.rulesets.find(x => x.id === id); if (!r) return; cacheDraft(); setCreating(false); setSelectedId(id); setDraftKey(null); setDraft(draftCache.current[id] ?? fromRuleset(r)); setExpanded(null)
  }
  const changeEntry = (id: string, fn: (e: RulesetEntry) => RulesetEntry) => setDraft(d => ({ ...d, entries: d.entries.map(e => e.id === id ? fn(e) : e) }))
  const move = (i: number, d: -1 | 1) => setDraft(old => { const entries = [...old.entries]; const j = i + d; if (j < 0 || j >= entries.length) return old; [entries[i], entries[j]] = [entries[j], entries[i]]; return { ...old, entries } })
  const save = async () => {
    if (!draft.name.trim()) return onError(new Error('Name the ruleset before saving.'))
    setBusy(true)
    try {
      const next = await api.saveRuleset({ id: draft.id, name: draft.name, entries: draft.entries, miscellaneous: draft.miscellaneous })
      onSnapshot(next)
      const saved = next.state.rulesets.find(r => r.id === draft.id) ?? [...next.state.rulesets].reverse().find(r => r.name === draft.name.trim())
      if (saved) { delete draftCache.current[draftKey ?? '__new__']; delete draftCache.current[saved.id]; setDraftKeys(keys => keys.filter(key => key !== draftKey)); setCreating(false); setSelectedId(saved.id); setDraftKey(null); setDraft(fromRuleset(saved)) }
    } catch (e) { onError(e) } finally { setBusy(false) }
  }
  const exportDoc = async (type: 'rule' | 'ruleset', entry?: RulesetEntry) => {
    const entries = type === 'rule' && entry ? [entry] : draft.entries
    const invalidRule = entries.some(e =>
      !e.name.trim()
      || e.name.length > 80
      || e.conditions.length < 1
      || e.conditions.length > 50
      || new Set(e.conditions.map(c => `${c.kind}:${c.value.trim().toLowerCase()}`)).size !== e.conditions.length
      || e.conditions.some(c => !validPortableCondition(c.kind, c.value))
    )
    if (
      entries.length > 200
      || invalidRule
      || (type === 'ruleset' && (!draft.name.trim() || draft.name.length > 80))
    ) {
      onError(new Error('Routing files can be exported only when rules and conditions are valid.'))
      return
    }
    const refs = new Map<string, string>()
    const rules: PortableRule[] = entries.map(e => {
      let destination: PortableDestination
      if (e.destination.kind === 'node') {
        const nodeId = e.destination.nodeId
        const node = state.nodes.find(n => n.id === nodeId)
        const ref = refs.get(nodeId) ?? `proxy-${refs.size + 1}`
        refs.set(nodeId, ref)
        destination = { kind: 'proxy', proxyRef: ref, label: `Proxy ${Number(ref.slice(6)) || 1}${node ? ` · ${node.protocol}` : ''}` }
      } else destination = { kind: e.destination.kind }
      return { name: e.name, conditions: e.conditions.map(({ kind, value }) => ({ kind, value })), destination }
    })
    let miscellaneous: PortableDoc['miscellaneous']
    if (draft.miscellaneous.kind === 'node') miscellaneous = portableDestination(draft.miscellaneous.nodeId, state.nodes, refs)
    else miscellaneous = draft.miscellaneous.kind
    const doc: PortableDoc = { format: 'clashfork-routing', version: 1, type, ...(type === 'ruleset' ? { name: draft.name, rules, miscellaneous } : { rule: rules[0] }) }
    const contents = JSON.stringify(doc, null, 2)
    const baseName = type === 'ruleset' ? draft.name : entry?.name ?? 'rule'
    const fileName = safeExportFileName(`${type === 'ruleset' ? 'clashfork-ruleset' : 'clashfork-rule'}-${baseName}`, `clashfork-${type}`)
    try {
      if (isTauri) {
        const result = await api.saveRoutingExport(fileName, contents)
        if (result.canceled) return
        setExportNotice(`Saved ${result.fileName ?? fileName}.`)
      } else {
        downloadJsonFile(contents, fileName)
        setExportNotice(`Downloaded ${fileName}.`)
      }
    } catch (error) {
      onError(error)
    }
  }
  const readFile = async (file?: File) => {
    if (!file) return
    try {
      if (file.size > 1_000_000) throw new Error('Routing files must be under 1 MB.')
      const d = JSON.parse(await file.text()) as PortableDoc
      if (d.format !== 'clashfork-routing' || d.version !== 1 || !['rule', 'ruleset'].includes(d.type)) throw new Error('Unsupported routing file format or version.')
      const rules = (d.type === 'rule' ? (d.rule ? [d.rule] : []) : d.rules) as PortableRule[] | undefined
      if (!Array.isArray(rules) || rules.length > 200 || (d.type === 'rule' && rules.length !== 1)) throw new Error('The file must contain 1–200 rules.')
      for (const r of rules) {
        if (!r || typeof r.name !== 'string' || !r.name.trim() || r.name.length > 80 || !Array.isArray(r.conditions) || r.conditions.length < 1 || r.conditions.length > 50) throw new Error('Each rule needs a name and 1–50 conditions.')
        if (r.conditions.some(c => !Object.hasOwn(kinds, c.kind) || typeof c.value !== 'string' || !validPortableCondition(c.kind, c.value))) throw new Error('The file contains an invalid condition value.')
        const distinctConditions = new Set(r.conditions.map(c => `${c.kind}:${c.value.trim().toLowerCase()}`))
        if (distinctConditions.size !== r.conditions.length) throw new Error('A rule cannot repeat the same condition.')
        const dest = r.destination
        if (!dest || !['direct', 'block', 'proxy'].includes(dest.kind) || (dest.kind === 'proxy' && (typeof dest.proxyRef !== 'string' || !/^proxy-[A-Za-z0-9_-]{1,58}$/.test(dest.proxyRef) || typeof dest.label !== 'string' || !dest.label.trim() || dest.label.length > 100))) throw new Error('The file contains an invalid destination.')
      }
      if (d.type === 'ruleset') {
        const misc = d.miscellaneous
        if (typeof d.name !== 'string' || !d.name.trim() || d.name.length > 80 || !misc || (typeof misc === 'string' ? !['direct', 'block'].includes(misc) : misc.kind !== 'proxy' || typeof misc.proxyRef !== 'string' || !/^proxy-[A-Za-z0-9_-]{1,58}$/.test(misc.proxyRef) || typeof misc.label !== 'string' || !misc.label.trim() || misc.label.length > 100)) throw new Error('The ruleset name or catch-all is invalid.')
      }
      const map: Record<string, string> = {}
      for (const r of rules) if (r.destination.kind === 'proxy') map[r.destination.proxyRef] ??= ''
      if (d.type === 'ruleset' && typeof d.miscellaneous === 'object' && d.miscellaneous.kind === 'proxy') map[d.miscellaneous.proxyRef] ??= ''
      setProxyMap(map)
      const base = d.type === 'ruleset' ? d.name || 'Imported ruleset' : draft.name || 'Imported ruleset'
      const usedNames = new Set([
        ...state.rulesets.map(r => r.name.toLowerCase()),
        ...draftKeys.map(key => draftCache.current[key]?.name.toLowerCase()).filter((name): name is string => Boolean(name)),
        draft.name.toLowerCase(),
      ])
      let candidate = base
      if (usedNames.has(candidate.toLowerCase())) {
        candidate = `${base} (imported)`
        let suffix = 2
        while (usedNames.has(candidate.toLowerCase())) candidate = `${base} (imported ${suffix++})`
      }
      setImportName(candidate)
      setImportDoc(d)
    } catch (e) { onError(e) }
  }
  const proxyRefs = Object.keys(proxyMap)
  const importRules = importDoc ? (importDoc.type === 'rule' ? [importDoc.rule!] : importDoc.rules!) : []
  const acceptImport = () => {
    if (!importDoc || proxyRefs.some(ref => !proxyMap[ref])) return
    const entries = importRules.map(rule => {
      const destination = rule.destination.kind === 'proxy'
        ? { kind: 'node' as const, nodeId: proxyMap[rule.destination.proxyRef] }
        : { kind: rule.destination.kind }
      return {
        ...entryFrom(rule.name, rule.conditions.map(condition => makeCondition(condition.kind, condition.value))),
        destination,
      }
    })

    if (importDoc.type === 'rule') {
      setDraft(current => ({ ...current, entries: [...current.entries, ...entries] }))
      setExpanded(entries[0]?.id ?? null)
    } else {
      cacheDraft()
      const key = `draft-${crypto.randomUUID()}`
      const importedMiscellaneous = importDoc.miscellaneous
      const miscellaneous = typeof importedMiscellaneous === 'object' && importedMiscellaneous.kind === 'proxy'
        ? { kind: 'node' as const, nodeId: proxyMap[importedMiscellaneous.proxyRef] }
        : { kind: typeof importedMiscellaneous === 'string' ? importedMiscellaneous : 'direct' as const }
      const importedDraft = { id: null, name: importName, entries, miscellaneous }

      draftCache.current[key] = importedDraft
      setDraftKeys(keys => [...keys, key])
      setDraftKey(key)
      setDraft(importedDraft)
      setCreating(true)
      setSelectedId(null)
      setExpanded(null)
    }
    setImportDoc(null)
  }

  const importDescription = proxyRefs.length > 0
    ? 'Choose local nodes for the proxy destinations. Save the draft to apply it.'
    : 'Save the draft to apply it.'

  return <div className="rulesets-page">
    <PageHeader title="Rulesets" actions={<Button variant="primary" onClick={newRuleset}>New ruleset</Button>} />
    {exportNotice ? <Banner tone="info" onDismiss={() => setExportNotice(null)}>{exportNotice}</Banner> : null}
    <Panel className="unified-rules-panel">
      <Toolbar className="unified-rules-toolbar">
        <Field label="Ruleset" className="unified-rules-select"><select aria-label="Ruleset" value={creating ? draftKey ?? '' : selectedId ?? ''} onChange={e => e.target.value && selectRuleset(e.target.value)}>
          {!state.rulesets.length && !creating && draftKeys.length === 0 && <option value="">No rulesets yet</option>}
          {state.rulesets.map(r => <option key={r.id} value={r.id}>{r.name}{state.selection.rulesetId === r.id ? ' · Active' : ''}</option>)}
          {draftKeys.map(key => <option key={key} value={key}>{draftCache.current[key]?.name || 'New ruleset'} · Draft</option>)}
        </select></Field>
        <div className="unified-file-actions"><input ref={fileRef} className="sr-only" type="file" accept=".json,application/json" aria-label="Import routing JSON" onChange={e => { void readFile(e.target.files?.[0]); e.currentTarget.value = '' }} /><Button onClick={() => fileRef.current?.click()}>Import JSON</Button><Button onClick={() => exportDoc('ruleset')} disabled={!draft.name}>Export ruleset</Button></div>
      </Toolbar>
      {!creating && !selected && !draft.name ? <EmptyState title="No rulesets yet" description="Create a ruleset to route apps and domains." /> : <>
        <div className="unified-rules-head"><input aria-label="Ruleset name" maxLength={80} placeholder="Ruleset name" value={draft.name} onChange={e => setDraft(d => ({ ...d, name: e.target.value }))} /><span className="unified-spacer"/><Button onClick={() => setDraft(selected ? fromRuleset(selected) : emptyDraft())}>Undo</Button><Button variant="primary" busy={busy} onClick={() => void save()}>Save</Button></div>
        <div className="unified-rules-list" aria-label="Rules">
          {draft.entries.map((entry, i) => {
            const destination = entry.destination
            const isOpen = expanded === entry.id
            return <article className={`unified-rule ${isOpen ? 'is-expanded' : ''}`} key={entry.id}>
              <div className="unified-rule-summary">
                <button className="unified-rule-expand" aria-expanded={isOpen} onClick={() => setExpanded(isOpen ? null : entry.id)}>
                  <span className="unified-rule-name">{entry.name || 'Untitled rule'}</span>
                </button>
                <DestinationPicker
                  destination={destination}
                  nodes={state.nodes}
                  label={`Destination for ${entry.name || 'rule'}`}
                  showLabel={false}
                  inlineLatency
                  now={now}
                  onChange={nextDestination => changeEntry(entry.id, current => ({ ...current, destination: nextDestination }))}
                />
                <div className="unified-rule-actions">
                  <IconButton icon="arrow-up" label="Move rule up" disabled={i === 0} onClick={() => move(i, -1)} />
                  <IconButton icon="arrow-down" label="Move rule down" disabled={i === draft.entries.length - 1} onClick={() => move(i, 1)} />
                  <IconButton icon="download" label="Export rule" onClick={() => exportDoc('rule', entry)} />
                  <IconButton icon="close" label="Remove rule" onClick={() => setDraft(current => ({ ...current, entries: current.entries.filter(item => item.id !== entry.id) }))} />
                </div>
              </div>
              {isOpen && <div className="unified-rule-editor"><div className="unified-edit-top"><Field label="Rule name"><input value={entry.name} maxLength={80} onChange={e => changeEntry(entry.id, x => ({ ...x, name: e.target.value }))} /></Field></div>
                <div className="unified-condition-list">{entry.conditions.map((c, ci) => <div className="unified-condition" key={c.id}><select aria-label={`Condition ${ci + 1} type`} value={c.kind} onChange={e => changeEntry(entry.id, x => ({ ...x, conditions: x.conditions.map((v,j) => j===ci ? { ...v, kind: e.target.value as ConditionKind } : v) }))}>{Object.entries(kinds).map(([k,l]) => <option key={k} value={k}>{l}</option>)}</select><input aria-label={`Condition ${ci + 1} value`} value={c.value} onChange={e => changeEntry(entry.id, x => ({ ...x, conditions: x.conditions.map((v,j) => j===ci ? { ...v, value: e.target.value } : v) }))} /><IconButton icon="close" label={`Remove condition ${ci + 1}`} disabled={entry.conditions.length <= 1} onClick={() => changeEntry(entry.id, x => ({ ...x, conditions: x.conditions.filter((_,j) => j!==ci) }))} /></div>)}</div>
                <div className="unified-condition-tools"><Button onClick={() => changeEntry(entry.id, x => ({ ...x, conditions: [...x.conditions, makeCondition()] }))}>Add condition</Button></div>
              </div>}
            </article>
          })}
          <article className="unified-rule catchall"><div className="unified-rule-summary"><span className="unified-rule-name">Everything else</span><DestinationPicker destination={draft.miscellaneous} nodes={state.nodes} label="Everything else destination" showLabel={false} now={now} onChange={miscellaneous => setDraft(d => ({ ...d, miscellaneous }))} /></div></article>
        </div>
        <div className="unified-rules-bottom"><Button onClick={() => { const e=entryFrom(`Rule ${draft.entries.length+1}`); setDraft(d => ({ ...d, entries: [...d.entries,e] })); setExpanded(e.id) }}>Add rule</Button></div>
      </>}
    </Panel>
    {importDoc && <Modal title={`Import ${importDoc.type}`} description={importDescription} onClose={() => setImportDoc(null)} wide footer={<><Button onClick={() => setImportDoc(null)}>Cancel import</Button><Button variant="primary" disabled={proxyRefs.some(ref => !proxyMap[ref])} onClick={acceptImport}>Use imported draft</Button></>}>
      {importDoc.type === 'ruleset' && <Field label="Ruleset name"><input value={importName} maxLength={80} onChange={e => setImportName(e.target.value)} /></Field>}
      <p className="field-hint">{importRules.length} rule{importRules.length===1?'':'s'} · {importRules.reduce((n,r)=>n+r.conditions.length,0)} condition{importRules.reduce((n,r)=>n+r.conditions.length,0)===1?'':'s'} · Everything else: {typeof importDoc.miscellaneous === 'string' ? importDoc.miscellaneous : importDoc.miscellaneous?.kind === 'proxy' ? importDoc.miscellaneous.label : 'unchanged'}</p>
      {proxyRefs.map(ref => {
        const ruleDestination = importRules.find(rule => (
          rule.destination.kind === 'proxy' && rule.destination.proxyRef === ref
        ))?.destination
        const fallbackDestination = importDoc.miscellaneous
        const mappedFallback = typeof fallbackDestination === 'object'
          && fallbackDestination.kind === 'proxy'
          && fallbackDestination.proxyRef === ref
          ? fallbackDestination
          : undefined
        const label = ruleDestination?.kind === 'proxy'
          ? ruleDestination.label
          : mappedFallback?.label

        return (
          <Field key={ref} label={`Map “${label ?? ref}” to local node`}>
            <select
              value={proxyMap[ref]}
              onChange={event => setProxyMap(current => ({ ...current, [ref]: event.target.value }))}
            >
              <option value="">Choose a node</option>
              {state.services.map(service => (
                <optgroup key={service.id} label={service.title}>
                  {state.nodes
                    .filter(node => node.serviceId === service.id && !node.missing)
                    .map(node => (
                      <option key={node.id} value={node.id}>
                        {node.displayName} · {node.protocol}
                      </option>
                    ))}
                </optgroup>
              ))}
            </select>
          </Field>
        )
      })}
      <div className="import-rule-preview">{importRules.map((r,i)=><div key={i}><strong>{r.name}</strong><span>{r.conditions.length} condition{r.conditions.length===1?'':'s'} → {r.destination.kind === 'proxy' ? r.destination.label : r.destination.kind}</span></div>)}</div>
    </Modal>}
  </div>
}
