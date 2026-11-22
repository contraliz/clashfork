import type {
  Allowance,
  AppState,
  CustomAppGroup,
  HistoryInput,
  HistoryReport,
  ImportPreview,
  LiveSnapshot,
  NodeView,
  ServiceView,
  Snapshot,
} from './types'

const now = () => Math.floor(Date.now() / 1000)
const useEmptyRulesetFixture =
  typeof window !== 'undefined' && new URLSearchParams(window.location.search).get('fixture') === 'empty-rulesets'

const services: ServiceView[] = [
  {
    id: 'service_falemon',
    title: 'Falemon',
    sourceType: 'subscription',
    sourceHost: 'fixture-subscription.invalid',
    nodeCount: 3,
    missingCount: 0,
    allowance: {
      kind: 'full',
      total: 100 * 1024 ** 3,
      used: 36 * 1024 ** 3,
      remaining: 64 * 1024 ** 3,
      expiresAt: now() + 18 * 86400,
      source: 'header',
    },
    stale: false,
    lastRefreshAt: now() - 12 * 60,
    lastError: null,
  },
  {
    id: 'service_northwind',
    title: 'Northwind',
    sourceType: 'subscription',
    sourceHost: 'nodes.northwind.invalid',
    nodeCount: 2,
    missingCount: 0,
    allowance: {
      kind: 'remaining',
      remaining: 41 * 1024 ** 3,
      source: 'notice',
    },
    stale: true,
    lastRefreshAt: now() - 26 * 3600,
    lastError: 'The last refresh failed. Showing the last known-good nodes and allowance.',
  },
]

const node = (
  id: string,
  serviceId: string,
  serviceTitle: string,
  displayName: string,
  protocol: string,
): NodeView => ({
  id,
  serviceId,
  serviceTitle,
  displayName,
  protocol,
  missing: false,
  coreName: `CF-${id}`,
})

let nodes: NodeView[] = [
  node('node_falemon_sg', 'service_falemon', 'Falemon', 'Singapore I0', 'VLESS'),
  node('node_falemon_tokyo', 'service_falemon', 'Falemon', 'Tokyo 02', 'VMess'),
  node('node_falemon_taipei', 'service_falemon', 'Falemon', 'Taipei 01', 'Trojan'),
  node('node_northwind_sg', 'service_northwind', 'Northwind', 'Singapore I0', 'Trojan'),
  node('node_northwind_osaka', 'service_northwind', 'Northwind', 'Osaka 01', 'Shadowsocks'),
]

let customGroups: CustomAppGroup[] = [
  { id: 'appgroup_dev', name: 'Development', apps: ['codex.exe', 'code.exe'] },
  { id: 'appgroup_chat', name: 'AI tools', apps: ['ChatGPT.exe'] },
]

let state: AppState = {
  services,
  nodes,
  matchGroups: [
    {
      id: 'group_openai',
      name: 'OpenAI',
      conditions: [
        { id: 'c1', kind: 'process_name', value: 'ChatGPT.exe' },
        { id: 'c2', kind: 'process_name', value: 'codex.exe' },
        { id: 'c3', kind: 'domain_suffix', value: 'chatgpt.com' },
      ],
    },
    {
      id: 'group_work',
      name: 'Work apps',
      conditions: [{ id: 'c4', kind: 'domain_suffix', value: 'notion.so' }],
    },
  ],
  rulesets: useEmptyRulesetFixture ? [] : [
    {
      id: 'ruleset_ai',
      name: 'AI',
      entries: [
        {
          id: 'entry_openai',
          matchGroupId: 'group_openai',
          destination: { kind: 'node', nodeId: 'node_falemon_sg' },
        },
      ],
      miscellaneous: 'direct',
    },
    {
      id: 'ruleset_work',
      name: 'Work',
      entries: [
        {
          id: 'entry_work',
          matchGroupId: 'group_work',
          destination: { kind: 'node', nodeId: 'node_northwind_osaka' },
        },
      ],
      miscellaneous: 'direct',
    },
  ],
  presets: useEmptyRulesetFixture ? [] : [
    {
      id: 'preset_ai',
      name: 'AI',
      mode: 'rule',
      rulesetId: 'ruleset_ai',
      globalNodeId: null,
    },
  ],
  selection: {
    mode: 'rule',
    rulesetId: useEmptyRulesetFixture ? null : 'ruleset_ai',
    globalNodeId: 'node_falemon_sg',
  },
  customAppGroups: customGroups,
  historyPaused: false,
}

