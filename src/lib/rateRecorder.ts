import { useEffect, useSyncExternalStore } from 'react'
import { api } from '../api'
import { isDocumentVisible, useDocumentVisible } from '../hooks'
import {
  initialRateRecorderState,
  markRateRecorderUnavailable,
  recordRateSample,
  type RateRecorderState,
} from './rateRecorderCore'

export {
  RATE_WINDOW_SECONDS,
  emptyRates,
  initialRateRecorderState,
  markRateRecorderUnavailable,
  recordRateSample,
  type RatePoint,
  type RateRecorderState,
} from './rateRecorderCore'

let recorderState = initialRateRecorderState
const listeners = new Set<() => void>()

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

function getSnapshot() {
  return recorderState
}

function updateRecorder(next: RateRecorderState) {
  recorderState = next
  for (const listener of listeners) listener()
}

export function useRateRecorderSnapshot(): RateRecorderState {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}

export function useRateRecorder(enabled: boolean) {
  const visible = useDocumentVisible()
  useEffect(() => {
    if (!enabled || !visible || !isDocumentVisible()) return
    let cancelled = false
    let timer: number | undefined
    const poll = async () => {
      try {
        const next = await api.getLiveRates()
        if (!cancelled && isDocumentVisible()) {
          updateRecorder(recordRateSample(recorderState, next))
        }
      } catch {
        if (!cancelled && isDocumentVisible()) {
          updateRecorder(markRateRecorderUnavailable(recorderState))
        }
      }
      if (!cancelled && isDocumentVisible()) {
        timer = window.setTimeout(() => void poll(), 1000)
      }
    }
    void poll()
    return () => {
      cancelled = true
      if (timer) window.clearTimeout(timer)
    }
  }, [enabled, visible])
}
