// Uses a fresh browser profile and creates/deletes only its own newly provisioned fixture.
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { randomUUID } from 'node:crypto'
import { chromium } from '@playwright/test'

const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
const credentials = JSON.parse(await readFile(new URL('organisation-invitations-fixture.json', privateDir), 'utf8'))
const browser = await chromium.launch({ channel: 'msedge', headless: true })
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
  const errors = []
  page.on('pageerror', error => errors.push(error.message))
  await page.goto('http://localhost:18082/server')
  await page.getByLabel('Email', { exact: true }).fill(credentials.email)
  await page.getByLabel('Password', { exact: true }).fill(credentials.password)
  await page.locator('button[type=submit]').filter({ hasText: 'Sign in' }).click()
  await page.getByRole('heading', { name: 'Enterprise management', exact: true }).waitFor()
  await page.locator('.enterprise-management > section select').first().selectOption(credentials.organisation_id)
  const name = `Lösch-UI-Abnahme ${randomUUID()}`
  await page.getByLabel('Instance name', { exact: true }).fill(name)
  const created = page.waitForResponse(r => r.request().method() === 'POST' && r.url().endsWith('/instances')).then(r => r.json())
  await page.getByRole('button', { name: 'Create instance', exact: true }).click()
  const instance = await created
  assert.match(instance.id, /^[a-f0-9-]{36}$/)
  await writeFile(new URL('deletion-ui-fixture.json', privateDir), JSON.stringify({ instance_id: instance.id, organisation_id: credentials.organisation_id, name }, null, 2))
  const card = page.locator('article').filter({ has: page.getByRole('heading', { name, exact: true }) })
  await card.getByRole('button', { name: 'Delete', exact: true }).click()
  const confirmation = page.getByRole('button', { name: 'Confirm operation', exact: true })
  assert.equal(await confirmation.isEnabled(), false)
  await page.getByLabel('Type the instance name', { exact: true }).fill(name)
  assert.equal(await confirmation.isEnabled(), false)
  await page.getByLabel('Permanently delete all managed backups as well.', { exact: true }).check()
  await page.getByLabel('Type the instance name', { exact: true }).fill(name + ' wrong')
  assert.equal(await confirmation.isEnabled(), false)
  await page.getByLabel('Type the instance name', { exact: true }).fill(name)
  assert.equal(await confirmation.isEnabled(), true)
  await page.screenshot({ path: fileURLToPath(new URL('deletion-confirmation.png', privateDir)), fullPage: true })
  await confirmation.click()
  await card.waitFor({ state: 'detached', timeout: 60000 })
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await page.getByText('No instances yet.', { exact: true }).waitFor()
  assert.equal(await page.getByRole('alert').count(), 0)
  assert.deepEqual(errors, [])
  await writeFile(new URL('deletion-ui-results.json', privateDir), JSON.stringify({ verified_at: new Date().toISOString(), instance_id: instance.id, tests: ['ordinary_admin_browser_login', 'exact_name_and_backup_confirmation', 'newly_provisioned_fixture_deleted', 'no_stale_history_error'], page_errors: errors }, null, 2))
  console.log('deletion_browser_confirmation_and_provisioned_fixture_cleanup=ok')
} finally {
  await browser.close()
}
