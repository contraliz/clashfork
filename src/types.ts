export type Mode = 'rule' | 'global'
export type CoreStatus = 'off' | 'starting' | 'on' | 'error'
export type ConditionKind =
  | 'process_name'
  | 'process_path'
  | 'domain'
  | 'domain_suffix'
  | 'ipv4_cidr'
  | 'ipv6_cidr'

export type Destination =
  | { kind: 'direct' }
  | { kind: 'block' }
  | { kind: 'node'; nodeId: string }

export type Allowance =
  | {
      kind: 'full'
      total: number
      used: number
      remaining: number
      expiresAt: number | null
      source: 'header'
    }
  | { kind: 'remaining'; remaining: number; source: 'notice' }
  | { kind: 'unavailable' }

export interface ServiceView {
  id: string
  title: string
  sourceType: 'subscription' | 'link' | 'manual'
  sourceHost: string | null
  nodeCount: number
  missingCount: number
  allowance: Allowance
  stale: boolean
  lastRefreshAt: number | null
  lastError: string | null
}

export type SourceType = 'subscription' | 'link' | 'manual'

export interface NodeView {
  id: string
  serviceId: string
  serviceTitle: string
  displayName: string
  protocol: string
  missing: boolean
  coreName: string
}

export interface MatchCondition {
  id: string
  kind: ConditionKind
  value: string
}

export interface MatchGroup {
  id: string
  name: string
  conditions: MatchCondition[]
}

export interface RulesetEntry {
  id: string
  matchGroupId: string
  destination: Destination
}

export interface Ruleset {
  id: string
  name: string
  entries: RulesetEntry[]
  miscellaneous: 'direct' | 'block'
}

export interface Selection {
  mode: Mode
  rulesetId: string | null
  globalNodeId: string | null
}

export interface Preset {
  id: string
  name: string
  mode: Mode
  rulesetId: string | null
  globalNodeId: string | null
}

export interface CustomAppGroup {
  id: string
  name: string
  apps: string[]
}

export interface RuntimeView {
  vpnActive: boolean
  coreStatus: CoreStatus
  serviceAvailable: boolean
  serviceMessage: string | null
  lastError: string | null
  activeSummary: string | null
}

export interface AppState {
  services: ServiceView[]
  nodes: NodeView[]
  matchGroups: MatchGroup[]
  rulesets: Ruleset[]
  presets: Preset[]
  selection: Selection
  customAppGroups: CustomAppGroup[]
  historyPaused: boolean
}

export interface Snapshot {
  state: AppState
  runtime: RuntimeView
}

export interface PreviewNode {
  displayName: string
  protocol: string
}

export interface ImportPreview {
  id: string
  sourceType: 'subscription' | 'link' | 'manual'
  title: string
  sourceHost: string | null
  nodeCount: number
  unsupportedCount: number
  noticeCount: number
  allowance: Allowance
  stale: boolean
  nodes: PreviewNode[]
}

export interface LiveRoute {
  app: string
  appKey: string
  destination: string
  network: string
  route: string
  matchedGroup: string | null
  downloadRate: number
  uploadRate: number
  downloadTotal: number
  uploadTotal: number
  startedAt: number
}

export interface LiveSnapshot {
  active: boolean
  downloadRate: number
  uploadRate: number
  downloadTotal: number
  uploadTotal: number
  connections: LiveRoute[]
  sampledAt: number
}

export interface HistoryBar {
  start: number
  label: string
  download: number
  upload: number
  gap: boolean
}

export interface HistoryRow {
  label: string
  app: string | null
  route: string | null
  routeClass: 'proxy' | 'direct' | 'blocked' | 'unknown'
  download: number
  upload: number
  total: number
}

export interface HistoryReport {
  from: number
  to: number
  totalDownload: number
  totalUpload: number
  total: number
  gapMinutes: number
  bars: HistoryBar[]
  rows: HistoryRow[]
  breakdown: 'app' | 'customGroup' | 'route'
}

export interface DiagnosticEntry {
  id: number
  at: number
  source: 'app' | 'core' | 'service'
  level: 'debug' | 'info' | 'warn' | 'error'
  message: string
}

export interface AddNodesInput {
  value: string
  displayName?: string
}

export interface SaveRulesetInput {
  id: string | null
  name: string
  entries: RulesetEntry[]
  miscellaneous: 'direct' | 'block'
}

export interface SaveMatchGroupInput {
  id: string | null
  name: string
  conditions: Array<{ kind: ConditionKind; value: string }>
}

export interface HistoryInput {
  range: 'today' | '7d' | '30d'
  breakdown: 'app' | 'customGroup' | 'route'
}
