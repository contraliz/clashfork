import { describe, expect, it } from 'vitest'
import {
  monotonePath,
  nextStableRateScaleIndex,
  rateChartGeometry,
  rateScaleFromIndex,
  rateScaleIndex,
  RATE_GAP_SECONDS,
  RATE_SCALE_MIN,
} from './rateChart'
import type { RatePoint } from './rateRecorderCore'

function point(at: number, download: number, upload = 1_000): RatePoint {
  return { at, download, upload }
}

describe('smooth rate chart', () => {
  it('keeps a minimum scale so small rates stay near the bottom', () => {
    const now = 2_000
    const geometry = rateChartGeometry([point(now - 1, 10 * 1024)], now, RATE_SCALE_MIN)
    expect(geometry.labels[0]).toBe('1 MB/s')
    expect(geometry.downloadAnchors[0].y).toBeGreaterThan(145)
  })

  it('grows the rounded scale with headroom for real peaks', () => {
    const peak = 50 * 1024 * 1024
    const index = rateScaleIndex(peak)
    const scale = rateScaleFromIndex(index)
    expect(scale).toBeGreaterThanOrEqual(peak * 1.15)
    expect(scale).toBe(64 * 1024 * 1024)
    const now = 2_000
    const geometry = rateChartGeometry([point(now - 1, peak)], now, scale)
    expect(geometry.labels[0]).toBe('64 MB/s')
    expect(geometry.downloadAnchors[0].y).toBeLessThan(50)
  })

  it('uses smooth monotone segments without linear bridging', () => {
    const now = 2_000
    const points = [4, 3, 2, 1].map((age) => point(now - age, (5 - age) * 100_000))
    const geometry = rateChartGeometry(points, now, 1024 * 1024)
    expect(geometry.downloadPath).toContain('C ')
    expect(geometry.downloadPath).not.toContain(' L ')
    expect(geometry.groups).toBe(1)
    expect(geometry.firstX).toBeLessThan(geometry.lastX)
    expect(geometry.lastX).toBeGreaterThan(800)
    expect(geometry.area).toContain('C ')
  })

  it('breaks the curve across missing sample periods', () => {
    const now = 2_000
    const gap = RATE_GAP_SECONDS + 1
    const points = [point(now - gap - 2, 200_000), point(now - gap - 1, 300_000), point(now - 1, 400_000), point(now, 500_000)]
    const geometry = rateChartGeometry(points, now, 1024 * 1024)
    expect(geometry.groups).toBe(2)
    expect(geometry.downloadPath.match(/M /g)).toHaveLength(2)
  })

  it('keeps measured anchors on the smooth path and inside the chart', () => {
    const now = 2_000
    const points = [point(now - 3, 100_000), point(now - 2, 900_000), point(now - 1, 300_000)]
    const geometry = rateChartGeometry(points, now, 1024 * 1024)
    const path = monotonePath(geometry.downloadAnchors)
    for (const anchor of geometry.downloadAnchors) {
      expect(anchor.y).toBeGreaterThanOrEqual(10)
      expect(anchor.y).toBeLessThanOrEqual(150)
      expect(path).toContain(`${anchor.x.toFixed(1)} ${anchor.y.toFixed(1)}`)
    }
  })

  it('applies hysteresis before lowering the scale', () => {
    expect(nextStableRateScaleIndex(2, 5, 0)).toBe(5)
    expect(nextStableRateScaleIndex(5, 2, 0)).toBe(5)
    expect(nextStableRateScaleIndex(5, 2, 2)).toBe(4)
    expect(nextStableRateScaleIndex(1, 0, 1.9)).toBe(1)
    expect(nextStableRateScaleIndex(1, 0, 2)).toBe(0)
  })

  it('handles disconnected or empty samples without inventing a curve', () => {
    const geometry = rateChartGeometry([], 2_000, RATE_SCALE_MIN)
    expect(geometry.groups).toBe(0)
    expect(geometry.points).toHaveLength(0)
    expect(geometry.downloadPath).toBe('M 820 150')
    expect(geometry.area).toBe('')
  })
})
