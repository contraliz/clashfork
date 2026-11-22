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
  assert.match(await page.title(), /Clashfork/)
  assert.match(await page.locator('#root').innerText(), /Dashboard/)
  assert.equal(await page.getByText(/Vite|Webpack|Next\.js.*error/i).count(), 0)
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
  await page.getByRole('table').waitFor()
  await page.getByRole('cell', { name: 'ChatGPT.exe', exact: true }).first().waitFor()
  await capture('live')
  await open('Logs')
  await page.getByRole('tab', { name: 'Traffic History', exact: true }).waitFor()
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
  await capture('logs')
  await page.getByRole('tab', { name: 'Diagnostics', exact: true }).click()
  await page.getByText('Live diagnostics', { exact: true }).waitFor()

  await open('Dashboard')
  for (const screen of ['Dashboard', 'Nodes', 'Rulesets', 'Live Connections', 'Logs']) {
    await open(screen)
    await page.setViewportSize({ width: 800, height: 640 })
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `${screen} overflows at 800x640`)
    if (screen === 'Dashboard' && screenshotDirectory) {
      await page.screenshot({ path: join(screenshotDirectory, 'dashboard-800.png') })
    }
    await page.setViewportSize({ width: 1280, height: 800 })
  }
  assert.deepEqual(errors, [])
  assert.deepEqual(consoleIssues, [])
  console.log('Browser fixture passed on all five screens at 1280x800 and 800x640, including ruleset selection while connected.')

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