const runtime: Snapshot['runtime'] = {
  vpnActive: false,
  coreStatus: 'off',
  serviceAvailable: true,
  serviceMessage: null,
  lastError: null,
  activeSummary: useEmptyRulesetFixture ? null : 'Rule · AI',
}

function snapshot(): Snapshot {
  state.services = state.services.map((service) => {
    const serviceNodes = state.nodes.filter((item) => item.serviceId === service.id)
    return {
      ...service,
      nodeCount: serviceNodes.filter((item) => !item.missing).length,
      missingCount: serviceNodes.filter((item) => item.missing).length,
    }
  })
  return {
    state: structuredClone(state),
    runtime: { ...runtime },
  }
}

function delay<T>(value: T): Promise<T> {
  return new Promise((resolve) => setTimeout(() => resolve(structuredClone(value)), 120))
}

const previews = new Map<string, ImportPreview>()
let liveTick = 0

const pendingPreview = (id: string, nodes: ImportPreview['nodes'], sourceType: ImportPreview['sourceType'], title: string, host: string | null, allowance: Allowance): ImportPreview => ({
  id,
  sourceType,
  title,
  sourceHost: host,
  nodeCount: nodes.length,
  unsupportedCount: 0,
  noticeCount: 0,
  allowance,
  stale: false,
  nodes,
})

