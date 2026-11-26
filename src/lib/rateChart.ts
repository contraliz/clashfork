import { formatRateTick } from './format'
import { RATE_WINDOW_SECONDS, type RatePoint } from './rateRecorderCore'

export const RATE_GAP_SECONDS = 2.5
export const RATE_SCALE_MIN = 1024 * 1024
const MAX_SCALE_INDEX = 60
const CHART_WIDTH = 820
const CHART_HEIGHT = 150

export interface ChartAnchor {
  at: number
  x: number
  y: number
}

function scaleValue(index: number): number {
  return RATE_SCALE_MIN * 2 ** index
}

export function rateScaleIndex(rawMax: number): number {
  const target = Math.max(RATE_SCALE_MIN, rawMax * 1.15)
  let index = 0
  while (index < MAX_SCALE_INDEX && scaleValue(index) < target) index += 1
  return index
}

export function rateScaleFromIndex(index: number): number {
  return scaleValue(Math.max(0, Math.min(MAX_SCALE_INDEX, Math.round(index))))
}

export function nextStableRateScaleIndex(
  currentIndex: number,
  desiredIndex: number,
  secondsSinceChange: number,
): number {
  if (desiredIndex > currentIndex) return desiredIndex
  if (desiredIndex < currentIndex && secondsSinceChange >= 2) return Math.max(0, currentIndex - 1)
  return currentIndex
}

function project(
  points: RatePoint[],
  key: 'download' | 'upload',
  nowSeconds: number,
  scale: number,
): ChartAnchor[] {
  return points.map((point) => {
    const age = Math.max(0, Math.min(RATE_WINDOW_SECONDS, nowSeconds - point.at))
    return {
      at: point.at,
      x: 4 + (CHART_WIDTH - 8) * (1 - age / RATE_WINDOW_SECONDS),
      y: CHART_HEIGHT - Math.min(1, point[key] / scale) * (CHART_HEIGHT - 10),
    }
  })
}

function splitGroups(anchors: ChartAnchor[]): ChartAnchor[][] {
  const groups: ChartAnchor[][] = []
  for (const anchor of anchors) {
    const current = groups.at(-1)
    if (!current || anchor.at - current.at(-1)!.at > RATE_GAP_SECONDS) groups.push([anchor])
    else current.push(anchor)
  }
  return groups
}

export function monotonePath(values: ChartAnchor[]): string {
  if (values.length === 0) return ''
  if (values.length === 1) return `M ${values[0].x.toFixed(1)} ${values[0].y.toFixed(1)}`
  const secants = values.slice(0, -1).map((point, index) => {
    const next = values[index + 1]
    return (next.y - point.y) / (next.x - point.x)
  })
  const tangents = new Array<number>(values.length)
  tangents[0] = secants[0]
  tangents[values.length - 1] = secants[secants.length - 1]
  for (let index = 1; index < values.length - 1; index += 1) {
    const before = secants[index - 1]
    const after = secants[index]
    if (before === 0 || after === 0 || Math.sign(before) !== Math.sign(after)) {
      tangents[index] = 0
    } else {
      const average = (before + after) / 2
      const limit = 3 * Math.min(Math.abs(before), Math.abs(after))
      tangents[index] = Math.sign(average) * Math.min(Math.abs(average), limit)
    }
  }
  let path = `M ${values[0].x.toFixed(1)} ${values[0].y.toFixed(1)}`
  for (let index = 0; index < values.length - 1; index += 1) {
    const point = values[index]
    const next = values[index + 1]
    const dx = next.x - point.x
    const c1x = point.x + dx / 3
    const c1y = point.y + tangents[index] * dx / 3
    const c2x = next.x - dx / 3
    const c2y = next.y - tangents[index + 1] * dx / 3
    path += ` C ${c1x.toFixed(1)} ${c1y.toFixed(1)} ${c2x.toFixed(1)} ${c2y.toFixed(1)} ${next.x.toFixed(1)} ${next.y.toFixed(1)}`
  }
  return path
}

export function rateChartGeometry(
  allPoints: RatePoint[],
  nowSeconds: number,
  scale: number,
) {
  const points = allPoints.filter((point) => point.at <= nowSeconds + 1 && nowSeconds - point.at <= RATE_WINDOW_SECONDS)
  const downloadGroups = splitGroups(project(points, 'download', nowSeconds, scale))
  const uploadGroups = splitGroups(project(points, 'upload', nowSeconds, scale))
  const downloadPath = downloadGroups.map(monotonePath).join(' ') || `M ${CHART_WIDTH} ${CHART_HEIGHT}`
  const uploadPath = uploadGroups.map(monotonePath).join(' ') || `M ${CHART_WIDTH} ${CHART_HEIGHT}`
  const area = downloadGroups
    .filter((group) => group.length > 1)
    .map((group) => {
      const path = monotonePath(group)
      const first = group[0]
      const last = group.at(-1)!
      return `${path} L ${last.x.toFixed(1)} ${CHART_HEIGHT} L ${first.x.toFixed(1)} ${CHART_HEIGHT} Z`
    })
    .join(' ')
  const download = project(points, 'download', nowSeconds, scale)
  const upload = project(points, 'upload', nowSeconds, scale)
  return {
    scale,
    points,
    downloadAnchors: download,
    uploadAnchors: upload,
    downloadPath,
    uploadPath,
    area,
    groups: downloadGroups.length,
    firstX: download[0]?.x ?? CHART_WIDTH,
    lastX: download.at(-1)?.x ?? CHART_WIDTH,
    latestPoint: points.length === 1 ? { download: download[0], upload: upload[0] } : null,
    labels: [1, 0.75, 0.5, 0.25, 0].map((fraction) => formatRateTick(scale * fraction)),
  }
}
