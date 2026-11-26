import type { LiveRates } from '../types'

export const RATE_WINDOW_SECONDS = 60
const MAX_POINTS = 120

export interface RatePoint {
  at: number
  download: number
  upload: number
}

export interface RateRecorderState {
  latest: LiveRates
  points: RatePoint[]
}

export const emptyRates: LiveRates = {
  active: false,
  available: false,
  proxyDownloadRate: 0,
  proxyUploadRate: 0,
  sampledAt: 0,
}

export const initialRateRecorderState: RateRecorderState = {
  latest: emptyRates,
  points: [],
}

export function recordRateSample(state: RateRecorderState, sample: LiveRates): RateRecorderState {
  if (sample.sampledAt && state.points.some((point) => point.at === sample.sampledAt)) {
    return { latest: sample, points: state.points }
  }
  const points = sample.sampledAt
    ? state.points.filter((point) => point.at <= sample.sampledAt && sample.sampledAt - point.at <= RATE_WINDOW_SECONDS)
    : state.points
  if (!sample.active || !sample.available || !sample.sampledAt) {
    return { latest: sample, points }
  }
  return {
    latest: sample,
    points: [
      ...points,
      { at: sample.sampledAt, download: sample.proxyDownloadRate, upload: sample.proxyUploadRate },
    ].slice(-MAX_POINTS),
  }
}

export function markRateRecorderUnavailable(state: RateRecorderState): RateRecorderState {
  return {
    latest: { ...state.latest, active: false, available: false },
    points: state.points,
  }
}