export async function invoke(command: string, args?: Record<string, unknown>): Promise<unknown> {
  switch (command) {
    case 'get_snapshot':
      return delay(snapshot())
    case 'set_vpn': {
      const enabled = Boolean(args?.enabled)
      if (enabled) {
        runtime.vpnActive = true
        runtime.coreStatus = 'on'
        runtime.lastError = null
      } else {
        runtime.vpnActive = false
        runtime.coreStatus = 'off'
      }
      return delay(snapshot())
    }
    case 'set_selection': {
      const selection = args?.selection as AppState['selection']
      if (selection.mode === 'rule' && !state.rulesets.some((item) => item.id === selection.rulesetId)) {
        throw new Error('Select a ruleset before enabling Rule mode.')
      }
      if (selection.mode === 'global') {
        const selected = state.nodes.find((item) => item.id === selection.globalNodeId)
        if (!selected || selected.missing) {
          throw new Error('Select an available node before enabling Global mode.')
        }
      }
      state.selection = selection
      const ruleset = state.rulesets.find((item) => item.id === selection.rulesetId)
      const globalNode = state.nodes.find((item) => item.id === selection.globalNodeId)
      runtime.activeSummary =
        selection.mode === 'global' && globalNode
          ? `Global · ${globalNode.serviceTitle} / ${globalNode.displayName}`
          : ruleset
            ? `Rule · ${ruleset.name}`
            : null
      return delay(snapshot())
    }
    case 'preview_import': {
      const inputValue = String(args?.input ?? '')
      if (!/^https:\/\//i.test(inputValue) && !/^(trojan|ss|vmess|vless):\/\//i.test(inputValue)) {
        throw new Error('Enter an HTTPS subscription URL or a supported share link.')
      }
      const link = /:\/\//i.test(inputValue) && !/^https:\/\//i.test(inputValue)
      const id = `preview_${previews.size + 1}`
      const previewNodes = link
        ? [{ displayName: 'Manual node', protocol: 'Trojan' }]
        : [
            { displayName: 'Singapore I0', protocol: 'Trojan' },
            { displayName: 'Tokyo 02', protocol: 'VMess' },
            { displayName: 'Frankfurt 01', protocol: 'VLESS' },
          ]
      const preview = pendingPreview(
        id,
        previewNodes,
        link ? 'link' : 'subscription',
        link ? 'Manual nodes' : new URL(inputValue).hostname.replace(/^www\./, ''),
        link ? null : new URL(inputValue).hostname,
        {
          kind: 'full',
          total: 50 * 1024 ** 3,
          used: 4 * 1024 ** 3,
          remaining: 46 * 1024 ** 3,
          expiresAt: now() + 30 * 86400,
          source: 'header',
        },
      )
      previews.set(id, preview)
      return delay(preview)
    }
    case 'cancel_import':
      previews.delete(String(args?.previewId))
      return undefined
    case 'confirm_import': {
      const preview = previews.get(String(args?.previewId))
      if (!preview) throw new Error('This import preview expired. Preview the source again.')
      const title = String(args?.title || preview.title)
      const serviceId = `service_${Date.now()}`
      state.services.push({
        id: serviceId,
        title,
        sourceType: preview.sourceType === 'subscription' ? 'subscription' : 'manual',
        sourceHost: preview.sourceHost,
        nodeCount: preview.nodeCount,
        missingCount: 0,
        allowance: preview.allowance,
        stale: false,
        lastRefreshAt: now(),
        lastError: null,
      })
      for (const [index, imported] of preview.nodes.entries()) {
        state.nodes.push(
          node(
            `node_${Date.now()}_${index}`,
            serviceId,
            title,
            imported.displayName,
            imported.protocol,
          ),
        )
      }
      previews.delete(preview.id)
      return delay(snapshot())
    }
    case 'rename_service': {
      const id = String(args?.serviceId)
      const title = String(args?.title ?? '').trim()
      if (!title) throw new Error('The service name cannot be empty.')
      state.services = state.services.map((service) =>
        service.id === id ? { ...service, title } : service,
      )
      state.nodes = state.nodes.map((item) =>
        item.serviceId === id ? { ...item, serviceTitle: title } : item,
      )
      return delay(snapshot())
    }
    case 'refresh_service': {
      const id = String(args?.serviceId)
      const service = state.services.find((item) => item.id === id)
      if (!service) throw new Error('The service no longer exists.')
      service.stale = false
      service.lastRefreshAt = now()
      service.lastError = null
      return delay(snapshot())
    }
    case 'save_match_group': {
      const input = args?.input as {
        id: string | null
        name: string
        conditions: { kind: string; value: string }[]
      }
      if (!input.name.trim()) throw new Error('Match group names cannot be empty.')
      const group = {
        id: input.id || `group_${Date.now()}`,
        name: input.name.trim(),
        conditions: input.conditions.map((condition, index) => ({
          id: `condition_${index}`,
          kind: condition.kind,
          value: condition.value,
        })) as AppState['matchGroups'][number]['conditions'],
      }
      const existing = state.matchGroups.findIndex((item) => item.id === group.id)
      if (existing >= 0) state.matchGroups[existing] = group
      else state.matchGroups.push(group)
      return delay(snapshot())
    }
    case 'delete_match_group': {
      const id = String(args?.groupId)
      if (state.rulesets.some((ruleset) => ruleset.entries.some((entry) => entry.matchGroupId === id))) {
        throw new Error('This match group is used by a ruleset. Remove it from the ruleset first.')
      }
      state.matchGroups = state.matchGroups.filter((group) => group.id !== id)
      return delay(snapshot())
    }
    case 'save_ruleset': {
      const input = args?.input as {
        id: string | null
        name: string
        entries: AppState['rulesets'][number]['entries']
        miscellaneous: 'direct' | 'block'
      }
      const name = input.name.trim()
      if (!name) throw new Error('Ruleset names cannot be empty.')
      if (state.rulesets.some((item) => item.name.toLowerCase() === name.toLowerCase() && item.id !== input.id)) {
        throw new Error('A ruleset with this name already exists.')
      }
      const ruleset = {
        id: input.id || `ruleset_${Date.now()}`,
        name,
        entries: input.entries,
        miscellaneous: input.miscellaneous,
      }
      const index = state.rulesets.findIndex((item) => item.id === ruleset.id)
      if (index >= 0) state.rulesets[index] = ruleset
      else state.rulesets.push(ruleset)
      if (!state.selection.rulesetId) state.selection.rulesetId = ruleset.id
      if (state.selection.rulesetId === ruleset.id) runtime.activeSummary = `Rule · ${ruleset.name}`
      return delay(snapshot())
    }
    case 'delete_ruleset': {
      const id = String(args?.rulesetId)
      if (state.selection.rulesetId === id) {
        throw new Error('The active ruleset cannot be deleted. Select another ruleset first.')
      }
      state.rulesets = state.rulesets.filter((ruleset) => ruleset.id !== id)
      return delay(snapshot())
    }
    case 'save_preset': {
      const name = String(args?.name ?? '').trim()
      if (!name) throw new Error('Saved run configuration names cannot be empty.')
      const preset = {
        id: `preset_${Date.now()}`,
        name,
        ...structuredClone(state.selection),
      }
      state.presets.push(preset)
      return delay(snapshot())
    }
    case 'delete_preset': {
      const id = String(args?.presetId)
      state.presets = state.presets.filter((preset) => preset.id !== id)
      return delay(snapshot())
    }
    case 'apply_preset': {
      const id = String(args?.presetId)
      const preset = state.presets.find((item) => item.id === id)
      if (!preset) throw new Error('The saved run configuration no longer exists.')
      state.selection = {
        mode: preset.mode,
        rulesetId: preset.rulesetId,
        globalNodeId: preset.globalNodeId,
      }
      const ruleset = state.rulesets.find((item) => item.id === preset.rulesetId)
      const globalNode = state.nodes.find((item) => item.id === preset.globalNodeId)
      runtime.activeSummary =
        preset.mode === 'global' && globalNode
          ? `Global · ${globalNode.serviceTitle} / ${globalNode.displayName}`
          : ruleset
            ? `Rule · ${ruleset.name}`
            : null
      return delay(snapshot())
    }
    case 'set_history_paused':
      state.historyPaused = Boolean(args?.paused)
      return delay(snapshot())
    case 'clear_history':
      if (!args?.confirm) throw new Error('Confirmation is required to clear traffic history.')
      return delay(snapshot())
    case 'get_history':
      return delay(history(args?.input as HistoryInput, state.customAppGroups))
    case 'get_live':
      return live()
    case 'get_diagnostics':
      return delay(diagnostics())
    case 'save_custom_group': {
      const input = args?.group as CustomAppGroup
      const clean = {
        id: input.id || `appgroup_${Date.now()}`,
        name: input.name.trim(),
        apps: input.apps.map((app) => app.trim()).filter(Boolean),
      }
      const index = state.customAppGroups.findIndex((group) => group.id === clean.id)
      if (index >= 0) state.customAppGroups[index] = clean
      else state.customAppGroups.push(clean)
      return delay(snapshot())
    }
    case 'delete_custom_group': {
      const id = String(args?.groupId)
      state.customAppGroups = state.customAppGroups.filter((group) => group.id !== id)
      return delay(snapshot())
    }
    default:
      throw new Error(`The UI mock does not implement ${command}.`)
  }
}

