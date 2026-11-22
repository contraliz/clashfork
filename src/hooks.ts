import { useEffect, useState, useSyncExternalStore } from 'react'
import { api, isTauri } from './api'
import type { LiveRates, LiveSnapshot } from './types'

const emptyLive: LiveSnapshot = {
  active: false,
  downloadRate: 0,
  uploadRate: 0,
  downloadTotal: 0,
  uploadTotal: 0,
  connections: [],
  sampledAt: 0,
}

const emptyRates: LiveRates = {
  active: false,
  downloadRate: 0,
  uploadRate: 0,
  downloadTotal: 0,
  uploadTotal: 0,
  sampledAt: 0,
}

function subscribeDocumentVisibility(onChange: () => void) {
  if (typeof document === 'undefined') return () => {}
  document.addEventListener('visibilitychange', onChange)
  return () => document.removeEventListener('visibilitychange', onChange)
}

export function isDocumentVisible() {
  return typeof document === 'undefined' || document.visibilityState !== 'hidden'
}

export function useDocumentVisible(): boolean {
  return useSyncExternalStore(subscribeDocumentVisibility, isDocumentVisible, () => true)
}

function useVisiblePoll<T extends { active: boolean }>(
  enabled: boolean,
  load: () => Promise<T>,
  initial: T,
): T {
  const [value, setValue] = useState<T>(initial)
  const visible = useDocumentVisible()
  useEffect(() => {
    if (!enabled || !visible || !isDocumentVisible()) return
    let cancelled = false
    let timer: number | undefined
    const poll = async () => {
      try {
        const next = await load()
        if (!cancelled && isDocumentVisible()) setValue(next)
      } catch {
        if (!cancelled && isDocumentVisible()) setValue((current) => ({ ...current, active: false }))
      }
      if (!cancelled && isDocumentVisible()) timer = window.setTimeout(poll, 1000)
    }
    void poll()
    return () => {
      cancelled = true
      if (timer) window.clearTimeout(timer)
    }
  }, [enabled, load, visible])
  return value
}

export function useLive(enabled = true): LiveSnapshot {
  return useVisiblePoll(enabled, api.getLive, emptyLive)
}

export function useLiveRates(enabled = true): LiveRates {
  return useVisiblePoll(enabled, api.getLiveRates, emptyRates)
}

export function useRatePoints(
  live: Pick<LiveSnapshot, 'active' | 'downloadRate' | 'uploadRate' | 'sampledAt'>,
  limit = 60,
): Array<{ at: number; download: number; upload: number }> {
  const [points, setPoints] = useState<Array<{ at: number; download: number; upload: number }>>([])
  useEffect(() => {
    if (!live.active) {
      setPoints([])
      return
    }
    if (!live.sampledAt) return
    setPoints((current) => {
      if (current.some((point) => point.at === live.sampledAt)) return current
      return [
        ...current,
        { at: live.sampledAt, download: live.downloadRate, upload: live.uploadRate },
      ].slice(-limit)
    })
  }, [limit, live.active, live.downloadRate, live.sampledAt, live.uploadRate])
  return points
}

export function useNow(intervalMs = 1000): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000))
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Math.floor(Date.now() / 1000)), intervalMs)
    return () => window.clearInterval(timer)
  }, [intervalMs])
  return now
}

export function getWindowControls() {
  if (!isTauri) {
    return {
      minimize: () => {},
      maximize: () => {},
      close: () => {},
    }
  }
  return {
    minimize: async () => {
      const { getCurrentWindow } = await import('@tauri-apps/api/window')
      await getCurrentWindow().minimize()
    },
    maximize: async () => {
      const { getCurrentWindow } = await import('@tauri-apps/api/window')
      await getCurrentWindow().toggleMaximize()
    },
    close: async () => {
      const { getCurrentWindow } = await import('@tauri-apps/api/window')
      await getCurrentWindow().close()
    },
  }
}
