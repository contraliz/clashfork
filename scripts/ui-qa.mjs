import assert from 'node:assert/strict'
import { access, mkdtemp, readFile } from 'node:fs/promises'
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
const packageVersion = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8')).version
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
  await heading('Proxy traffic').waitFor()
  await page.getByText('Proxy download', { exact: true }).first().waitFor()
  await page.getByText('Proxy upload', { exact: true }).first().waitFor()
  assert.equal(await page.getByRole('heading', { name: 'Live traffic', exact: true }).count(), 0, 'Dashboard traffic must be labeled as proxy-only')
  assert.equal(await mockCalls('get_live_rates'), 0, 'the app must not poll proxy rates while the VPN is disconnected')
  assert.match(await page.title(), /Clashfork/)
  assert.match(await page.locator('#root').innerText(), /Dashboard/)
  assert.equal(await page.locator('.sidebar-foot .version').innerText(), packageVersion, 'sidebar version must match the package manifest')
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
  await waitForMockCalls('get_live_rates', 1)
  await page.waitForFunction(() => (window.__clashforkMockPendingCalls?.get_live_rates ?? 0) > 0)
  const initialRateCalls = await mockCalls('get_live_rates')
  await setPageVisibility('hidden')
  await page.waitForTimeout(1200)
  assert.equal(await mockCalls('get_live_rates'), initialRateCalls, 'hidden app must cancel in-flight rates polling and its next timer')
  assert.equal(await mockPendingCalls('get_live_rates'), 0, 'hidden app must ignore the in-flight rates result')
  await setPageVisibility('visible')
  await waitForMockCalls('get_live_rates', initialRateCalls + 1)
  const ruleset = page.getByRole('combobox', { name: 'Ruleset', exact: true })
  await ruleset.selectOption({ label: 'Work' })
  await page.getByText('Rule · Work', { exact: true }).waitFor()
  assert.equal(await vpn.getAttribute('aria-checked'), 'true')
  await ruleset.selectOption({ label: 'AI' })
  await page.getByText('Rule · AI', { exact: true }).first().waitFor()
  await page.waitForFunction(() => (document.querySelector('.chart-download')?.getAttribute('d') ?? '').includes('C '))
  await page.waitForTimeout(3200)
  const expectedProxyRates = await page.evaluate(() => {
    const rates = window.__clashforkMockLastRates
    const formatRate = (value) => {
      if (!Number.isFinite(value) || value <= 0) return '0 B/s'
      const units = ['B', 'KB', 'MB', 'GB', 'TB']
      let amount = value
      let unit = 0
      while (amount >= 1024 && unit < units.length - 1) {
        amount /= 1024
        unit += 1
      }
      const formatted = amount >= 100 || unit === 0 ? amount.toFixed(0) : amount.toFixed(1)
      return `${formatted} ${units[unit]}/s`
    }
    return {
      download: formatRate(rates?.proxyDownloadRate ?? NaN),
      upload: formatRate(rates?.proxyUploadRate ?? NaN),
    }
  })
  assert.equal(await page.locator('.rate-block strong').nth(0).innerText(), expectedProxyRates.download, 'Dashboard must render only the proxy-class download rate from lightweight IPC')
  assert.equal(await page.locator('.rate-block strong').nth(1).innerText(), expectedProxyRates.upload, 'Dashboard must render only the proxy-class upload rate from lightweight IPC')
  assert.equal(await mockCalls('get_live'), 0, 'Dashboard must poll only the lightweight rates endpoint')
  const rateChart = page.locator('.traffic-chart')
  assert.equal(await rateChart.getAttribute('data-rate-window'), '60')
  const pointsBeforeNavigation = Number(await rateChart.getAttribute('data-rate-points'))
  assert.ok(pointsBeforeNavigation >= 2, 'the proxy recorder must collect recent measured points')
  const rateCallsBeforeNavigation = await mockCalls('get_live_rates')
  await open('Nodes')
  await page.getByText('Falemon', { exact: true }).first().waitFor()
  assert.equal(await page.getByRole('heading', { name: 'Proxy traffic', exact: true }).count(), 0, 'hidden Dashboard must not stay mounted')
  await waitForMockCalls('get_live_rates', rateCallsBeforeNavigation + 2)
  await page.waitForTimeout(1100)
  await open('Dashboard')
  await page.locator('.traffic-chart').waitFor()
  await page.waitForFunction((count) => Number(document.querySelector('.traffic-chart')?.getAttribute('data-rate-points')) > count, pointsBeforeNavigation)
  const pointsAfterNavigation = Number(await page.locator('.traffic-chart').getAttribute('data-rate-points'))
  assert.ok(pointsAfterNavigation > pointsBeforeNavigation, 'recent proxy points must keep recording across navigation')
  const groupsBeforeGap = Number(await page.locator('.traffic-chart').getAttribute('data-rate-groups'))
  await setPageVisibility('hidden')
  await page.waitForTimeout(3200)
  await setPageVisibility('visible')
  await page.waitForFunction((count) => Number(document.querySelector('.traffic-chart')?.getAttribute('data-rate-groups')) > count, groupsBeforeGap)
  const gapPath = await page.locator('.chart-download').getAttribute('d')
  const gapGroups = Number(await page.locator('.traffic-chart').getAttribute('data-rate-groups'))
  assert.equal((gapPath.match(/M /g) ?? []).length, gapGroups, 'each missing-sample period must start a separate smooth segment')
  await page.evaluate(() => { window.__clashforkQaRatesUnavailable = true })
  await page.waitForFunction(() => document.querySelector('.rate-block strong')?.textContent?.includes('—'))
  const pointsDuringUnavailable = Number(await page.locator('.traffic-chart').getAttribute('data-rate-points'))
  assert.ok(pointsDuringUnavailable >= 2, 'unavailable samples must keep recent proxy points')
  await page.waitForTimeout(1400)
  assert.equal(Number(await page.locator('.traffic-chart').getAttribute('data-rate-points')), pointsDuringUnavailable, 'brief unavailable samples must not clear recorded points')
  assert.match(await page.locator('.rate-block strong').nth(0).innerText(), /— B\/s/, 'unavailable samples must not show stale current rates')
  await page.evaluate(() => { window.__clashforkQaRatesUnavailable = false })
  await page.waitForFunction(() => !document.querySelector('.rate-block strong')?.textContent?.includes('—'))
  await page.waitForFunction((count) => Number(document.querySelector('.traffic-chart')?.getAttribute('data-rate-points')) > count, pointsDuringUnavailable)
  await capture('dashboard')

  await open('Nodes')
  await page.getByText('Falemon', { exact: true }).first().waitFor()
  assert.equal(await page.locator('.service-error').count(), 0, 'usable cached nodes must not show routine refresh errors')
  assert.equal(await mockCalls('test_node'), 0, 'opening Nodes must never trigger a latency probe')
  assert.equal(await mockCalls('test_all_nodes'), 0, 'opening Nodes must never trigger a sweep')
  const singaporeRow = page.locator('[data-node-id="node_falemon_sg"]')
  await singaporeRow.getByRole('button', { name: 'Test Singapore I0 latency', exact: true }).click()
  assert.equal(await mockCalls('test_node'), 1, 'an individual latency probe must be explicitly user initiated')
  await singaporeRow.locator('.latency-value[data-latency-ms="142"]').waitFor()
  assert.equal(await singaporeRow.locator('.latency-value').innerText(), '142 ms', 'individual node test must publish a measured latency')
  await page.getByRole('button', { name: 'Test all nodes', exact: true }).click()
  await page.getByText('5 of 5 responded', { exact: true }).waitFor()
  assert.equal(await mockCalls('test_all_nodes'), 1, 'Test all nodes must run only after an explicit click')
  await open('Dashboard')
  await page.getByRole('button', { name: 'Global', exact: true }).click()
  await page.locator('.picker-field .selected-node-latency .latency-value[data-latency-ms="142"]').waitFor()
  assert.equal(await page.locator('.picker-field .selected-node-latency .latency-value').innerText(), '142 ms', 'Dashboard Global picker must share the cached node latency')
  await open('Nodes')
  await capture('nodes')
  await page.getByRole('button', { name: 'Add Nodes', exact: true }).first().click()
  await page.getByRole('dialog').locator('textarea').fill('https://test.invalid/falemon')
  await page.getByRole('button', { name: 'Preview', exact: true }).click()
  const preview = page.getByRole('dialog', { name: 'Preview nodes' })
  await preview.waitFor()
  await preview.getByRole('button', { name: 'Back', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click()

  await open('Rulesets')
  await page.getByText('Everything else', { exact: true }).waitFor()
  const ruleDraftName = page.getByRole('textbox', { name: 'Ruleset name', exact: true })
  await ruleDraftName.fill('AI draft')
  await page.getByRole('button', { name: 'Undo', exact: true }).click()
  assert.equal(await ruleDraftName.inputValue(), 'AI', 'Undo restores selected saved ruleset')
  const firstRule = page.locator('.unified-rule').first()
  const expandRule = firstRule.getByRole('button', { name: 'OpenAI', exact: true })
  const namedDestination = firstRule.getByRole('combobox', { name: 'Destination for OpenAI', exact: true })
  assert.equal(await namedDestination.locator('option').nth(0).innerText(), 'Direct')
  assert.equal(await namedDestination.locator('option').nth(1).innerText(), 'Block')
  assert.ok(await namedDestination.locator('option[value="node:node_falemon_sg"]').count(), 'named rules list available local nodes')
  assert.equal(await expandRule.getAttribute('aria-expanded'), 'false')
  assert.equal(await firstRule.locator('.unified-rule-conditions').count(), 0, 'collapsed rule headers omit the condition summary')
  await namedDestination.selectOption('direct')
  assert.equal(await expandRule.getAttribute('aria-expanded'), 'false', 'changing a collapsed rule destination does not expand it')
  assert.equal(await namedDestination.inputValue(), 'direct')
  await namedDestination.selectOption('node:node_falemon_taipei')
  assert.equal(await expandRule.getAttribute('aria-expanded'), 'false', 'proxy destination changes also keep the rule collapsed')
  await namedDestination.selectOption('node:node_falemon_sg')
  await expandRule.click()
  assert.equal(await firstRule.locator('.unified-destination-field').count(), 1, 'expanded rules keep exactly one destination selector in the row')
  await firstRule.getByRole('textbox', { name: 'Rule name', exact: true }).fill('OpenAI draft')
  const catchallDestination = page.getByRole('combobox', { name: 'Everything else destination', exact: true })
  assert.equal(await catchallDestination.locator('option').nth(0).innerText(), 'Direct')
  assert.equal(await catchallDestination.locator('option').nth(1).innerText(), 'Block')
  await catchallDestination.selectOption('node:node_falemon_taipei')
  await firstRule.getByRole('button', { name: 'Add condition', exact: true }).click()
  assert.equal(await firstRule.locator('.unified-condition').count(), 2, 'conditions edit inline on their owning rule')
  assert.equal(await firstRule.getByLabel('Copy conditions').count(), 0, 'the expanded editor does not show a condition copy control')
  await firstRule.getByRole('textbox', { name: 'Condition 2 value', exact: true }).fill('chatgpt.com')
  assert.equal(await page.getByRole('button', { name: 'Import JSON', exact: true }).isVisible(), true)
  assert.equal(await page.getByRole('button', { name: 'Export ruleset', exact: true }).isVisible(), true)
  await firstRule.getByRole('button', { name: 'Add condition', exact: true }).click()
  await firstRule.getByRole('textbox', { name: 'Condition 3 value', exact: true }).fill('chatgpt.com')
  await page.getByRole('button', { name: 'Export ruleset', exact: true }).click()
  await page.getByRole('alert').getByText('Routing files can be exported only when rules and conditions are valid.', { exact: true }).waitFor()
  await firstRule.getByRole('button', { name: 'Remove condition 3', exact: true }).click()
  assert.equal(await firstRule.locator('.unified-condition').count(), 2, 'invalid exports must leave the draft unchanged')
  const [routingExport] = await Promise.all([
    page.waitForEvent('download'),
    page.getByRole('button', { name: 'Export ruleset', exact: true }).click(),
  ])
  await page.getByText('Downloaded clashfork-ruleset-AI.json.', { exact: true }).waitFor()
  const routingBytes = await readFile(await routingExport.path())
  const routingDoc = JSON.parse(routingBytes.toString('utf8'))
  assert.equal(routingDoc.format, 'clashfork-routing')
  assert.equal(routingDoc.version, 1)
  assert.equal(routingDoc.type, 'ruleset')
  assert.equal(routingDoc.rules[0].conditions.length, 2)
  assert.equal(routingDoc.rules[0].destination.kind, 'proxy')
  assert.equal(routingDoc.miscellaneous.kind, 'proxy', 'catch-all proxy destinations export portably')
  assert.notEqual(routingDoc.rules[0].destination.proxyRef, routingDoc.miscellaneous.proxyRef, 'distinct rule and fallback nodes export as distinct opaque references')
  assert.equal(Object.hasOwn(routingDoc.miscellaneous, 'nodeId'), false, 'catch-all export must not include local node IDs')
  assert.equal(Object.hasOwn(routingDoc.rules[0].destination, 'nodeId'), false, 'exports must not include local node IDs')
  assert.equal(Object.hasOwn(routingDoc.rules[0].conditions[0], 'id'), false, 'exports must omit local condition IDs')
  assert.equal(JSON.stringify(routingDoc).includes('node_falemon'), false, 'exports must not include internal routing IDs')
  assert.equal(JSON.stringify(routingDoc).includes('serviceId'), false, 'exports must not include provider identifiers')
  for (const forbidden of ['server', 'password', 'credential', 'sourceHost', 'userinfo']) {
    assert.equal(JSON.stringify(routingDoc).includes(forbidden), false, `exports must not include ${forbidden}`)
  }
  await page.getByRole('button', { name: 'Import JSON', exact: true }).click()
  await page.locator('input[type=file]').setInputFiles({ name: 'unsupported.json', mimeType: 'application/json', buffer: Buffer.from(JSON.stringify({ format: 'clashfork-routing', version: 9, type: 'rule' })) })
  await page.getByRole('alert').getByText('Unsupported routing file format or version.', { exact: true }).waitFor()
  assert.equal(await ruleDraftName.inputValue(), 'AI', 'invalid import must not change the selected draft')
  assert.equal(await page.getByRole('dialog').count(), 0, 'invalid input must not open an import preview')
  await page.getByRole('button', { name: 'Import JSON', exact: true }).click()
  await page.locator('input[type=file]').setInputFiles({ name: 'ruleset-export.json', mimeType: 'application/json', buffer: routingBytes })
  const routingImport = page.getByRole('dialog', { name: 'Import ruleset' })
  await routingImport.waitFor()
  await routingImport.getByText('Choose local nodes for the proxy destinations. Save the draft to apply it.', { exact: true }).waitFor()
  const mappings = routingImport.getByLabel(/Map .* to local node/)
  assert.equal(await mappings.count(), 2, 'each distinct rule or fallback proxy reference needs its own mapping')
  assert.equal(await mappings.nth(0).inputValue(), '', 'proxy mappings must start unselected')
  assert.equal(await mappings.nth(1).inputValue(), '', 'fallback proxy mapping must start unselected')
  assert.equal(await mappings.nth(0).locator('option').first().innerText(), 'Choose a node')
  assert.equal(await routingImport.getByRole('button', { name: 'Use imported draft', exact: true }).isDisabled(), true, 'proxy imports must require explicit local mapping')
  await mappings.nth(0).selectOption('node_falemon_sg')
  await mappings.nth(1).selectOption('node_falemon_taipei')
  const importNameField = routingImport.getByRole('textbox', { name: 'Ruleset name', exact: true })
  await importNameField.click()
  await importNameField.press(process.platform === 'darwin' ? 'Meta+a' : 'Control+a')
  await importNameField.pressSequentially('AI import typing', { delay: 40 })
  assert.equal(await importNameField.inputValue(), 'AI import typing', 'typing the import name must keep every keystroke')
  assert.equal(await importNameField.evaluate(element => element === document.activeElement), true, 'typing the import name must retain keyboard focus')
  await routingImport.getByRole('button', { name: 'Use imported draft', exact: true }).click()
  assert.equal(await page.getByRole('textbox', { name: 'Ruleset name', exact: true }).inputValue(), 'AI import typing', 'the typed import name must become the draft name')
  assert.equal(await page.getByRole('combobox', { name: 'Everything else destination', exact: true }).inputValue(), 'node:node_falemon_taipei', 'import maps fallback proxy to the explicit local node')
  await page.getByRole('button', { name: 'Import JSON', exact: true }).click()
  await page.locator('input[type=file]').setInputFiles({ name: 'ruleset-export.json', mimeType: 'application/json', buffer: routingBytes })
  const cancelledImport = page.getByRole('dialog', { name: 'Import ruleset' })
  await cancelledImport.waitFor()
  await cancelledImport.getByRole('button', { name: 'Cancel import', exact: true }).click()
  assert.equal(await page.getByRole('dialog').count(), 0, 'Cancel import must close the preview')
  assert.equal(await page.getByRole('textbox', { name: 'Ruleset name', exact: true }).inputValue(), 'AI import typing', 'Cancel import must preserve the imported draft')
  await page.waitForFunction(() => document.activeElement?.textContent?.trim() === 'Import JSON')
  const rulesetPicker = page.getByRole('combobox', { name: 'Ruleset', exact: true })
  await rulesetPicker.selectOption('ruleset_ai')
  assert.equal(await page.locator('.unified-rule-name').first().innerText(), 'OpenAI draft', 'import must retain the existing unsaved ruleset edits')
  assert.equal(await rulesetPicker.locator('option').filter({ hasText: 'AI import typing · Draft' }).count(), 1, 'imported rulesets remain drafts until Save')
  const [ruleExport] = await Promise.all([
    page.waitForEvent('download'),
    page.locator('.unified-rule').first().getByRole('button', { name: 'Export rule', exact: true }).click(),
  ])
  await page.getByText('Downloaded clashfork-rule-OpenAI draft.json.', { exact: true }).waitFor()
  const ruleBytes = await readFile(await ruleExport.path())
  const ruleDoc = JSON.parse(ruleBytes.toString('utf8'))
  assert.equal(ruleDoc.type, 'rule')
  assert.equal(ruleDoc.rule.name, 'OpenAI draft')
  assert.equal(ruleDoc.rule.conditions.length, 2)
  await page.getByRole('button', { name: 'Import JSON', exact: true }).click()
  await page.locator('input[type=file]').setInputFiles({ name: 'rule-export.json', mimeType: 'application/json', buffer: ruleBytes })
  const ruleImport = page.getByRole('dialog', { name: 'Import rule' })
  await ruleImport.waitFor()
  await ruleImport.getByLabel(/Map .* to local node/).selectOption('node_falemon_sg')
  await ruleImport.getByRole('button', { name: 'Use imported draft', exact: true }).click()
  assert.equal(await page.locator('.unified-rule').count(), 3, 'single-rule import appends to the selected unsaved draft')
  assert.equal(await rulesetPicker.inputValue(), 'ruleset_ai', 'single-rule import keeps the current ruleset draft selected')
  await capture('rulesets')
  await open('Live Connections')
  await waitForMockCalls('get_live', 1)
  const connectionCalls = await mockCalls('get_live')
  const rateCallsBeforeConnections = await mockCalls('get_live_rates')
  await page.waitForFunction(() => (window.__clashforkMockPendingCalls?.get_live ?? 0) > 0)
  await waitForMockCalls('get_live_rates', rateCallsBeforeConnections + 1)
  assert.equal(await mockCalls('get_live'), connectionCalls, 'Connections must keep using only its full live snapshot poll')
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
      proxy: Number(item.getAttribute('data-history-bar-proxy')),
      direct: Number(item.getAttribute('data-history-bar-direct')),
    })))
    assert.equal(rows.reduce((sum, item) => sum + item.download, 0), totals.download, 'history rows must match filtered download totals')
    assert.equal(rows.reduce((sum, item) => sum + item.upload, 0), totals.upload, 'history rows must match filtered upload totals')
    assert.equal(bars.reduce((sum, item) => sum + item.download, 0), totals.download, 'history bars must match filtered download totals')
    assert.equal(bars.reduce((sum, item) => sum + item.upload, 0), totals.upload, 'history bars must match filtered upload totals')
    assert.ok(bars.every((item) => Number.isFinite(item.proxy) && Number.isFinite(item.direct) && item.proxy >= 0 && item.direct >= 0), 'chart lanes must use nonnegative measured route totals')
    assert.ok(bars.reduce((sum, item) => sum + item.proxy + item.direct, 0) <= totals.download + totals.upload, 'route lanes must not exceed authoritative report totals')
  }

  const assertRouteLanes = async (filter) => {
    const totals = await page.locator('.bar-group').evaluateAll((items) => items.reduce((sum, item) => ({
      proxy: sum.proxy + Number(item.getAttribute('data-history-bar-proxy')),
      direct: sum.direct + Number(item.getAttribute('data-history-bar-direct')),
    }), { proxy: 0, direct: 0 }))
    if (filter === 'proxy') {
      assert.ok(totals.proxy > 0, 'Proxy filter should show measured Proxy traffic')
      assert.equal(totals.direct, 0, 'Proxy filter must keep the right Direct lane empty')
    } else if (filter === 'direct') {
      assert.ok(totals.direct > 0, 'Direct filter should show measured Direct traffic')
      assert.equal(totals.proxy, 0, 'Direct filter must keep the left Proxy lane empty')
    }
  }

  await assertHistoryAccounting()
  assert.deepEqual(
    await page.locator('.bar-group').first().locator('.bar-route-slot').evaluateAll((items) => items.map((item) => item.getAttribute('data-history-route'))),
    ['proxy', 'direct'],
    'each period must keep Proxy on the left and Direct on the right',
  )
  const chartPairLayout = await page.locator('.bar-group').evaluateAll((groups) => groups.map((group) => {
    const bounds = group.getBoundingClientRect()
    const [proxy, direct] = [...group.querySelectorAll('.bar-route-slot')].map((item) => item.getBoundingClientRect())
    return {
      groupLeft: bounds.left,
      groupRight: bounds.right,
      groupCenter: (bounds.left + bounds.right) / 2,
      proxyCenter: (proxy.left + proxy.right) / 2,
      directCenter: (direct.left + direct.right) / 2,
      pairGap: direct.left - proxy.right,
    }
  }))
  assert.ok(chartPairLayout.every((period) => period.pairGap >= 0 && period.pairGap <= 3), 'each period must draw adjacent route bars with a small internal gap')
  assert.ok(chartPairLayout.every((period) => Math.abs((period.proxyCenter + period.directCenter) / 2 - period.groupCenter) < 2), 'route pairs must be centered within their shared date group')
  assert.ok(chartPairLayout.slice(1).every((period, index) => period.groupLeft - chartPairLayout[index].groupRight > 0), 'date groups must remain visually separated')
  await routeFilter.focus()
  await page.keyboard.press('Tab')
  await page.keyboard.press('Tab')
  assert.equal(await page.evaluate(() => document.activeElement?.classList.contains('bar-group')), true, 'chart periods must be keyboard reachable')
  const periodInspector = page.locator('.bar-period-inspector')
  let periodDetails = await periodInspector.innerText()
  assert.match(periodDetails, /Proxy/)
  assert.match(periodDetails, /Direct/)
  assert.match(periodDetails, /No recording gap/)
  assert.ok(await periodInspector.locator('strong').innerText(), 'period details must include a date')
  await page.locator('.bar-group').first().hover()
  assert.match(await page.locator('.bar-group').first().getAttribute('aria-label'), /Proxy .* Direct .* recording gap/i)
  await page.getByRole('button', { name: '30 days', exact: true }).click()
  const gapPeriod = page.locator('.bar-group[data-history-bar-gap="true"]').first()
  await gapPeriod.waitFor()
  await gapPeriod.hover()
  await periodInspector.getByText('Recording gap', { exact: true }).waitFor()
  periodDetails = await periodInspector.innerText()
  assert.match(periodDetails, /Proxy/)
  assert.match(periodDetails, /Direct/)
  assert.ok(await periodInspector.locator('strong').innerText(), 'gap details must include a date')
  await assertHistoryAccounting()

  await routeFilter.selectOption('proxy')
  await page.getByLabel('Breakdown').selectOption('customGroup')
  await page.waitForFunction(() => {
    const classes = [...document.querySelectorAll('.history-table .route-class')]
    return classes.length > 0 && classes.every((item) => item.classList.contains('route-class-proxy'))
  })
  await assertRouteLanes('proxy')
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
  const noMatchLanes = await page.locator('.bar-group').evaluateAll((items) => items.every((item) =>
    Number(item.getAttribute('data-history-bar-proxy')) === 0 && Number(item.getAttribute('data-history-bar-direct')) === 0,
  ))
  assert.equal(noMatchLanes, true, 'no-match chart lanes must remain empty')
  assert.ok((await subjectFilter.locator('option').allTextContents()).includes('steam.exe'), 'app choices must remain available when filters match no traffic')
  await assertHistoryAccounting()
  await capture('history-no-match')

  await subjectFilter.selectOption({ label: 'AI tools' })
  await page.locator('.history-table').getByText('AI tools', { exact: true }).waitFor()
  await page.locator('.history-table .route-class-direct').first().waitFor()
  await assertRouteLanes('direct')
  await capture('history-filtered')
  await page.getByLabel('Breakdown').selectOption('app')
  await page.locator('.history-table').getByText('ChatGPT.exe', { exact: true }).waitFor()
  assert.equal(await routeFilter.inputValue(), 'direct')
  await assertHistoryAccounting()
  const historyCallsBeforeNavigation = await mockCalls('get_history')
  await open('Nodes')
  await page.getByText('Falemon', { exact: true }).first().waitFor()
  assert.equal(await mockCalls('get_history'), historyCallsBeforeNavigation, 'hidden Traffic History must stop loading reports')
  await open('Logs')
  await page.getByRole('tab', { name: 'Traffic History', exact: true }).waitFor()
  assert.equal(await page.getByRole('tab', { name: 'Traffic History', exact: true }).getAttribute('aria-selected'), 'true', 'Logs subtab must survive navigation')
  assert.equal(await page.getByRole('button', { name: 'Today', exact: true }).getAttribute('aria-pressed'), 'true', 'History range must survive navigation')
  assert.equal(await page.getByLabel('Breakdown').inputValue(), 'app', 'History breakdown must survive navigation')
  assert.equal(await routeFilter.inputValue(), 'direct', 'History route filter must survive navigation')
  assert.equal(await subjectFilter.locator('option:checked').innerText(), 'AI tools', 'History app-group filter must survive navigation')
  await page.locator('.history-table').getByText('ChatGPT.exe', { exact: true }).waitFor()
  await assertHistoryAccounting()

  await subjectFilter.selectOption({ label: 'steam.exe' })
  await page.locator('.history-table').getByText('steam.exe', { exact: true }).waitFor()
  await routeFilter.selectOption('proxy')
  await page.getByRole('heading', { name: 'No traffic matches these filters', exact: true }).waitFor()
  assert.ok((await subjectFilter.locator('option').allTextContents()).includes('steam.exe'))
  await page.getByRole('button', { name: 'Reset filters', exact: true }).first().click()
  assert.equal(await routeFilter.inputValue(), 'all')
  assert.equal(await subjectFilter.inputValue(), 'all')
  await page.getByLabel('Breakdown').selectOption('rule')
  await page.locator('.history-table').getByText('Google', { exact: true }).waitFor()
  await page.locator('.history-table').getByText('YouTube', { exact: true }).waitFor()
  await page.locator('.history-table').getByText('ChatGPT direct rule', { exact: true }).waitFor()
  await page.locator('.history-table').getByText('Everything else', { exact: true }).waitFor()
  assert.equal(await page.locator('.history-table').getByText('Unattributed', { exact: true }).count(), 1, 'unknown and legacy rule identity remains unattributed')
  assert.equal((await page.locator('.history-table thead th').first().innerText()).toLowerCase(), 'rule')
  await assertHistoryAccounting()
  await open('Nodes')
  await page.getByText('Falemon', { exact: true }).first().waitFor()
  await open('Logs')
  assert.equal(await page.getByLabel('Breakdown').inputValue(), 'rule', 'ruleset-rule breakdown must survive navigation')
  await page.locator('.history-table').getByText('Google', { exact: true }).waitFor()
  await assertHistoryAccounting()
  await routeFilter.selectOption('proxy')
  await subjectFilter.selectOption({ label: 'AI tools' })
  await page.locator('.history-table').getByText('Google', { exact: true }).waitFor()
  await page.locator('.history-table').getByText('YouTube', { exact: true }).waitFor()
  assert.equal(await page.locator('.history-table').getByText('ChatGPT direct rule', { exact: true }).count(), 0, 'Proxy route filter must exclude the Direct rule')
  await assertHistoryAccounting()
  await page.getByRole('button', { name: 'Reset filters', exact: true }).first().click()
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
  await open('Nodes')
  await page.getByText('Falemon', { exact: true }).first().waitFor()
  await open('Logs')
  assert.equal(await page.getByRole('tab', { name: 'Diagnostics', exact: true }).getAttribute('aria-selected'), 'true', 'selected Logs subtab must survive navigation')
  await page.getByText('Live diagnostics', { exact: true }).waitFor()
  await page.getByRole('tab', { name: 'Traffic History', exact: true }).click()
  assert.equal(await page.getByRole('button', { name: '7 days', exact: true }).getAttribute('aria-pressed'), 'true')
  assert.equal(await page.getByLabel('Breakdown').inputValue(), 'app')
  assert.equal(await page.getByLabel('Route', { exact: true }).inputValue(), 'all')
  assert.equal(await page.getByLabel('App or group', { exact: true }).inputValue(), 'all')

  const refreshPage = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' })
  const softRefreshUrl = new URL(url.href)
  softRefreshUrl.searchParams.set('fixture', 'refresh-soft-failure')
  await refreshPage.goto(softRefreshUrl.href, { waitUntil: 'domcontentloaded' })
  await refreshPage.getByRole('navigation').getByRole('button', { name: 'Nodes', exact: true }).click()
  const falemonCard = refreshPage.locator('.service-card').first()
  await falemonCard.getByRole('button', { name: 'Refresh', exact: true }).click()
  await refreshPage.waitForFunction(() => (window.__clashforkMockCalls?.refresh_service ?? 0) === 1)
  await refreshPage.getByText('Singapore I0', { exact: true }).first().waitFor()
  assert.equal(await refreshPage.locator('.service-error').count(), 0, 'transient refresh failure must preserve usable nodes without an alarming error')
  if (screenshotDirectory) {
    await refreshPage.screenshot({ path: join(screenshotDirectory, 'refresh-soft-failure.png'), fullPage: true })
  }

  const missingPage = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' })
  const missingUrl = new URL(url.href)
  missingUrl.searchParams.set('fixture', 'actionable-missing')
  await missingPage.goto(missingUrl.href, { waitUntil: 'domcontentloaded' })
  await missingPage.getByRole('navigation').getByRole('button', { name: 'Nodes', exact: true }).click()
  await missingPage.getByText('Missing · choose replacement', { exact: true }).waitFor()
  assert.match(await missingPage.locator('.service-error').first().innerText(), /disappeared|replacement/i, 'confirmed missing selection must retain an actionable warning')
  await missingPage.getByRole('navigation').getByRole('button', { name: 'Rulesets', exact: true }).click()
  const missingRule = missingPage.locator('.unified-rule').first()
  const missingDestination = missingRule.getByRole('combobox', { name: 'Destination for OpenAI', exact: true })
  assert.equal(await missingRule.locator('.unified-rule-expand').getAttribute('aria-expanded'), 'false')
  assert.equal(await missingDestination.inputValue(), 'node:node_falemon_sg', 'a selected missing node remains selected while the rule is collapsed')
  assert.equal(await missingDestination.locator('option:disabled').filter({ hasText: 'Missing' }).count(), 1, 'a selected missing node remains available as a disabled reference')
  assert.equal(await missingRule.locator('.selected-node-latency').count(), 0, 'a selected node without a measurement must not reserve an empty latency line')
  if (screenshotDirectory) {
    await missingPage.screenshot({ path: join(screenshotDirectory, 'nodes-actionable-missing.png'), fullPage: true })
  }
  await refreshPage.close()
  await missingPage.close()

  const staleLatencyPage = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' })
  const staleLatencyUrl = new URL(url.href)
  staleLatencyUrl.searchParams.set('fixture', 'latency-stale')
  await staleLatencyPage.goto(staleLatencyUrl.href, { waitUntil: 'domcontentloaded' })
  await staleLatencyPage.getByRole('navigation').getByRole('button', { name: 'Nodes', exact: true }).click()
  const staleRow = staleLatencyPage.locator('[data-node-id="node_falemon_sg"]')
  await staleRow.locator('del[data-latency-ms="142"]').waitFor()
  assert.equal(await staleRow.locator('del[data-latency-ms="142"]').innerText(), '142 ms', 'latency becomes struck through at the 15 minute threshold')
  await staleLatencyPage.getByRole('navigation').getByRole('button', { name: 'Dashboard', exact: true }).click()
  await staleLatencyPage.getByRole('button', { name: 'Global', exact: true }).click()
  await staleLatencyPage.locator('.picker-field .selected-node-latency del[data-latency-ms="142"]').waitFor()
  if (screenshotDirectory) await staleLatencyPage.screenshot({ path: join(screenshotDirectory, 'stale-latency-picker.png'), fullPage: true })

  const globalModalPage = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' })
  const globalModalUrl = new URL(url.href)
  globalModalUrl.searchParams.set('fixture', 'global-unselected')
  await globalModalPage.goto(globalModalUrl.href, { waitUntil: 'domcontentloaded' })
  await globalModalPage.getByRole('button', { name: 'Global', exact: true }).click()
  const globalDialog = globalModalPage.getByRole('dialog', { name: 'Choose a Global node' })
  await globalDialog.waitFor()
  await globalDialog.getByRole('combobox', { name: 'Node', exact: true }).selectOption('node_falemon_sg')
  assert.equal(await globalDialog.locator('.selected-node-latency').count(), 0, 'an untested Global node must not leave an empty latency line in the dialog')

  const latencyFailurePage = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' })
  const latencyFailureUrl = new URL(url.href)
  latencyFailureUrl.searchParams.set('fixture', 'latency-failure')
  await latencyFailurePage.goto(latencyFailureUrl.href, { waitUntil: 'domcontentloaded' })
  await latencyFailurePage.getByRole('switch', { name: 'VPN connection' }).click()
  await latencyFailurePage.getByRole('heading', { name: 'Connected', exact: true }).waitFor()
  await latencyFailurePage.getByRole('navigation').getByRole('button', { name: 'Nodes', exact: true }).click()
  const failedNodeRow = latencyFailurePage.locator('[data-node-id="node_falemon_sg"]')
  const failedLatency = failedNodeRow.getByRole('button', { name: 'Test Singapore I0 latency', exact: true })
  assert.equal((await failedLatency.innerText()).trim(), '—', 'the unmeasured value is the test button')
  await failedLatency.click()
  await failedLatency.getByText('Timeout', { exact: true }).waitFor()
  assert.equal(await failedNodeRow.locator('.latency-value[data-latency-ms="0"]').count(), 0, 'failed test must not be rendered as zero latency')
  await latencyFailurePage.getByRole('button', { name: 'Test all nodes', exact: true }).click()
  await latencyFailurePage.getByText('4 of 5 responded', { exact: true }).waitFor()
  await latencyFailurePage.locator('[data-node-id="node_northwind_osaka"]').getByRole('button', { name: 'Test Osaka 01 latency', exact: true }).getByText('Timeout', { exact: true }).waitFor()
  await staleLatencyPage.close()
  await globalModalPage.close()
  await latencyFailurePage.close()

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
          const chart = document.querySelector('.traffic-chart')
          const line = document.querySelector('.chart-download')?.getAttribute('d') ?? ''
          const ratePoints = Number(chart?.getAttribute('data-rate-points') ?? 0)
          const firstX = Number(chart?.getAttribute('data-rate-first-x') ?? 0)
          const lastX = Number(chart?.getAttribute('data-rate-last-x') ?? 0)
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
            sampleXs: ratePoints > 0 ? (ratePoints > 1 ? [firstX, lastX] : [firstX]) : [],
            hasArea: Boolean(document.querySelector('.chart-area')),
            hasSmoothCurve: line.includes('C '),
            rateScale: Number(chart?.getAttribute('data-rate-scale') ?? 0),
            rateGroups: Number(chart?.getAttribute('data-rate-groups') ?? 0),
            markerXs: [...document.querySelectorAll('.chart-point')].map((point) => Number(point.getAttribute('cx'))),
          }
        })
        assert.ok(cards.vpnTop < cards.trafficTop && cards.trafficTop < cards.presetsTop, `Dashboard must stack VPN, live traffic, and saved configurations at ${viewport.name}`)
        assert.ok(Math.abs(cards.vpnWidth - cards.trafficWidth) < 4, `Dashboard sections must use the same full-width pane at ${viewport.name}`)
        assert.ok(cards.vpnControlsTop >= cards.vpnHeadingBottom, `VPN mode and destination must sit below status and toggle at ${viewport.name}`)
        assert.ok(cards.vpnToggleRight <= cards.vpnRight - 12, `VPN toggle must stay inside the status header at ${viewport.name}`)
        if (viewport.width >= 1600) assert.ok(cards.destinationWidth <= 980, `Wide Dashboard destination field must remain bounded at ${viewport.name}`)
        assert.ok(cards.rateScale >= 1024 * 1024, `Live rate scale must keep a readable minimum at ${viewport.name}`)
        assert.ok(cards.rateGroups >= 0, `Live rate chart must expose sampled time groups at ${viewport.name}`)
        if (cards.sampleXs.length < 2) {
          assert.equal(cards.hasArea, false, `A sparse live sample must not fill the entire chart at ${viewport.name}`)
          assert.equal(cards.markerXs.length, cards.sampleXs.length * 2, `Sparse live samples must remain visible as points for both directions at ${viewport.name}`)
          if (cards.markerXs.length > 0) assert.ok(cards.markerXs.every((x) => x >= 790), `Sparse live samples must appear at the recent edge at ${viewport.name}`)
        } else {
          assert.equal(cards.hasArea, true, `Multiple live samples should draw a chart area at ${viewport.name}`)
          assert.equal(cards.hasSmoothCurve, true, `Multiple live samples must use smooth curve segments at ${viewport.name}`)
          assert.ok(cards.sampleXs[0] > 0 && cards.sampleXs.at(-1) >= 790, `Live samples must follow their recent time range at ${viewport.name}`)
        }
        if (viewport.width >= 1920) assert.ok(cards.presetsBottom < viewport.height, `Dashboard run configurations should fit in the first viewport at ${viewport.name}`)
      }

      if (screen === 'Rulesets') {
        const panel = page.locator('.unified-rules-panel')
        await panel.waitFor()
        const newRuleset = page.getByRole('button', { name: 'New ruleset', exact: true })
        assert.ok(await newRuleset.isVisible(), `New ruleset must remain available at ${viewport.name}`)
        const headerLayout = await page.evaluate(() => {
          const title = document.querySelector('.rulesets-page > .page-header h1').getBoundingClientRect()
          const action = document.querySelector('.rulesets-page > .page-header .page-actions').getBoundingClientRect()
          return { titleTop: title.top, titleBottom: title.bottom, actionTop: action.top, actionBottom: action.bottom, actionRight: action.right }
        })
        assert.ok(Math.abs(headerLayout.titleTop - headerLayout.actionTop) <= 8, `Rulesets title and action should share a row at ${viewport.name}`)
        assert.ok(headerLayout.actionRight <= viewport.width, `Rulesets header action must fit at ${viewport.name}`)
        assert.equal(await page.getByText('Final catch-all · always last', { exact: true }).count(), 0, `catch-all helper text must stay removed at ${viewport.name}`)
        assert.equal(await page.getByText('First matching rule wins · OR within a rule', { exact: true }).count(), 0, `duplicate matching helper text must stay removed at ${viewport.name}`)
        assert.equal(await page.getByLabel('Copy conditions').count(), 0, `condition copy picker must stay removed at ${viewport.name}`)
        assert.equal(await page.locator('.unified-rule-conditions').count(), 0, `named rule headers must not show condition summaries at ${viewport.name}`)
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `Rulesets must not overflow at ${viewport.name}`)
        const metrics = await page.evaluate(() => {
          const panel = document.querySelector('.unified-rules-panel').getBoundingClientRect()
          const row = document.querySelector('.unified-rule-summary')?.getBoundingClientRect()
          return { panelRight: panel.right, rowRight: row?.right ?? panel.right, rowHeight: row?.height ?? 0 }
        })
        assert.ok(metrics.rowHeight === 0 || metrics.rowHeight <= 60, `Collapsed rules must stay compact at ${viewport.name}: ${JSON.stringify(metrics)}`)
        assert.ok(metrics.rowRight <= metrics.panelRight, `Rules must stay inside the panel at ${viewport.name}`)
        if (screenshotDirectory && ['800x640', '1920x1000'].includes(viewport.name)) {
          await page.screenshot({ path: join(screenshotDirectory, `rulesets-${viewport.name}.png`), fullPage: true })
        }
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
  await open('Logs')
  await page.getByRole('tab', { name: 'Traffic History', exact: true }).click()
  const clearTrigger = page.getByRole('button', { name: 'Clear history', exact: true })
  const clearDialog = page.getByRole('dialog', { name: 'Clear traffic history' })
  const confirmDeletion = clearDialog.getByRole('switch', { name: 'Confirm deletion' })
  const clearHistory = clearDialog.getByRole('button', { name: 'Clear history', exact: true })
  await clearTrigger.click()
  assert.equal(await confirmDeletion.getAttribute('aria-checked'), 'false')
  assert.equal(await clearHistory.isDisabled(), true)
  await clearHistory.evaluate((button) => button.click())
  assert.equal(await mockCalls('clear_history'), 0)
  await confirmDeletion.press('Space')
  assert.equal(await clearHistory.isEnabled(), true)
  await confirmDeletion.press('Space')
  assert.equal(await clearHistory.isDisabled(), true)
  await confirmDeletion.press('Space')
  await page.keyboard.press('Escape')
  await clearDialog.waitFor({ state: 'hidden' })
  await clearTrigger.click()
  assert.equal(await clearHistory.isDisabled(), true)
  assert.equal(await mockCalls('clear_history'), 0)
  await confirmDeletion.press('Space')
  const historyReadsBeforeClear = await mockCalls('get_history')
  await clearHistory.press('Enter')
  await clearDialog.waitFor({ state: 'hidden' })
  await waitForMockCalls('get_history', historyReadsBeforeClear + 1)
  assert.equal(await mockCalls('clear_history'), 1)
  await clearTrigger.click()
  assert.equal(await confirmDeletion.getAttribute('aria-checked'), 'false')
  await clearDialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  await clearDialog.waitFor({ state: 'hidden' })

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
  assert.equal(await freshPage.getByRole('combobox', { name: 'Ruleset', exact: true }).locator('option').filter({ hasText: 'No rulesets yet' }).count(), 1)
  await freshPage.getByRole('button', { name: 'New ruleset', exact: true }).first().click()
  const rulesetName = freshPage.getByRole('textbox', { name: 'Ruleset name', exact: true })
  await rulesetName.fill('Fresh')
  const miscellaneous = freshPage.getByRole('combobox', { name: 'Everything else destination', exact: true })
  assert.equal(await miscellaneous.inputValue(), 'direct')
  await miscellaneous.selectOption('block')
  await miscellaneous.selectOption('node:node_falemon_sg')
  assert.equal(await miscellaneous.inputValue(), 'node:node_falemon_sg', 'fallback selector offers actual local nodes')
  await freshPage.getByRole('button', { name: 'Add rule', exact: true }).click()
  await freshPage.getByRole('textbox', { name: 'Rule name', exact: true }).fill('Fresh rule')
  await freshPage.getByRole('textbox', { name: 'Condition 1 value', exact: true }).fill('fresh.example')
  await freshPage.getByRole('button', { name: 'Save', exact: true }).click()
  await freshPage.waitForFunction(() => [...document.querySelectorAll('.unified-rules-select select option')].some(option => option.textContent?.startsWith('Fresh')))
  const rulesetSelector = freshPage.getByRole('combobox', { name: 'Ruleset', exact: true })
  const freshRulesetValue = await rulesetSelector.locator('option').filter({ hasText: 'Fresh' }).getAttribute('value')
  assert.ok(freshRulesetValue)
  await freshPage.getByRole('button', { name: 'Undo', exact: true }).click()
  assert.equal(await rulesetName.inputValue(), 'Fresh')
  await freshPage.getByRole('button', { name: 'Import JSON', exact: true }).click()
  await freshPage.locator('input[type=file]').setInputFiles({ name: 'rule.json', mimeType: 'application/json', buffer: Buffer.from(JSON.stringify({format:'clashfork-routing',version:1,type:'rule',rule:{name:'Imported',conditions:[{kind:'domain_suffix',value:'import.test'}],destination:{kind:'direct'}}})) })
  const importDialog = freshPage.getByRole('dialog', { name: 'Import rule' })
  await importDialog.waitFor()
  await importDialog.getByRole('button', { name: 'Cancel import', exact: true }).click()
  assert.equal(await rulesetName.inputValue(), 'Fresh', 'cancel keeps current draft unchanged')
  await freshPage.getByRole('button', { name: 'Import JSON', exact: true }).click()
  await freshPage.locator('input[type=file]').setInputFiles({ name: 'legacy-actions.json', mimeType: 'application/json', buffer: Buffer.from(JSON.stringify({format:'clashfork-routing',version:1,type:'ruleset',name:'Legacy actions',miscellaneous:'block',rules:[{name:'Direct rule',conditions:[{kind:'domain_suffix',value:'legacy.example'}],destination:{kind:'direct'}}]})) })
  const legacyImport = freshPage.getByRole('dialog', { name: 'Import ruleset' })
  await legacyImport.waitFor()
  await legacyImport.getByText('Save the draft to apply it.', { exact: true }).waitFor()
  assert.equal(await legacyImport.getByText(/Choose local nodes for the proxy destinations/).count(), 0, 'imports without proxies omit mapping instructions')
  assert.equal(await legacyImport.getByRole('button', { name: 'Use imported draft', exact: true }).isDisabled(), false, 'legacy Direct and Block routing documents remain importable without a proxy map')
  await legacyImport.getByRole('button', { name: 'Use imported draft', exact: true }).click()
  assert.equal(await freshPage.getByRole('combobox', { name: 'Everything else destination', exact: true }).inputValue(), 'block', 'legacy catch-all action remains unchanged')
  await freshPage.getByRole('navigation').getByRole('button', { name: 'Dashboard', exact: true }).click()
  await freshPage.getByRole('heading', { name: 'Dashboard', exact: true }).waitFor()
  await freshPage.waitForFunction(() => [...document.querySelectorAll('.picker-field select[aria-label="Ruleset"] option')].some(option => option.textContent === 'Fresh'))
  await freshPage.getByRole('navigation').getByRole('button', { name: 'Rulesets', exact: true }).click()
  await freshPage.getByRole('button', { name: 'New ruleset', exact: true }).first().click()
  await freshPage.getByRole('button', { name: 'Save', exact: true }).click()
  await freshPage.getByRole('alert').getByText('Name the ruleset before saving.', { exact: true }).waitFor()
  assert.deepEqual(freshErrors, [])
  assert.deepEqual(freshConsoleIssues, [])
  if (screenshotDirectory) {
    await freshPage.waitForTimeout(300)
    await freshPage.screenshot({ path: join(screenshotDirectory, 'ruleset-errors.png'), fullPage: true })
  }
  console.log('Fresh ruleset fixture passed: empty setup, Direct default, Block and node fallback selection, Dashboard selection, duplicate/empty validation with draft retention.')

  const advisoryPage = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' })
  advisoryPage.setDefaultTimeout(25000)
  const advisoryUrl = new URL(url.href)
  advisoryUrl.searchParams.set('fixture', 'competing-tunnel')
  await advisoryPage.goto(advisoryUrl.href, { waitUntil: 'domcontentloaded' })
  await advisoryPage.getByRole('heading', { name: 'Dashboard', exact: true }).waitFor()
  await advisoryPage.getByRole('status').filter({ hasText: 'Another active tunnel adapter currently owns the selected outbound route' }).waitFor()
  assert.equal(await advisoryPage.getByRole('status').filter({ hasText: 'Clashfork has not changed system routing' }).count(), 1)
  if (screenshotDirectory) await advisoryPage.screenshot({ path: join(screenshotDirectory, 'tunnel-advisory.png'), fullPage: true })
  console.log('Tunnel-conflict advisory fixture passed: actionable warning is visible and explicitly read-only.')
  if (screenshotDirectory) console.log(screenshotDirectory)
} finally {
  await browser.close()
}