function history(input: HistoryInput, groups: CustomAppGroup[]): HistoryReport {
  const timestamp = now()
  const currentDate = new Date(timestamp * 1000)
  const today = new Date(currentDate)
  today.setHours(0, 0, 0, 0)
  const dayCount = input.range === '30d' ? 30 : input.range === '7d' ? 7 : 1
  const firstDay = new Date(today)
  firstDay.setDate(firstDay.getDate() - (dayCount - 1))
  const periodCount = input.range === 'today' ? currentDate.getHours() + 1 : dayCount
  const scale = input.range === '30d' ? 0.42 : input.range === 'today' ? 0.08 : 1
  const baseBars = Array.from({ length: periodCount }, (_, index) => {
    const start = input.range === 'today'
      ? Math.floor((today.getTime() / 1000 + index * 3600))
      : Math.floor(firstDay.getTime() / 1000) + index * 86400
    const date = new Date(start * 1000)
    const wave = 1.4 + Math.sin(index * 1.37) * 0.7 + (index % 5) * 0.2
    return {
      start,
      label: input.range === 'today'
        ? date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
        : date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' }),
      download: Math.round(wave * 1.1 * 1024 ** 3 * scale),
      upload: Math.round(wave * 0.22 * 1024 ** 3 * scale),
      gap: input.range === '30d' && index === 17,
    }
  })
  const shares = [
    { app: 'ChatGPT.exe', route: 'Falemon / Singapore I0', routeClass: 'proxy' as const, download: 0.20, upload: 0.18 },
    { app: 'ChatGPT.exe', route: 'Direct', routeClass: 'direct' as const, download: 0.10, upload: 0.08 },
    { app: 'ChatGPT.exe', route: 'Blocked', routeClass: 'blocked' as const, download: 0.02, upload: 0.01 },
    { app: 'ChatGPT.exe', route: 'Unknown chain', routeClass: 'unknown' as const, download: 0.01, upload: 0.01 },
    { app: 'codex.exe', route: 'Falemon / Singapore I0', routeClass: 'proxy' as const, download: 0.19, upload: 0.21 },
    { app: 'steam.exe', route: 'Direct', routeClass: 'direct' as const, download: 0.17, upload: 0.22 },
    { app: 'firefox.exe', route: 'Unknown chain', routeClass: 'unknown' as const, download: 0.03, upload: 0.03 },
  ]
  const availableApps = [...new Map(shares.map((sample) => [sample.app.toLowerCase(), sample.app])).values()]
    .sort((left, right) => left.localeCompare(right, undefined, { sensitivity: 'base' }))
  const routeFilter = input.routeFilter ?? null
  const subjectFilter = input.subjectFilter ?? null
  const selectedGroup = subjectFilter?.kind === 'group'
    ? groups.find((group) => group.id === subjectFilter.id)
    : undefined
  const isSelected = (sample: typeof shares[number]) => {
    if (routeFilter && sample.routeClass !== routeFilter) return false
    if (subjectFilter?.kind === 'app' && sample.app.toLowerCase() !== subjectFilter.name.toLowerCase()) return false
    if (subjectFilter?.kind === 'group' && !selectedGroup?.apps.some((app) => app.toLowerCase() === sample.app.toLowerCase())) return false
    return true
  }
  const filtered = routeFilter != null || subjectFilter != null
  const rowsByKey = new Map<string, HistoryReport['rows'][number]>()
  const bars = baseBars.map((bar) => {
    const samples = shares.filter(isSelected).map((sample) => ({
      ...sample,
      download: Math.floor(bar.download * sample.download),
      upload: Math.floor(bar.upload * sample.upload),
    }))
    const estimatedDownload = samples.reduce((sum, sample) => sum + sample.download, 0)
    const estimatedUpload = samples.reduce((sum, sample) => sum + sample.upload, 0)
    let periodDownload = 0
    let periodUpload = 0
    for (const sample of samples) {
      const download = estimatedDownload > bar.download
        ? Math.floor(sample.download * bar.download / estimatedDownload)
        : sample.download
      const upload = estimatedUpload > bar.upload
        ? Math.floor(sample.upload * bar.upload / estimatedUpload)
        : sample.upload
      periodDownload += download
      periodUpload += upload
      const group = subjectFilter?.kind === 'group' && selectedGroup?.apps.some((app) => app.toLowerCase() === sample.app.toLowerCase())
        ? selectedGroup
        : groups.find((candidate) => candidate.apps.some((app) => app.toLowerCase() === sample.app.toLowerCase()))
      const label = input.breakdown === 'route' ? sample.route : input.breakdown === 'customGroup' ? group?.name ?? sample.app : sample.app
      const app = input.breakdown === 'app' || (input.breakdown === 'customGroup' && !group) ? sample.app : null
      const route = input.breakdown === 'route' ? sample.route : null
      const key = `${label}\u0000${sample.routeClass}`
      const existing = rowsByKey.get(key) ?? { label, app, route, routeClass: sample.routeClass, download: 0, upload: 0, total: 0 }
      existing.download += download
      existing.upload += upload
      existing.total = existing.download + existing.upload
      rowsByKey.set(key, existing)
    }
    if (!filtered) {
      const residualDownload = Math.max(0, bar.download - estimatedDownload)
      const residualUpload = Math.max(0, bar.upload - estimatedUpload)
      if (residualDownload > 0 || residualUpload > 0) {
        const key = input.breakdown === 'route' ? 'Unattributed\u0000unknown' : 'Unattributed\u0000unknown'
        const existing = rowsByKey.get(key) ?? {
          label: 'Unattributed',
          app: null,
          route: input.breakdown === 'route' ? 'Unattributed' : null,
          routeClass: 'unknown' as const,
          download: 0,
          upload: 0,
          total: 0,
        }
        existing.download += residualDownload
        existing.upload += residualUpload
        existing.total = existing.download + existing.upload
        rowsByKey.set(key, existing)
      }
    }
    return { ...bar, download: filtered ? periodDownload : bar.download, upload: filtered ? periodUpload : bar.upload }
  })
  const rows = [...rowsByKey.values()].sort((left, right) => right.total - left.total || left.label.localeCompare(right.label))
  const totalDownload = bars.reduce((sum, bar) => sum + bar.download, 0)
  const totalUpload = bars.reduce((sum, bar) => sum + bar.upload, 0)
  return {
    from: Math.floor(firstDay.getTime() / 1000),
    to: timestamp,
    totalDownload,
    totalUpload,
    total: totalDownload + totalUpload,
    gapMinutes: 4,
    bars,
    rows,
    breakdown: input.breakdown,
    availableApps,
  }
}

