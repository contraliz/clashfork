import { describe, expect, it } from 'vitest'
import {
  initialRateRecorderState,
  markRateRecorderUnavailable,
  RATE_WINDOW_SECONDS,
  recordRateSample,
  type RateRecorderState,
} from './rateRecorderCore'
import type { LiveRates } from '../types'

function sample(at: number, download: number, upload = 0, overrides: Partial<LiveRates> = {}): LiveRates {
  return {
    active: true,
    available: true,
    proxyDownloadRate: download,
    proxyUploadRate: upload,
    sampledAt: at,
    ...overrides,
  }
}

describe('rate recorder', () => {
  it('keeps a bounded rolling window of measured proxy points', () => {
    let state: RateRecorderState = initialRateRecorderState
    for (let at = 1_000; at <= 1_130; at += 10) state = recordRateSample(state, sample(at, at))

    expect(state.points[0].at).toBe(1_130 - RATE_WINDOW_SECONDS)
    expect(state.points.at(-1)?.at).toBe(1_130)
    expect(state.points).toHaveLength(RATE_WINDOW_SECONDS / 10 + 1)
  })

  it('retains points through unavailable samples and clears only current rates', () => {
    let state = recordRateSample(initialRateRecorderState, sample(1_000, 100, 10))
    state = recordRateSample(state, sample(1_010, 200, 20))
    state = recordRateSample(state, sample(1_020, 0, 0, { available: false }))
    expect(state.points).toHaveLength(2)
    expect(state.latest.available).toBe(false)

    state = markRateRecorderUnavailable(state)
    expect(state.points).toHaveLength(2)
    expect(state.latest.available).toBe(false)
    expect(state.latest.proxyDownloadRate).toBe(0)
  })

  it('records again after a reconnect while recent points remain in range', () => {
    let state = recordRateSample(initialRateRecorderState, sample(1_000, 100))
    state = recordRateSample(state, sample(1_030, 0, 0, { active: false, available: false }))
    state = recordRateSample(state, sample(1_040, 300))
    expect(state.points.map((point) => point.at)).toEqual([1_000, 1_040])
  })

  it('drops points outside the rolling window and ignores duplicate samples', () => {
    let state = recordRateSample(initialRateRecorderState, sample(1_000, 100))
    state = recordRateSample(state, sample(1_010, 110))
    state = recordRateSample(state, sample(1_030, 130))
    state = recordRateSample(state, sample(1_010, 999))
    expect(state.points).toHaveLength(3)
    expect(state.points[1].download).toBe(110)

    state = recordRateSample(state, sample(1_080, 300))
    expect(state.points.map((point) => point.at)).toEqual([1_030, 1_080])
  })
})
