import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from 'react'
import { api, subscribeNodeTestProgress } from '../api'
import type { ImportPreview, NodeTestProgress, NodeView, Snapshot, SourceType } from '../types'
import { allowanceProgress, allowanceSummary, formatBytes, formatDate, formatExpiry } from '../lib/format'
import {
  applyNodeOrder,
  loadNodesUiPreferences,
  moveNodeByOffset,
  placeNodeId,
  sameNodeOrder,
  saveNodesUiPreferences,
  type NodeDropPlacement,
  type NodesUiPreferences,
} from '../lib/nodesUiState'
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

interface NodeDragState {
  serviceId: string
  nodeId: string
  pointerId: number
  startX: number
  startY: number
  moved: boolean
  targetId: string | null
  placement: NodeDropPlacement | null
}

interface GroupDragState {
  serviceId: string
  pointerId: number
  startX: number
  startY: number
  moved: boolean
  targetId: string | null
  placement: NodeDropPlacement | null
}

export function NodesPage({ snapshot, onSnapshot, onError }: Props) {
  const { state } = snapshot
  const now = useNowMs(1000)
  const [search, setSearch] = useState('')
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
  const [uiPreferences, setUiPreferences] = useState<NodesUiPreferences>(() => loadNodesUiPreferences())
  const [dragState, setDragState] = useState<NodeDragState | null>(null)
  const [groupDragState, setGroupDragState] = useState<GroupDragState | null>(null)
  const [orderFeedback, setOrderFeedback] = useState('')
  const dragRef = useRef<NodeDragState | null>(null)
  const groupDragRef = useRef<GroupDragState | null>(null)
  const isDragging = dragState !== null || groupDragState !== null

  useEffect(() => {
    saveNodesUiPreferences(uiPreferences)
  }, [uiPreferences])

  useEffect(() => {
    if (!isDragging) return
    const cancelDrag = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      event.preventDefault()
      dragRef.current = null
      groupDragRef.current = null
      setDragState(null)
      setGroupDragState(null)
      setOrderFeedback('Reorder cancelled.')
    }
    document.body.classList.add('is-node-dragging')
    window.addEventListener('keydown', cancelDrag)
    return () => {
      document.body.classList.remove('is-node-dragging')
      window.removeEventListener('keydown', cancelDrag)
    }
  }, [isDragging])

  useEffect(() => {
    let mounted = true
    let unlisten: (() => void) | null = null
    void subscribeNodeTestProgress((progress) => {
      if (!mounted) return
      setSweepProgress(progress)
      if (progress.nodeId) {
        setTestFeedback((current) => {
          const next = { ...current }
          if (progress.latencyMs == null) next[progress.nodeId!] = 'Timeout'
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

  const orderedNodesByService = useMemo(() => {
    const byService = new Map<string, NodeView[]>()
    for (const service of state.services) {
      const serviceNodes = state.nodes.filter((node) => node.serviceId === service.id)
      byService.set(service.id, applyNodeOrder(serviceNodes, uiPreferences.nodeOrderByService[service.id]))
    }
    return byService
  }, [state.nodes, state.services, uiPreferences.nodeOrderByService])

  const orderedServices = useMemo(
    () => applyNodeOrder(state.services, uiPreferences.serviceOrder),
    [state.services, uiPreferences.serviceOrder],
  )

  const filtered = useMemo(() => {
    const query = search.trim().toLowerCase()
    const allNodes = orderedServices.flatMap((service) => orderedNodesByService.get(service.id) ?? [])
    if (!query) return allNodes
    return allNodes.filter((node) =>
      [node.displayName, node.serviceTitle, node.protocol].some((value) =>
        value.toLowerCase().includes(query),
      ),
    )
  }, [orderedNodesByService, orderedServices, search])

  const collapsedServiceIds = useMemo(
    () => new Set(uiPreferences.collapsedServiceIds),
    [uiPreferences.collapsedServiceIds],
  )
  const reorderLocked = search.trim().length > 0
  const allCollapsed = orderedServices.length > 0
    && orderedServices.every((service) => collapsedServiceIds.has(service.id))

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

  const setServiceCollapsed = (serviceId: string, nextCollapsed: boolean) => {
    setUiPreferences((current) => {
      const collapsed = new Set(current.collapsedServiceIds)
      if (nextCollapsed) collapsed.add(serviceId)
      else collapsed.delete(serviceId)
      return { ...current, collapsedServiceIds: [...collapsed] }
    })
  }

  const setAllServicesCollapsed = (nextCollapsed: boolean) => {
    setUiPreferences((current) => ({
      ...current,
      collapsedServiceIds: nextCollapsed ? state.services.map((service) => service.id) : [],
    }))
  }

  const storeNodeOrder = (serviceId: string, nextOrder: string[]) => {
    setUiPreferences((current) => ({
      ...current,
      nodeOrderByService: {
        ...current.nodeOrderByService,
        [serviceId]: nextOrder,
      },
    }))
  }

  const storeGroupOrder = (nextOrder: string[]) => {
    setUiPreferences((current) => ({ ...current, serviceOrder: nextOrder }))
  }

  const reorderWithKeyboard = (
    event: ReactKeyboardEvent<HTMLButtonElement>,
    service: { id: string; title: string },
    node: NodeView,
  ) => {
    if (reorderLocked || (event.key !== 'ArrowUp' && event.key !== 'ArrowDown')) return
    event.preventDefault()
    const serviceNodes = orderedNodesByService.get(service.id) ?? []
    const currentOrder = serviceNodes.map((item) => item.id)
    const offset = event.key === 'ArrowUp' ? -1 : 1
    const index = currentOrder.indexOf(node.id)
    if (index < 0) return
    const nextOrder = moveNodeByOffset(currentOrder, node.id, offset)
    if (sameNodeOrder(currentOrder, nextOrder)) {
      setOrderFeedback(`${node.displayName} is already ${offset < 0 ? 'first' : 'last'} in ${service.title}.`)
      return
    }
    storeNodeOrder(service.id, nextOrder)
    const position = nextOrder.indexOf(node.id) + 1
    setOrderFeedback(`${node.displayName} moved to position ${position} of ${nextOrder.length} in ${service.title}.`)
    focusReorderControl('data-reorder-node-id', node.id)
  }

  const startNodeDrag = (
    event: ReactPointerEvent<HTMLButtonElement>,
    serviceId: string,
    nodeId: string,
  ) => {
    if (reorderLocked || event.button !== 0 || groupDragRef.current) return
    event.preventDefault()
    event.currentTarget.focus()
    event.currentTarget.setPointerCapture(event.pointerId)
    const next: NodeDragState = {
      serviceId,
      nodeId,
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      moved: false,
      targetId: null,
      placement: null,
    }
    dragRef.current = next
    setDragState(next)
  }

  const continueNodeDrag = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const current = dragRef.current
    if (!current || current.pointerId !== event.pointerId) return
    const moved = current.moved || Math.hypot(event.clientX - current.startX, event.clientY - current.startY) > 3
    const target = findNodeDropTarget(event.clientX, event.clientY, current)
    const targetId = target?.nodeId ?? null
    const placement = target?.placement ?? null
    if (moved === current.moved && targetId === current.targetId && placement === current.placement) return
    const next = { ...current, moved, targetId, placement }
    dragRef.current = next
    setDragState(next)
  }

  const finishNodeDrag = (event: ReactPointerEvent<HTMLButtonElement>, commit: boolean) => {
    const current = dragRef.current
    if (!current || current.pointerId !== event.pointerId) return
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId)
    }
    dragRef.current = null
    setDragState(null)
    if (!commit) {
      if (current.moved) setOrderFeedback('Reorder cancelled.')
      return
    }
    if (!current.targetId || !current.placement) {
      if (current.moved) setOrderFeedback('Reorder cancelled; drop a node inside this service.')
      return
    }
    const serviceNodes = orderedNodesByService.get(current.serviceId) ?? []
    const currentOrder = serviceNodes.map((node) => node.id)
    const nextOrder = placeNodeId(currentOrder, current.nodeId, current.targetId, current.placement)
    if (sameNodeOrder(currentOrder, nextOrder)) {
      setOrderFeedback('Order unchanged.')
      focusReorderControl('data-reorder-node-id', current.nodeId)
      return
    }
    storeNodeOrder(current.serviceId, nextOrder)
    const movedNode = serviceNodes.find((node) => node.id === current.nodeId)
    const position = nextOrder.indexOf(current.nodeId) + 1
    setOrderFeedback(`${movedNode?.displayName ?? 'Node'} moved to position ${position} of ${nextOrder.length}.`)
    focusReorderControl('data-reorder-node-id', current.nodeId)
  }

  const moveGroupWithKeyboard = (
    event: ReactKeyboardEvent<HTMLButtonElement>,
    service: { id: string; title: string },
  ) => {
    if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return
    event.preventDefault()
    const currentOrder = orderedServices.map((item) => item.id)
    const offset = event.key === 'ArrowUp' ? -1 : 1
    const index = currentOrder.indexOf(service.id)
    if (index < 0) return
    const nextOrder = moveNodeByOffset(currentOrder, service.id, offset)
    if (sameNodeOrder(currentOrder, nextOrder)) {
      setOrderFeedback(`${service.title} group is already ${offset < 0 ? 'first' : 'last'}.`)
      return
    }
    storeGroupOrder(nextOrder)
    const position = nextOrder.indexOf(service.id) + 1
    setOrderFeedback(`${service.title} group moved to position ${position} of ${nextOrder.length}.`)
    focusReorderControl('data-group-reorder-id', service.id)
  }

  const startGroupDrag = (
    event: ReactPointerEvent<HTMLButtonElement>,
    serviceId: string,
  ) => {
    if (event.button !== 0 || dragRef.current) return
    event.preventDefault()
    event.currentTarget.focus()
    event.currentTarget.setPointerCapture(event.pointerId)
    const next: GroupDragState = {
      serviceId,
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      moved: false,
      targetId: null,
      placement: null,
    }
    groupDragRef.current = next
    setGroupDragState(next)
  }

  const continueGroupDrag = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const current = groupDragRef.current
    if (!current || current.pointerId !== event.pointerId) return
    const moved = current.moved || Math.hypot(event.clientX - current.startX, event.clientY - current.startY) > 3
    const target = findGroupDropTarget(event.clientX, event.clientY, current)
    const targetId = target?.serviceId ?? null
    const placement = target?.placement ?? null
    if (moved === current.moved && targetId === current.targetId && placement === current.placement) return
    const next = { ...current, moved, targetId, placement }
    groupDragRef.current = next
    setGroupDragState(next)
  }

  const finishGroupDrag = (event: ReactPointerEvent<HTMLButtonElement>, commit: boolean) => {
    const current = groupDragRef.current
    if (!current || current.pointerId !== event.pointerId) return
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId)
    }
    groupDragRef.current = null
    setGroupDragState(null)
    if (!commit) {
      if (current.moved) setOrderFeedback('Reorder cancelled.')
      return
    }
    if (!current.targetId || !current.placement) {
      if (current.moved) setOrderFeedback('Group move cancelled; drop on a service header.')
      return
    }
    const currentOrder = orderedServices.map((service) => service.id)
    const nextOrder = placeNodeId(currentOrder, current.serviceId, current.targetId, current.placement)
    if (sameNodeOrder(currentOrder, nextOrder)) {
      setOrderFeedback('Group order unchanged.')
      focusReorderControl('data-group-reorder-id', current.serviceId)
      return
    }
    storeGroupOrder(nextOrder)
    const movedService = orderedServices.find((service) => service.id === current.serviceId)
    const position = nextOrder.indexOf(current.serviceId) + 1
    setOrderFeedback(`${movedService?.title ?? 'Service'} group moved to position ${position} of ${nextOrder.length}.`)
    focusReorderControl('data-group-reorder-id', current.serviceId)
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
      <p id="node-order-instructions" className="sr-only">
        Drag a node handle, or focus it and press Up or Down, to reorder nodes within its service.
      </p>
      <p id="group-order-instructions" className="sr-only">
        Drag a service group handle, or focus it and press Up or Down, to move that service group.
      </p>
      <p id="node-order-disabled-hint" className="sr-only">Clear the search to reorder nodes.</p>
      <p className="sr-only" role="status" aria-live="polite">{orderFeedback}</p>

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
        {state.services.length > 1 ? (
          <Button
            variant="quiet"
            icon={allCollapsed ? 'chevron-down' : 'chevron-up'}
            className="collapse-all-button"
            aria-label={allCollapsed ? 'Expand all service node lists' : 'Collapse all service node lists'}
            onClick={() => setAllServicesCollapsed(!allCollapsed)}
          >
            {allCollapsed ? 'Expand all' : 'Collapse all'}
          </Button>
        ) : null}
      </Toolbar>
      {reorderLocked ? <p className="node-order-hint">Clear search to reorder nodes.</p> : null}
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
          {orderedServices.map((service) => {
            const nodes = filtered.filter((node) => node.serviceId === service.id)
            const hasAvailableNodes = state.nodes.some((node) => node.serviceId === service.id && !node.missing)
            const isCollapsed = collapsedServiceIds.has(service.id)
            const serviceTitleId = `service-title-${service.id}`
            const nodesRegionId = `service-nodes-${service.id}`
            const progress = allowanceProgress(service.allowance)
            const groupClasses = [
              'service-card',
              groupDragState?.serviceId === service.id ? 'is-group-dragging' : '',
              groupDragState?.targetId === service.id && groupDragState.placement
                ? `is-group-drop-${groupDragState.placement}`
                : '',
            ].filter(Boolean).join(' ')
            return (
              <Panel className={groupClasses} key={service.id} data-service-id={service.id}>
                <div className="service-head">
                  <div className="service-title-wrap">
                    <div className="service-title-line">
                      <h2 id={serviceTitleId}>{service.title}</h2>
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
                    <IconButton
                      icon="grip"
                      label={`Move ${service.title} group`}
                      className="service-drag-handle"
                      data-group-reorder-id={service.id}
                      draggable={false}
                      aria-describedby="group-order-instructions"
                      aria-keyshortcuts="ArrowUp ArrowDown"
                      title={`Move ${service.title} group with the mouse, or focus and press Up or Down`}
                      onPointerDown={(event) => startGroupDrag(event, service.id)}
                      onPointerMove={continueGroupDrag}
                      onPointerUp={(event) => finishGroupDrag(event, true)}
                      onPointerCancel={(event) => finishGroupDrag(event, false)}
                      onKeyDown={(event) => moveGroupWithKeyboard(event, service)}
                    />
                    <IconButton
                      icon={isCollapsed ? 'chevron-down' : 'chevron-up'}
                      label={`${isCollapsed ? 'Expand' : 'Collapse'} ${service.title} nodes`}
                      className="service-collapse-button"
                      aria-expanded={!isCollapsed}
                      aria-controls={nodesRegionId}
                      onClick={() => setServiceCollapsed(service.id, !isCollapsed)}
                    />
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
                <div
                  className="node-table-wrap"
                  id={nodesRegionId}
                  role="region"
                  aria-labelledby={serviceTitleId}
                  hidden={isCollapsed}
                >
                  {nodes.length === 0 ? (
                    <p className="muted node-empty">No nodes match this search.</p>
                  ) : (
                    <table className="node-table" data-service-id={service.id} aria-label={`${service.title} nodes`}>
                      <thead>
                        <tr>
                          <th scope="col" className="node-order-column"><span className="sr-only">Reorder</span></th>
                          <th scope="col">Node</th>
                          <th scope="col">Protocol</th>
                          <th scope="col">Latency / State</th>
                        </tr>
                      </thead>
                      <tbody>
                        {nodes.map((node) => {
                          const rowClassName = [
                            node.missing ? 'is-missing' : '',
                            dragState?.nodeId === node.id ? 'is-dragging' : '',
                            dragState?.serviceId === service.id
                              && dragState.targetId === node.id
                              && dragState.placement
                              ? `is-drop-${dragState.placement}`
                              : '',
                          ].filter(Boolean).join(' ')
                          return (
                            <tr key={node.id} data-node-id={node.id} className={rowClassName}>
                              <td className="node-order-cell">
                                <IconButton
                                  icon="grip"
                                  label={`Reorder ${node.displayName} in ${service.title}`}
                                  className="node-drag-handle"
                                  data-reorder-node-id={node.id}
                                  draggable={false}
                                  disabled={reorderLocked}
                                  aria-describedby={reorderLocked ? 'node-order-disabled-hint' : 'node-order-instructions'}
                                  aria-keyshortcuts="ArrowUp ArrowDown"
                                  title={reorderLocked
                                    ? 'Clear search to reorder nodes'
                                    : `Reorder ${node.displayName} with the mouse, or focus and press Up or Down`}
                                  onPointerDown={(event) => startNodeDrag(event, service.id, node.id)}
                                  onPointerMove={continueNodeDrag}
                                  onPointerUp={(event) => finishNodeDrag(event, true)}
                                  onPointerCancel={(event) => finishNodeDrag(event, false)}
                                  onKeyDown={(event) => reorderWithKeyboard(event, service, node)}
                                />
                              </td>
                              <td><strong>{node.displayName}</strong></td>
                              <td>{node.protocol}</td>
                              <td>
                                {node.missing ? (
                                  <SourceBadge tone="red">Missing · choose replacement</SourceBadge>
                                ) : (
                                  <div className="node-latency-cell">
                                    <Button
                                      variant="quiet"
                                      className="node-test-button"
                                      busy={testingNodeId === node.id}
                                      disabled={!snapshot.runtime.vpnActive || testingNodeId !== null || testingAll || busy !== null}
                                      aria-label={`Test ${node.displayName} latency`}
                                      aria-describedby={`node-latency-${node.id}`}
                                      title={snapshot.runtime.vpnActive ? `Test ${node.displayName} latency` : 'Connect Clashfork to test node latency'}
                                      onClick={() => {
                                        setTestingNodeId(node.id)
                                        setTestFeedback((current) => {
                                          const next = { ...current }
                                          delete next[node.id]
                                          return next
                                        })
                                        void api.testNode(node.id).then(onSnapshot).catch((error: unknown) => {
                                          setTestFeedback((current) => ({ ...current, [node.id]: nodeTestFailureText(error, 'Timeout') }))
                                        }).finally(() => setTestingNodeId(null))
                                      }}
                                    >
                                      <span id={`node-latency-${node.id}`} role="status">
                                        {testingNodeId === node.id ? 'Testing…' : testFeedback[node.id] ? (
                                          <span className="node-test-feedback">{testFeedback[node.id]}</span>
                                        ) : (
                                          <LatencyValue node={node} now={now} />
                                        )}
                                      </span>
                                    </Button>
                                    {node.omittedOnce ? (
                                      <SourceBadge tone="amber">Omitted once · awaiting confirmation</SourceBadge>
                                    ) : null}
                                  </div>
                                )}
                              </td>
                            </tr>
                          )
                        })}
                      </tbody>
                    </table>
                  )}
                </div>
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

function nodeTestFailureText(error: unknown, fallback = 'No response'): string {
  const message = error instanceof Error ? error.message : String(error)
  if (message.includes('Connect Clashfork')) return 'Connect Clashfork to test'
  if (message.includes('Reconnect Clashfork')) return 'Reconnect to test this update'
  return fallback
}

function findNodeDropTarget(
  clientX: number,
  clientY: number,
  drag: Pick<NodeDragState, 'serviceId' | 'nodeId'>,
): { nodeId: string; placement: NodeDropPlacement } | null {
  const element = document.elementFromPoint(clientX, clientY)
  const row = element instanceof Element ? element.closest<HTMLTableRowElement>('tr[data-node-id]') : null
  const table = row?.closest<HTMLTableElement>('table[data-service-id]')
  if (!row || !table || table.dataset.serviceId !== drag.serviceId) return null
  const nodeId = row.dataset.nodeId
  if (!nodeId || nodeId === drag.nodeId) return null
  const bounds = row.getBoundingClientRect()
  return {
    nodeId,
    placement: clientY < bounds.top + bounds.height / 2 ? 'before' : 'after',
  }
}

function findGroupDropTarget(
  clientX: number,
  clientY: number,
  drag: Pick<GroupDragState, 'serviceId'>,
): { serviceId: string; placement: NodeDropPlacement } | null {
  const element = document.elementFromPoint(clientX, clientY)
  const header = element instanceof Element ? element.closest<HTMLElement>('.service-head') : null
  const card = header?.closest<HTMLElement>('.service-card')
  const serviceId = card?.dataset.serviceId
  if (!header || !serviceId || serviceId === drag.serviceId) return null
  const bounds = header.getBoundingClientRect()
  return {
    serviceId,
    placement: clientY < bounds.top + bounds.height / 2 ? 'before' : 'after',
  }
}

function focusReorderControl(
  attribute: 'data-reorder-node-id' | 'data-group-reorder-id',
  id: string,
) {
  requestAnimationFrame(() => {
    for (const control of document.querySelectorAll<HTMLButtonElement>(`[${attribute}]`)) {
      if (control.getAttribute(attribute) === id) {
        control.focus()
        return
      }
    }
  })
}