function live(): LiveSnapshot {
  liveTick += 1
  if (!runtime.vpnActive) {
    return {
      active: false,
      downloadRate: 0,
      uploadRate: 0,
      downloadTotal: 0,
      uploadTotal: 0,
      connections: [],
      sampledAt: now(),
    }
  }
  const connections = [
    {
      app: 'ChatGPT.exe',
      appKey: 'mock-1',
      destination: 'chatgpt.com',
      network: 'tcp',
      route: 'Falemon / Singapore I0',
      matchedGroup: 'OpenAI',
      downloadRate: 1_580_000 + (liveTick % 7) * 32_000,
      uploadRate: 92_000,
      downloadTotal: 18_432_000,
      uploadTotal: 1_024_000,
      startedAt: now() - 320,
    },
    {
      app: 'codex.exe',
      appKey: 'mock-2',
      destination: 'api.openai.com',
      network: 'tcp',
      route: 'Falemon / Singapore I0',
      matchedGroup: 'OpenAI',
      downloadRate: 420_000,
      uploadRate: 210_000,
      downloadTotal: 6_430_000,
      uploadTotal: 4_820_000,
      startedAt: now() - 210,
    },
    {
      app: 'steam.exe',
      appKey: 'mock-3',
      destination: 'steamcontent.com',
      network: 'tcp',
      route: 'Direct',
      matchedGroup: null,
      downloadRate: 2_780_000,
      uploadRate: 120_000,
      downloadTotal: 54_820_000,
      uploadTotal: 1_820_000,
      startedAt: now() - 90,
    },
    {
      app: 'firefox.exe',
      appKey: 'mock-4',
      destination: 'example.org',
      network: 'tcp',
      route: 'Direct',
      matchedGroup: null,
      downloadRate: 38_000,
      uploadRate: 11_000,
      downloadTotal: 620_000,
      uploadTotal: 240_000,
      startedAt: now() - 42,
    },
  ]
  return {
    active: true,
    downloadRate: connections.reduce((sum, item) => sum + item.downloadRate, 0),
    uploadRate: connections.reduce((sum, item) => sum + item.uploadRate, 0),
    downloadTotal: 80_302_000,
    uploadTotal: 7_904_000,
    connections,
    sampledAt: now(),
  }
}

function diagnostics() {
  const at = now()
  // The API is newest first; ids 4 and 5 also exercise the equal-second tie break.
  return [
    { id: 5, at: at - 1, source: 'app', level: 'warn', message: 'Northwind refresh failed. Last known-good nodes retained; allowance marked stale.' },
    { id: 4, at: at - 1, source: 'service', level: 'info', message: 'Core running mode: Service; process discovery always enabled.' },
    { id: 3, at: at - 2, source: 'app', level: 'info', message: 'Subscription nodes refreshed.' },
    { id: 2, at: at - 3, source: 'core', level: 'info', message: 'TUN: using adapter ClashforkTUN with mixed stack.' },
    { id: 1, at: at - 4, source: 'app', level: 'info', message: 'Routing switched and existing flows were closed.' },
  ]
}
