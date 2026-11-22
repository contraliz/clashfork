import assert from 'node:assert/strict'
import { access, mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { chromium } from 'playwright-core'

const browserCandidates = [
  process.env.CLASHFORK_QA_BROWSER,
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
].filter(Boolean)

let executablePath
for (const candidate of browserCandidates) {
  if (await access(candidate).then(() => true, () => false)) {
    executablePath = candidate
    break
  }
}
if (!executablePath) {
  throw new Error('Set CLASHFORK_QA_BROWSER to the path of an installed Chrome or Edge browser.')
}

const screenshotDirectory = process.env.CLASHFORK_QA_SCREENSHOTS === '1'
  ? await mkdtemp(join(tmpdir(), 'clashfork-qa-'))
  : undefined
const url = new URL(process.env.CLASHFORK_QA_URL ?? 'http://127.0.0.1:1420/')
url.searchParams.set('mock', '1')
const browser = await chromium.launch({ executablePath, headless: true })
try {
  const page = await browser.newPage({
    viewport: { width: 1280, height: 800 },
    reducedMotion: 'reduce',
  })
  await page.addInitScript(() => {
    window.__clashforkQaVisibility = 'visible'
    Object.defineProperty(document, 'visibilityState', {
      configurable: true,
      get: () => window.__clashforkQaVisibility,
    })
    Object.defineProperty(document, 'hidden', {
      configurable: true,
      get: () => window.__clashforkQaVisibility === 'hidden',
    })
  })
  page.setDefaultTimeout(25000)
  const errors = []
  const consoleIssues = []
  page.on('pageerror', (error) => errors.push(error.message))
  page.on('console', (message) => {
    if (message.type() === 'error' || message.type() === 'warning') {
      consoleIssues.push(`${message.type()}: ${message.text()}`)
    }
  })

  const heading = (name) => page.getByRole('heading', { name, exact: true }).first()
  const mockCalls = (command) => page.evaluate((name) => window.__clashforkMockCalls?.[name] ?? 0, command)
  const mockPendingCalls = (command) => page.evaluate((name) => window.__clashforkMockPendingCalls?.[name] ?? 0, command)
  const setPageVisibility = (state) => page.evaluate((next) => {
    window.__clashforkQaVisibility = next
    document.dispatchEvent(new Event('visibilitychange'))
  }, state)
  const waitForMockCalls = (command, count) => page.waitForFunction(
    ({ name, expected }) => (window.__clashforkMockCalls?.[name] ?? 0) >= expected,
    { name: command, expected: count },
  )
  const open = async (name) => {
    await page.getByRole('navigation').getByRole('button', { name, exact: true }).click()
    await heading(name).waitFor()
  }
  const capture = async (name) => {
    await page.waitForTimeout(300)
    if (screenshotDirectory) {
      await page.mouse.move(1450, 990)
      await page.screenshot({ path: join(screenshotDirectory, `${name}.png`), fullPage: true })
    }
  }

  await page.goto(url.href, { waitUntil: 'domcontentloaded' })
  await heading('Dashboard').waitFor()
  await waitForMockCalls('get_live_rates', 1)
  await page.waitForFunction(() => (window.__clashforkMockPendingCalls?.get_live_rates ?? 0) > 0)
  const initialRateCalls = await mockCalls('get_live_rates')
  await setPageVisibility('hidden')
  await page.waitForTimeout(1200)
  assert.equal(await mockCalls('get_live_rates'), initialRateCalls, 'hidden Dashboard must cancel in-flight rates polling and its next timer')
  assert.equal(await mockPendingCalls('get_live_rates'), 0, 'hidden Dashboard must ignore the in-flight rates result')
  await setPageVisibility('visible')
  await waitForMockCalls('get_live_rates', initialRateCalls + 1)
  assert.match(await page.title(), /Clashfork/)
  assert.match(await page.locator('#root').innerText(), /Dashboard/)
  assert.equal(await page.getByText(/Vite|Webpack|Next\.js.*error/i).count(), 0)

  await page.keyboard.press('Tab')
  const focus = await page.evaluate(() => {
    const element = document.activeElement
    if (!(element instanceof HTMLElement)) return null
    const style = getComputedStyle(element)
    return {
      tag: element.tagName,
      focusVisible: element.matches(':focus-visible'),
      outlineStyle: style.outlineStyle,
      outlineWidth: style.outlineWidth,
    }
  })
  assert.ok(focus && focus.tag !== 'BODY', 'Tab must move focus into the app')
  assert.equal(focus.focusVisible, true, 'keyboard focus must use :focus-visible')
  assert.notEqual(focus.outlineStyle, 'none', 'focused control must show an outline')
  assert.notEqual(focus.outlineWidth, '0px', 'focused control must show an outline width')

  const vpn = page.getByRole('switch', { name: 'VPN connection' })
  await vpn.click()
  await heading('Connected').waitFor()
  const ruleset = page.getByRole('combobox', { name: 'Ruleset', exact: true })
  await ruleset.selectOption({ label: 'Work' })
  await page.getByText('Rule · Work', { exact: true }).waitFor()
  assert.equal(await vpn.getAttribute('aria-checked'), 'true')
  await ruleset.selectOption({ label: 'AI' })
  await page.getByText('Rule · AI', { exact: true }).first().waitFor()
  await page.locator('.chart-download[d*="L"]').waitFor()
  await page.waitForTimeout(3200)
  assert.equal(await mockCalls('get_live'), 0, 'Dashboard must poll only the lightweight rates endpoint')
  await capture('dashboard')

  await open('Nodes')
  await page.getByText('Falemon', { exact: true }).first().waitFor()
  await capture('nodes')
  await page.getByRole('button', { name: 'Add Nodes', exact: true }).first().click()
  await page.getByRole('dialog').locator('textarea').fill('https://test.invalid/falemon')
  await page.getByRole('button', { name: 'Preview', exact: true }).click()
  const preview = page.getByRole('dialog', { name: 'Preview nodes' })
  await preview.waitFor()
  await preview.getByRole('button', { name: 'Back', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click()

  await open('Rulesets')
  await page.getByText('Miscellaneous', { exact: true }).first().waitFor()
  await capture('rulesets')
  await open('Live Connections')
  await waitForMockCalls('get_live', 1)
  const connectionCalls = await mockCalls('get_live')
  const rateCallsBeforeConnections = await mockCalls('get_live_rates')
  await page.waitForFunction(() => (window.__clashforkMockPendingCalls?.get_live ?? 0) > 0)
  assert.equal(await mockCalls('get_live_rates'), rateCallsBeforeConnections, 'Connections must not poll the Dashboard rates endpoint')
  await setPageVisibility('hidden')
  await page.waitForTimeout(1200)
  assert.equal(await mockCalls('get_live'), connectionCalls, 'hidden Connections must stop full snapshot polling')
  assert.equal(await mockPendingCalls('get_live'), 0, 'hidden Connections must ignore the in-flight full snapshot')
  await setPageVisibility('visible')
  await waitForMockCalls('get_live', connectionCalls + 1)
  await page.getByRole('table').waitFor()
  await page.getByRole('cell', { name: 'ChatGPT.exe', exact: true }).first().waitFor()
  await capture('live')
  await open('Logs')
  await page.getByRole('tab', { name: 'Traffic History', exact: true }).waitFor()
  await waitForMockCalls('get_history', 1)
  const historyCalls = await mockCalls('get_history')
  await page.waitForFunction(() => (window.__clashforkMockPendingCalls?.get_history ?? 0) > 0)
  await setPageVisibility('hidden')
  await page.waitForTimeout(250)
  assert.equal(await mockCalls('get_history'), historyCalls, 'hidden History must not restart a report request')
  assert.equal(await mockPendingCalls('get_history'), 0, 'hidden History must ignore its in-flight report result')
  await setPageVisibility('visible')
  await waitForMockCalls('get_history', historyCalls + 1)
  await page.getByRole('button', { name: '7 days', exact: true }).click()
  await page.getByRole('table').waitFor()
  const historyTable = page.locator('.history-table')
  const chatgptHistoryRows = historyTable.locator('tbody tr').filter({ has: page.getByText('ChatGPT.exe', { exact: true }) })
  await chatgptHistoryRows.filter({ hasText: 'Proxy' }).first().waitFor()
  await chatgptHistoryRows.filter({ hasText: 'Direct' }).first().waitFor()
  await page.getByLabel('Breakdown').selectOption('customGroup')
  const aiGroupRows = historyTable.locator('tbody tr').filter({ has: page.getByText('AI tools', { exact: true }) })
  await aiGroupRows.filter({ hasText: 'Proxy' }).first().waitFor()
  await aiGroupRows.filter({ hasText: 'Direct' }).first().waitFor()
  await page.getByLabel('Breakdown').selectOption('route')
  await historyTable.getByText('Blocked', { exact: true }).first().waitFor()
  await historyTable.getByText('Unknown', { exact: true }).first().waitFor()
  const unattributedRow = historyTable.locator('tbody tr').filter({ has: page.getByText('Unattributed', { exact: true }) })
  await unattributedRow.locator('.route-class-unknown').waitFor()
  await historyTable.getByText('Falemon / Singapore I0', { exact: true }).waitFor()
  const routeFilter = page.getByLabel('Route', { exact: true })
  const subjectFilter = page.getByLabel('App or group', { exact: true })
  const assertHistoryAccounting = async () => {
    await page.locator('.bar-group').first().waitFor()
    await page.waitForFunction(() => !document.querySelector('.chart-loading'))
    const totals = await page.locator('.metric-panel').evaluate((panel) => ({
      download: Number(panel.getAttribute('data-history-total-download')),
      upload: Number(panel.getAttribute('data-history-total-upload')),
    }))
    const rows = await page.locator('.history-table tr[data-history-row-download]').evaluateAll((items) => items.map((item) => ({
      download: Number(item.getAttribute('data-history-row-download')),
      upload: Number(item.getAttribute('data-history-row-upload')),
    })))
    const bars = await page.locator('.bar-group').evaluateAll((items) => items.map((item) => ({
      download: Number(item.getAttribute('data-history-bar-download')),
      upload: Number(item.getAttribute('data-history-bar-upload')),
    })))
    assert.equal(rows.reduce((sum, item) => sum + item.download, 0), totals.download, 'history rows must match filtered download totals')
    assert.equal(rows.reduce((sum, item) => sum + item.upload, 0), totals.upload, 'history rows must match filtered upload totals')
    assert.equal(bars.reduce((sum, item) => sum + item.download, 0), totals.download, 'history bars must match filtered download totals')
    assert.equal(bars.reduce((sum, item) => sum + item.upload, 0), totals.upload, 'history bars must match filtered upload totals')
  }

  await assertHistoryAccounting()
  await routeFilter.focus()
  await page.keyboard.press('Tab')
  await page.keyboard.press('Tab')
  assert.equal(await page.evaluate(() => document.activeElement?.classList.contains('bar-group')), true, 'chart periods must be keyboard reachable')
  const periodInspector = page.locator('.bar-period-inspector')
  let periodDetails = await periodInspector.innerText()
  assert.match(periodDetails, /Download/)
  assert.match(periodDetails, /Upload/)
  assert.match(periodDetails, /No recording gap/)
  assert.ok(await periodInspector.locator('strong').innerText(), 'period details must include a date')
  await page.locator('.bar-group').first().hover()
  assert.match(await page.locator('.bar-group').first().getAttribute('aria-label'), /Download .* Upload .* recording gap/i)
  await page.getByRole('button', { name: '30 days', exact: true }).click()
  const gapPeriod = page.locator('.bar-group[data-history-bar-gap="true"]').first()
  await gapPeriod.waitFor()
  await gapPeriod.hover()
  await periodInspector.getByText('Recording gap', { exact: true }).waitFor()
  periodDetails = await periodInspector.innerText()
  assert.match(periodDetails, /Download/)
  assert.match(periodDetails, /Upload/)
  assert.ok(await periodInspector.locator('strong').innerText(), 'gap details must include a date')
  await assertHistoryAccounting()

  await routeFilter.selectOption('proxy')
  await page.getByLabel('Breakdown').selectOption('customGroup')
  await page.waitForFunction(() => {
    const classes = [...document.querySelectorAll('.history-table .route-class')]
    return classes.length > 0 && classes.every((item) => item.classList.contains('route-class-proxy'))
  })
  assert.equal(await routeFilter.inputValue(), 'proxy', 'breakdown changes must preserve route filters')
  await assertHistoryAccounting()
  await subjectFilter.selectOption({ label: 'Development' })
  await page.waitForFunction(() => {
    const rows = [...document.querySelectorAll('.history-table tr[data-history-row-download]')]
    return rows.length === 1 && rows[0].querySelector('strong')?.textContent === 'Development' && rows[0].querySelector('.route-class-proxy')
  })
  assert.equal(await routeFilter.inputValue(), 'proxy', 'app-group and route filters must compose')
  await assertHistoryAccounting()
  await page.getByRole('button', { name: 'Today', exact: true }).click()
  assert.equal(await routeFilter.inputValue(), 'proxy', 'range changes must preserve route filters')
  assert.ok(await subjectFilter.inputValue(), 'range changes must preserve app-group filters')
  await page.locator('.history-table').getByText('Development', { exact: true }).waitFor()

  await routeFilter.selectOption('direct')
  await page.getByRole('heading', { name: 'No traffic matches these filters', exact: true }).waitFor()
  assert.equal(await page.locator('.metric-panel').getAttribute('data-history-total'), '0')
  assert.ok((await subjectFilter.locator('option').allTextContents()).includes('steam.exe'), 'app choices must remain available when filters match no traffic')
  await assertHistoryAccounting()
  await capture('history-no-match')

  await subjectFilter.selectOption({ label: 'AI tools' })
  await page.locator('.history-table').getByText('AI tools', { exact: true }).waitFor()
  await page.locator('.history-table .route-class-direct').first().waitFor()
  await capture('history-filtered')
  await page.getByLabel('Breakdown').selectOption('app')
  await page.locator('.history-table').getByText('ChatGPT.exe', { exact: true }).waitFor()
  assert.equal(await routeFilter.inputValue(), 'direct')
  await assertHistoryAccounting()

  await subjectFilter.selectOption({ label: 'steam.exe' })
  await page.locator('.history-table').getByText('steam.exe', { exact: true }).waitFor()
  await routeFilter.selectOption('proxy')
  await page.getByRole('heading', { name: 'No traffic matches these filters', exact: true }).waitFor()
  assert.ok((await subjectFilter.locator('option').allTextContents()).includes('steam.exe'))
  await page.getByRole('button', { name: 'Reset filters', exact: true }).first().click()
  assert.equal(await routeFilter.inputValue(), 'all')
  assert.equal(await subjectFilter.inputValue(), 'all')
  await page.getByLabel('Breakdown').selectOption('app')
  await page.locator('.history-table').getByText('Unattributed', { exact: true }).waitFor()
  await page.getByRole('button', { name: '7 days', exact: true }).click()
  await assertHistoryAccounting()
  await capture('logs')
  await page.getByRole('tab', { name: 'Diagnostics', exact: true }).click()
  await page.getByText('Live diagnostics', { exact: true }).waitFor()
  await page.locator('.diagnostic-line').first().waitFor()
  const diagnosticOrder = async () => page.locator('.diagnostic-line').evaluateAll((lines) =>
    lines.map((line) => ({
      id: Number(line.getAttribute('data-diagnostic-id')),
      at: Number(line.getAttribute('data-diagnostic-at')),
    })),
  )
  const assertNewestFirst = (entries) => {
    const expected = [...entries].sort((left, right) => right.at - left.at || right.id - left.id)
    assert.deepEqual(entries, expected, 'diagnostics must be newest first with ID tie breaks')
  }
  const initialDiagnostics = await diagnosticOrder()
  assert.ok(initialDiagnostics.length >= 2, 'fixture diagnostics must render')
  assertNewestFirst(initialDiagnostics)
  assert.equal(initialDiagnostics[0].at, initialDiagnostics[1].at, 'fixture includes an equal-second pair')
  assert.ok(initialDiagnostics[0].id > initialDiagnostics[1].id, 'equal seconds break by newest ID')
  assert.match(await page.locator('.diagnostic-line').first().innerText(), /Northwind refresh failed/)
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await waitForMockCalls('get_diagnostics', 2)
  await page.waitForFunction(() => (window.__clashforkMockPendingCalls?.get_diagnostics ?? 0) > 0)
  assertNewestFirst(await diagnosticOrder())
  const diagnosticCalls = await mockCalls('get_diagnostics')
  await setPageVisibility('hidden')
  await page.waitForTimeout(2300)
  assert.equal(await mockCalls('get_diagnostics'), diagnosticCalls, 'hidden Diagnostics must stop polling')
  await setPageVisibility('visible')
  await waitForMockCalls('get_diagnostics', diagnosticCalls + 1)
  assertNewestFirst(await diagnosticOrder())
  await capture('diagnostics')

  const viewports = [
    { name: '800x640', width: 800, height: 640 },
    { name: '1280x800', width: 1280, height: 800 },
    { name: '1920x1000', width: 1920, height: 1000 },
    { name: '2560x1375', width: 2560, height: 1375 },
  ]
  const screens = ['Dashboard', 'Nodes', 'Rulesets', 'Live Connections', 'Logs']
  for (const screen of screens) {
    for (const viewport of viewports) {
      await page.setViewportSize({ width: viewport.width, height: viewport.height })
      await open(screen)
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `${screen} overflows at ${viewport.name}`)
      const shell = await page.evaluate(() => ({
        sidebarWidth: Math.round(document.querySelector('.sidebar').getBoundingClientRect().width),
        mainWidth: Math.round(document.querySelector('.main').getBoundingClientRect().width),
        pageWidth: Math.round(document.querySelector('.page').getBoundingClientRect().width),
        pageRight: Math.round(document.querySelector('.page').getBoundingClientRect().right),
        navCount: document.querySelectorAll('.nav-item').length,
      }))
      assert.equal(shell.sidebarWidth, viewport.width <= 900 ? 62 : 180, `${screen} sidebar must match the responsive rail at ${viewport.name}`)
      assert.equal(shell.navCount, 5, `${screen} navigation must remain available at ${viewport.name}`)
      assert.ok(shell.pageRight <= viewport.width, `${screen} content must stay within the window at ${viewport.name}`)
      if (viewport.width >= 1920) {
        assert.ok(shell.pageWidth >= 1600, `${screen} should use the wide content pane at ${viewport.name}`)
      }

      if (screen === 'Dashboard') {
        await page.waitForFunction(() =>
          document.querySelectorAll('.chart-point').length === 2 || document.querySelector('.chart-area') !== null,
        )
        const cards = await page.evaluate(() => {
          const vpn = document.querySelector('.vpn-panel').getBoundingClientRect()
          const vpnHeading = document.querySelector('.vpn-heading').getBoundingClientRect()
          const vpnToggle = document.querySelector('.vpn-heading .toggle').getBoundingClientRect()
          const vpnControls = document.querySelector('.control-row').getBoundingClientRect()
          const destination = document.querySelector('.picker-field').getBoundingClientRect()
          const traffic = document.querySelector('.traffic-panel').getBoundingClientRect()
          const presets = document.querySelector('.run-configs').getBoundingClientRect()
          const line = document.querySelector('.chart-download')?.getAttribute('d') ?? ''
          const sampleXs = [...line.matchAll(/(?:M|L)\s+([\d.]+)/g)].map((match) => Number(match[1]))
          return {
            vpnTop: vpn.top,
            vpnBottom: vpn.bottom,
            vpnWidth: vpn.width,
            vpnRight: vpn.right,
            vpnHeadingBottom: vpnHeading.bottom,
            vpnToggleRight: vpnToggle.right,
            vpnControlsTop: vpnControls.top,
            destinationWidth: destination.width,
            trafficTop: traffic.top,
            trafficBottom: traffic.bottom,
            trafficWidth: traffic.width,
            presetsTop: presets.top,
            presetsBottom: presets.bottom,
            sampleXs,
            hasArea: Boolean(document.querySelector('.chart-area')),
            markerXs: [...document.querySelectorAll('.chart-point')].map((point) => Number(point.getAttribute('cx'))),
          }
        })
        assert.ok(cards.vpnTop < cards.trafficTop && cards.trafficTop < cards.presetsTop, `Dashboard must stack VPN, live traffic, and saved configurations at ${viewport.name}`)
        assert.ok(Math.abs(cards.vpnWidth - cards.trafficWidth) < 4, `Dashboard sections must use the same full-width pane at ${viewport.name}`)
        assert.ok(cards.vpnControlsTop >= cards.vpnHeadingBottom, `VPN mode and destination must sit below status and toggle at ${viewport.name}`)
        assert.ok(cards.vpnToggleRight <= cards.vpnRight - 12, `VPN toggle must stay inside the status header at ${viewport.name}`)
        if (viewport.width >= 1600) assert.ok(cards.destinationWidth <= 980, `Wide Dashboard destination field must remain bounded at ${viewport.name}`)
        if (cards.sampleXs.length < 2) {
          assert.equal(cards.hasArea, false, `A sparse live sample must not fill the entire chart at ${viewport.name}`)
          assert.equal(cards.markerXs.length, cards.sampleXs.length * 2, `Sparse live samples must remain visible as points for both directions at ${viewport.name}`)
          if (cards.markerXs.length > 0) assert.ok(cards.markerXs.every((x) => x >= 790), `Sparse live samples must appear at the recent edge at ${viewport.name}`)
        } else {
          assert.equal(cards.hasArea, true, `Multiple live samples should draw a chart area at ${viewport.name}`)
          assert.ok(cards.sampleXs[0] > 0 && cards.sampleXs.at(-1) >= 790, `Live samples must follow their recent time range at ${viewport.name}`)
        }
        if (viewport.width >= 1920) assert.ok(cards.presetsBottom < viewport.height, `Dashboard run configurations should fit in the first viewport at ${viewport.name}`)
      }

      if (screen === 'Rulesets') {
        const editor = page.locator('.ruleset-detail')
        await editor.waitFor()
        assert.ok(await page.getByRole('button', { name: 'New ruleset', exact: true }).isVisible(), `Ruleset header actions must remain visible at ${viewport.name}`)
        const editorRect = await editor.boundingBox()
        assert.ok(editorRect.width > 260, `Ruleset details must retain usable width at ${viewport.name}`)
      }

      if (screen === 'Logs') {
        assert.ok(await page.getByLabel('Route', { exact: true }).isVisible(), `History route filter must remain visible at ${viewport.name}`)
        assert.ok(await page.getByLabel('App or group', { exact: true }).isVisible(), `History app filter must remain visible at ${viewport.name}`)
        const historyLayout = await page.evaluate(() => {
          const toolbar = document.querySelector('.history-toolbar-panel').getBoundingClientRect()
          const route = document.querySelector('[aria-label="Route"]').getBoundingClientRect()
          const app = document.querySelector('[aria-label="App or group"]').getBoundingClientRect()
          const summary = document.querySelector('.metric-panel').getBoundingClientRect()
          const chart = document.querySelector('.chart-panel').getBoundingClientRect()
          return { toolbarWidth: toolbar.width, routeWidth: route.width, appWidth: app.width, summaryTop: summary.top, summaryBottom: summary.bottom, summaryWidth: summary.width, chartTop: chart.top, chartWidth: chart.width }
        })
        assert.ok(historyLayout.summaryBottom < historyLayout.chartTop, `Traffic summary must sit above the chart at ${viewport.name}`)
        assert.ok(Math.abs(historyLayout.summaryWidth - historyLayout.chartWidth) < 4, `Traffic summary and chart must share the full-width pane at ${viewport.name}`)
        if (viewport.width >= 1600) {
          assert.ok(historyLayout.routeWidth <= 240 && historyLayout.appWidth <= 500, `Wide history filters must stay bounded inside their toolbar at ${viewport.name}`)
          assert.ok(historyLayout.toolbarWidth < 2000, `History controls should remain within the intentional pane width at ${viewport.name}`)
        }
        await page.getByRole('tab', { name: 'Diagnostics', exact: true }).click()
        await page.locator('.diagnostics-panel').waitFor()
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `Diagnostics must not overflow at ${viewport.name}`)
        await page.getByRole('tab', { name: 'Traffic History', exact: true }).click()
      }

      const greenElements = await page.evaluate(() => {
        const parse = (value) => {
          const match = value.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)(?:,\s*([\d.]+))?\)/)
          if (!match) return null
          const alpha = match[4] === undefined ? 1 : Number(match[4])
          return alpha > 0.1 ? [Number(match[1]), Number(match[2]), Number(match[3])] : null
        }
        const greenish = (value) => {
          const rgb = parse(value)
          return Boolean(rgb && rgb[1] > 90 && rgb[1] > rgb[0] * 1.25 && rgb[1] > rgb[2] * 1.25)
        }
        return [...document.querySelectorAll('*')]
          .filter((element) => {
            const style = getComputedStyle(element)
            if (style.display === 'none' || style.visibility === 'hidden') return false
            return [
              style.color,
              style.backgroundColor,
              style.borderTopColor,
              style.borderRightColor,
              style.borderBottomColor,
              style.borderLeftColor,
              style.fill,
              style.stroke,
            ].some(greenish)
          })
          .map((element) => `${element.tagName.toLowerCase()}.${element.className}`)
          .slice(0, 5)
      })
      assert.deepEqual(greenElements, [], `${screen} must not use green UI colors at ${viewport.name}`)
      if (screenshotDirectory && (viewport.name === '800x640' || viewport.name === '1920x1000' || (screen === 'Dashboard' && viewport.name === '2560x1375'))) {
        const slug = screen.toLowerCase().replaceAll(' ', '-')
        await page.mouse.move(viewport.width - 10, viewport.height - 10)
        await page.waitForTimeout(150)
        await page.screenshot({ path: join(screenshotDirectory, `${slug}-${viewport.name}.png`) })
      }
    }
  }
  assert.deepEqual(errors, [])
  assert.deepEqual(consoleIssues, [])
  console.log('Browser fixture passed on all five screens at 800x640, 1280x800, 1920x1000, and 2560x1375, including filter accounting, chart inspection, vertical traffic layouts, and wide-window fit.')

  const freshPage = await browser.newPage({
    viewport: { width: 1280, height: 800 },
    reducedMotion: 'reduce',
  })
  freshPage.setDefaultTimeout(25000)
  const freshErrors = []
  const freshConsoleIssues = []
  freshPage.on('pageerror', (error) => freshErrors.push(error.message))
  freshPage.on('console', (message) => {
    if (message.type() === 'error' || message.type() === 'warning') {
      freshConsoleIssues.push(`${message.type()}: ${message.text()}`)
    }
  })
  const freshUrl = new URL(url.href)
  freshUrl.searchParams.set('fixture', 'empty-rulesets')
  await freshPage.goto(freshUrl.href, { waitUntil: 'domcontentloaded' })
  await freshPage.getByRole('heading', { name: 'Dashboard', exact: true }).waitFor()
  await freshPage.getByRole('navigation').getByRole('button', { name: 'Rulesets', exact: true }).click()
  await freshPage.getByRole('heading', { name: 'Rulesets', exact: true }).waitFor()
  assert.equal(await freshPage.getByText('No rulesets yet.', { exact: true }).count(), 1)
  await freshPage.getByRole('button', { name: 'New ruleset', exact: true }).first().click()
  const rulesetName = freshPage.getByRole('textbox', { name: 'Ruleset name', exact: true })
  await rulesetName.fill('Fresh')
  const miscellaneous = freshPage.getByRole('combobox', { name: 'Miscellaneous action', exact: true })
  assert.equal(await miscellaneous.inputValue(), 'direct', 'fresh rulesets default Miscellaneous to Direct')
  await miscellaneous.selectOption('block')
  await freshPage.getByRole('button', { name: 'Save ruleset', exact: true }).click()
  await freshPage.locator('.ruleset-index').getByRole('button', { name: /Fresh/ }).waitFor()
  assert.equal(await miscellaneous.inputValue(), 'block')
  await freshPage.getByRole('navigation').getByRole('button', { name: 'Dashboard', exact: true }).click()
  await freshPage.getByRole('heading', { name: 'Dashboard', exact: true }).waitFor()
  const freshSelector = freshPage.getByRole('combobox', { name: 'Ruleset', exact: true })
  await freshSelector.selectOption({ label: 'Fresh' })
  await freshPage.getByText('Rule · Fresh', { exact: true }).first().waitFor()

  await freshPage.getByRole('navigation').getByRole('button', { name: 'Rulesets', exact: true }).click()
  await freshPage.getByRole('heading', { name: 'Rulesets', exact: true }).waitFor()
  await freshPage.getByRole('button', { name: 'New ruleset', exact: true }).first().click()
  await freshPage.getByRole('button', { name: 'Save ruleset', exact: true }).click()
  await freshPage.getByRole('alert').getByText('Name the ruleset before saving.', { exact: true }).waitFor()
  assert.equal(await freshPage.getByRole('textbox', { name: 'Ruleset name', exact: true }).inputValue(), '')
  await freshPage.getByRole('textbox', { name: 'Ruleset name', exact: true }).fill('Fresh')
  await freshPage.getByRole('button', { name: 'Save ruleset', exact: true }).click()
  await freshPage.getByRole('alert').getByText('A ruleset with this name already exists.', { exact: true }).waitFor()
  assert.equal(await freshPage.getByRole('textbox', { name: 'Ruleset name', exact: true }).inputValue(), 'Fresh')
  assert.deepEqual(freshErrors, [])
  assert.deepEqual(freshConsoleIssues, [])
  if (screenshotDirectory) {
    await freshPage.waitForTimeout(300)
    await freshPage.screenshot({ path: join(screenshotDirectory, 'ruleset-errors.png'), fullPage: true })
  }
  console.log('Fresh ruleset fixture passed: empty setup, Direct default, Block save, Dashboard selection, duplicate/empty validation with draft retention.')
  if (screenshotDirectory) console.log(screenshotDirectory)
} finally {
  await browser.close()
}
