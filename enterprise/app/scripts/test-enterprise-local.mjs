import { nextManagementSecondFactor } from './enterprise-test-auth.mjs'
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { randomBytes, randomUUID } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { chromium } from '@playwright/test'

const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
const secret = async name => (await readFile(new URL(name, privateDir), 'utf8')).trim()
const management = 'http://localhost:18082'
const workspace = 'http://127.0.0.1:18083'
const results = []
const passed = name => { results.push(name); console.log(`${name}=ok`) }
const login = async (page, kind, native = false) => {
  if (native) await page.getByLabel('Server URL', { exact: true }).fill(kind === 'management' ? management : workspace)
  await page.getByLabel('Email', { exact: true }).fill('admin@cowork.local')
  await page.getByLabel('Password', { exact: true }).fill(await secret(`${kind}_admin_password`))
  if (kind === 'management') await page.getByLabel('Authenticator or recovery code', { exact: true }).fill(nextManagementSecondFactor())
  await page.locator('button[type=submit]').filter({ hasText: 'Sign in' }).click()
  if (kind === 'management') await page.getByRole('heading', { name: 'Enterprise management', exact: true }).waitFor()
  else await page.getByRole('heading', { name: 'Runs', exact: true }).waitFor()
}

const browser = await chromium.launch({ channel: 'msedge', headless: true })
try {
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } })
  const page = await context.newPage()
  const errors = []
  page.on('pageerror', error => errors.push(error.message))
  await page.goto(`${management}/server`)
  await login(page, 'management')
  assert.equal(new URL(page.url()).pathname, '/management')
  const organisations = page.locator('.enterprise-management > section select').first()
  const alphaId = await organisations.locator('option').filter({ hasText: 'Testorganisation Alpha' }).getAttribute('value')
  await organisations.selectOption('')
  const instanceResponse = page.waitForResponse(value => value.url().endsWith(`/organisations/${alphaId}/instances`) && value.status() === 200).then(value => value.json())
  await organisations.selectOption({ label: 'Testorganisation Alpha' })
  const existingInstances = await instanceResponse
  const name = 'Docker-Testinstanz Alpha'
  if (!existingInstances.some(value => value.name === name)) {
    await page.getByLabel('Instance name', { exact: true }).fill(name)
    await page.getByRole('button', { name: 'Create instance', exact: true }).click()
    await page.getByRole('heading', { name, exact: true }).waitFor()
  }
  await page.getByRole('heading', { name, exact: true }).waitFor()
  assert.equal(await page.getByText('An execution backend is not yet available for this profile.', { exact: true }).count(), 0)
  await page.locator('.enterprise-management > section select').first().selectOption({ label: 'Testorganisation Beta' })
  await page.getByText('No instances yet.', { exact: true }).waitFor()
  assert.equal(await page.getByRole('heading', { name, exact: true }).count(), 0)
  await page.locator('.enterprise-management > section select').first().selectOption({ label: 'Testorganisation Alpha' })
  await page.getByRole('heading', { name, exact: true }).waitFor()
  await page.getByRole('button', { name: 'Audit', exact: true }).click()
  await page.getByRole('table').waitFor()
  passed('web_management_login_organisation_switch_audit')
  const cookies = await context.cookies(management)
  const refresh = cookies.find(cookie => cookie.name === '__Host-cowork_refresh')
  assert.ok(refresh?.httpOnly && refresh.secure && refresh.sameSite === 'Strict')
  assert.ok(!(await page.evaluate(() => JSON.stringify(localStorage))).includes(refresh.value))
  await page.reload()
  await page.getByRole('heading', { name: 'Enterprise management', exact: true }).waitFor()
  await page.locator('.enterprise-management > section select').first().selectOption({ label: 'Testorganisation Alpha' })
  await page.getByRole('heading', { name, exact: true }).waitFor()
  await page.screenshot({ path: fileURLToPath(new URL('management.png', privateDir)), fullPage: true })
  passed('web_cookie_session_reload')
  await page.getByRole('button', { name: 'Sign out / switch server', exact: true }).click()
  await page.getByRole('heading', { name: 'Connect to Open Cowork Server', exact: true }).waitFor()
  await page.goto(`${workspace}/server`)
  await login(page, 'workspace')
  passed('web_workspace_login')
  assert.deepEqual(errors, [])

  for (const origin of ['http://tauri.localhost', 'tauri://localhost', 'http://127.0.0.1:5173']) {
    const response = await fetch(`${management}/api/v1/auth/native/authorize`, { method: 'OPTIONS', headers: { Origin: origin, 'Access-Control-Request-Method': 'POST', 'Access-Control-Request-Headers': 'authorization,content-type' } })
    assert.equal(response.headers.get('access-control-allow-origin'), origin)
    assert.equal(response.headers.get('access-control-allow-credentials'), null)
  }
  const blocked = await fetch(`${management}/api/v1/auth/native/authorize`, { method: 'OPTIONS', headers: { Origin: 'https://untrusted.example', 'Access-Control-Request-Method': 'POST' } })
  assert.equal(blocked.headers.get('access-control-allow-origin'), null)
  assert.equal((await fetch(`${management}/management/runner/v1/claim`, { method: 'POST' })).status, 404)
  passed('desktop_cors_allowlist_and_private_route_boundary')

  const request = async (origin, path, token, body, status = 200) => {
    const response = await fetch(`${origin}${path}`, { method: body === undefined ? 'GET' : 'POST', headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) }, body: body === undefined ? undefined : JSON.stringify(body) })
    assert.equal(response.status, status, `${path}: unexpected status`)
    return response.json().catch(() => null)
  }
  const admin = await request(management, '/api/v1/auth/login', null, { email: 'admin@cowork.local', password: await secret('management_admin_password'), device_id: randomUUID(), second_factor: nextManagementSecondFactor() })
  const orgs = (await request(management, '/management/v1/organisations', admin.access_token)).items
  const alpha = orgs.find(org => org.name === 'Testorganisation Alpha').id
  const beta = orgs.find(org => org.name === 'Testorganisation Beta').id
  const passwordPath = new URL('test_member_password', privateDir)
  try { await writeFile(passwordPath, randomBytes(32).toString('hex'), { flag: 'wx' }) } catch (error) { if (error.code !== 'EEXIST') throw error }
  const credentials = { email: 'member-alpha@cowork.local', password: await secret('test_member_password'), device_id: randomUUID() }
  let response = await fetch(`${management}/api/v1/auth/login`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(credentials) })
  let member
  if (response.status === 401) {
    const invitation = await request(management, '/api/v1/auth/invitations', admin.access_token, { email: credentials.email }, 201)
    member = await request(management, '/api/v1/auth/invitations/accept', null, { token: invitation.token, display_name: 'Testmitglied Alpha', password: credentials.password, device_id: credentials.device_id }, 201)
  } else { assert.equal(response.status, 200); member = await response.json() }
  await request(management, `/management/v1/organisations/${alpha}/members`, admin.access_token, { email: credentials.email, role: 'member' })
  assert.deepEqual((await request(management, '/management/v1/organisations', member.access_token)).items.map(org => org.id), [alpha])
  await request(management, `/management/v1/organisations/${beta}/instances`, member.access_token, undefined, 404)
  await request(management, `/management/v1/organisations/${alpha}/audit`, member.access_token, undefined, 404)
  await request(management, '/management/v1/organisations', member.access_token, { name: 'Forbidden' }, 404)
  await request(workspace, '/api/v1/runs', admin.access_token, undefined, 401)
  const instance = (await request(management, `/management/v1/organisations/${alpha}/instances`, admin.access_token)).find(value => value.name === name)
  assert.ok(Array.isArray(instance.available_actions))
  assert.ok(!instance.available_actions.includes('backup'))
  await request(management, '/api/v1/auth/logout', member.access_token, {}, 204)
  await request(management, '/api/v1/auth/logout', admin.access_token, {}, 204)
  passed('organisation_role_token_isolation_and_qualified_actions')

  if (process.argv.includes('--desktop')) {
    const desktop = await chromium.connectOverCDP('http://127.0.0.1:9223')
    try {
      const app = desktop.contexts()[0].pages().find(value => value.url().startsWith('http://127.0.0.1:5173'))
      assert.ok(app, 'Open the development desktop app first')
      await app.goto('http://127.0.0.1:5173/server')
      await app.locator('.remote-login-card, .enterprise-management, .remote-server-header').first().waitFor()
      const signOut = app.getByRole('button', { name: /^(Sign out|Sign out \/ switch server)$/ }).first()
      if (await signOut.isVisible()) await signOut.click()
      await app.getByRole('heading', { name: 'Connect to Open Cowork Server', exact: true }).waitFor()
      await login(app, 'management', true)
      await app.getByRole('button', { name: 'Sign out / switch server', exact: true }).click()
      await login(app, 'workspace', true)
      await app.getByRole('button', { name: 'Sign out', exact: true }).click()
      const connections = app.locator('.server-profiles select')
      const options = await connections.locator('option').allTextContents()
      assert.ok(options.some(value => value.includes('18082')))
      assert.ok(options.some(value => value.includes('18083')))
      await connections.selectOption({ label: options.find(value => value.includes('18082')) })
      assert.equal(await app.getByLabel('Server URL', { exact: true }).inputValue(), management)
      await login(app, 'management', true)
      await app.reload()
      await app.getByRole('heading', { name: 'Enterprise management', exact: true }).waitFor()
      await app.locator('.enterprise-management > section select').first().selectOption({ label: 'Testorganisation Alpha' })
      await app.getByRole('heading', { name, exact: true }).waitFor()
      await app.screenshot({ path: fileURLToPath(new URL('desktop-management.png', privateDir)) })
      passed('native_desktop_two_servers_saved_profiles_vault_restore')
    } finally { await desktop.close() }
  }
  await writeFile(new URL('test-results.json', privateDir), JSON.stringify({ testedAt: new Date().toISOString(), passed: results }, null, 2))
} finally { await browser.close() }
