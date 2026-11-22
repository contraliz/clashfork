import { useEffect, useState } from 'react'
import { api, isTauri } from './api'
import type { LiveSnapshot } from './types'

const emptyLive: LiveSnapshot = {
  active: false,
  downloadRate: 0,
  uploadRate: 0,
  downloadTotal: 0,
  uploadTotal: 0,
  connections: [],
  sampledAt: 0,
}

export function useLive(enabled = true): LiveSnapshot {
  const [live, setLive] = useState<LiveSnapshot>(emptyLive)
  useEffect(() => {
    if (!enabled) return
    let cancelled = false
    let timer: number | undefined
    const poll = async () => {
      try {
        const next = await api.getLive()
        if (!cancelled) setLive(next)
      } catch {
        if (!cancelled) setLive((current) => ({ ...current, active: false }))
      }
      if (!cancelled) timer = window.setTimeout(poll, 1000)
    }
    void poll()
    return () => {
      cancelled = true
      if (timer) window.clearTimeout(timer)
    }
  }, [enabled])
  return live
}

export function useRatePoints(live: LiveSnapshot, limit = 60): Array<{ at: number; download: number; upload: number }> {
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
