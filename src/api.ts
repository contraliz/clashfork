import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type {
  AppState,
  CustomAppGroup,
  HistoryInput,
  HistoryReport,
  ImportPreview,
  LiveSnapshot,
  SaveMatchGroupInput,
  SaveRulesetInput,
  Selection,
  Snapshot,
} from './types'

export const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
const useMock = !isTauri && new URLSearchParams(window.location.search).get('mock') === '1'

export const emptyState = (): AppState => ({
  services: [],
  nodes: [],
  matchGroups: [],
  rulesets: [],
  presets: [],
  selection: { mode: 'rule', rulesetId: null, globalNodeId: null },
  customAppGroups: [],
  historyPaused: false,
})

export const emptySnapshot = (): Snapshot => ({
  state: emptyState(),
  runtime: {
    vpnActive: false,
    coreStatus: 'off',
    serviceAvailable: false,
    serviceMessage:
      'The Clashfork service is not installed. Reinstall Clashfork to enable the TUN VPN.',
    lastError: null,
    activeSummary: null,
  },
})

async function mockInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!import.meta.env.DEV) {
    throw new Error('The UI mock is only available while developing.')
  }
  const mock = await import('./mock')
  return mock.invoke(command, args) as Promise<T>
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (useMock) {
    return mockInvoke<T>(command, args)
  }
  if (!isTauri) {
    throw new Error('This screen is available in the Clashfork desktop app.')
  }
  try {
    return await invoke<T>(command, args)
  } catch (error) {
    if (typeof error === 'string') throw new Error(error)
    if (error instanceof Error) throw error
    throw new Error('The Clashfork backend rejected the request.')
  }
}

export const api = {
  getSnapshot: () => call<Snapshot>('get_snapshot'),
  setVpn: (enabled: boolean) => call<Snapshot>('set_vpn', { enabled }),
  setSelection: (selection: Selection) => call<Snapshot>('set_selection', { selection }),
  previewImport: (input: string) => call<ImportPreview>('preview_import', { input }),
  cancelImport: (previewId: string) => call<void>('cancel_import', { previewId }),
  confirmImport: (previewId: string, title?: string) =>
    call<Snapshot>('confirm_import', { previewId, title: title || null }),
  refreshService: (serviceId: string) =>
    call<Snapshot>('refresh_service', { serviceId }),
  renameService: (serviceId: string, title: string) =>
    call<Snapshot>('rename_service', { serviceId, title }),
  saveMatchGroup: (input: SaveMatchGroupInput) =>
    call<Snapshot>('save_match_group', { input }),
  deleteMatchGroup: (groupId: string) =>
    call<Snapshot>('delete_match_group', { groupId }),
  saveRuleset: (input: SaveRulesetInput) => call<Snapshot>('save_ruleset', { input }),
  deleteRuleset: (rulesetId: string) =>
    call<Snapshot>('delete_ruleset', { rulesetId }),
  savePreset: (name: string) => call<Snapshot>('save_preset', { name }),
  deletePreset: (presetId: string) => call<Snapshot>('delete_preset', { presetId }),
  applyPreset: (presetId: string) => call<Snapshot>('apply_preset', { presetId }),
  setHistoryPaused: (paused: boolean) =>
    call<Snapshot>('set_history_paused', { paused }),
  clearHistory: (confirm: boolean) =>
    call<Snapshot>('clear_history', { confirm }),
  getHistory: (input: HistoryInput) => call<HistoryReport>('get_history', { input }),
  getLive: () => call<LiveSnapshot>('get_live'),
  getDiagnostics: () => call<import('./types').DiagnosticEntry[]>('get_diagnostics'),
  saveCustomGroup: (group: CustomAppGroup) =>
    call<Snapshot>('save_custom_group', { group }),
  deleteCustomGroup: (groupId: string) =>
    call<Snapshot>('delete_custom_group', { groupId }),
}

export async function subscribeSnapshots(
  handler: (snapshot: Snapshot) => void,
): Promise<UnlistenFn | null> {
  if (!isTauri) return null
  return listen<Snapshot>('snapshot-changed', (event) => handler(event.payload))
}

export async function subscribeErrors(
  handler: (message: string) => void,
): Promise<UnlistenFn | null> {
  if (!isTauri) return null
  return listen<string>('backend-error', (event) => handler(event.payload))
}
