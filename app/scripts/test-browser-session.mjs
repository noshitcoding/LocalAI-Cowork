import { createReadStream, existsSync, readFileSync, statSync } from 'node:fs'
import { randomUUID } from 'node:crypto'
import { createServer, request as httpsRequest } from 'node:https'
import { request as httpRequest } from 'node:http'
import { extname, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium, expect } from '@playwright/test'

const apiOrigin = new URL(process.env.COWORK_TEST_API_URL ?? 'http://127.0.0.1:18097')
const webOrigin = process.env.COWORK_TEST_WEB_ORIGIN ?? 'https://127.0.0.1:18447'
const distRoot = resolve(process.env.COWORK_TEST_WEB_DIST ?? fileURLToPath(new URL('../dist/', import.meta.url)))
const certificate = process.env.COWORK_TEST_TLS_CERT
const privateKey = process.env.COWORK_TEST_TLS_KEY
const email = process.env.COWORK_TEST_EMAIL
const password = process.env.COWORK_TEST_PASSWORD
const refreshRotations = Number.parseInt(process.env.COWORK_TEST_REFRESH_ROTATIONS ?? '25', 10)

if (!certificate || !privateKey || !email || !password) {
  throw new Error('browser-session E2E requires TLS certificate, key, email, and password')
}
if (!Number.isInteger(refreshRotations) || refreshRotations < 2 || refreshRotations > 250) {
  throw new Error('COWORK_TEST_REFRESH_ROTATIONS must be an integer from 2 through 250')
}

const contentTypes = new Map([
  ['.css', 'text/css; charset=utf-8'],
  ['.html', 'text/html; charset=utf-8'],
  ['.ico', 'image/x-icon'],
  ['.js', 'text/javascript; charset=utf-8'],
  ['.json', 'application/json; charset=utf-8'],
  ['.png', 'image/png'],
  ['.svg', 'image/svg+xml'],
  ['.webmanifest', 'application/manifest+json'],
  ['.woff2', 'font/woff2'],
])

function proxyApi(incoming, outgoing) {
  const headers = { ...incoming.headers, host: apiOrigin.host }
  delete headers.connection
  const upstream = httpRequest(new URL(incoming.url, apiOrigin), {
    method: incoming.method,
    headers,
  }, (response) => {
    outgoing.writeHead(response.statusCode ?? 502, response.headers)
    response.pipe(outgoing)
  })
  upstream.on('error', (error) => {
    outgoing.writeHead(502, { 'content-type': 'text/plain; charset=utf-8' })
    outgoing.end(`control-plane proxy failed: ${error.message}`)
  })
  incoming.pipe(upstream)
}

function serveWeb(incoming, outgoing) {
  const pathname = decodeURIComponent(new URL(incoming.url, webOrigin).pathname)
  const requested = resolve(distRoot, `.${pathname}`)
  const withinDist = requested === distRoot || requested.startsWith(`${distRoot}${sep}`)
  const file = withinDist && existsSync(requested) && statSync(requested).isFile()
    ? requested
    : resolve(distRoot, 'index.html')
  outgoing.writeHead(200, {
    'content-type': contentTypes.get(extname(file)) ?? 'application/octet-stream',
    'cache-control': 'no-store',
  })
  createReadStream(file).pipe(outgoing)
}

const server = createServer({
  cert: readFileSync(certificate),
  key: readFileSync(privateKey),
}, (incoming, outgoing) => {
  if (incoming.url?.startsWith('/api/')) proxyApi(incoming, outgoing)
  else serveWeb(incoming, outgoing)
})

function listen() {
  const url = new URL(webOrigin)
  return new Promise((accept, reject) => {
    server.once('error', reject)
    server.listen(Number(url.port), url.hostname, accept)
  })
}

function close() {
  return new Promise((accept) => server.close(accept))
}

function rawRefresh(cookie) {
  const url = new URL('/api/v1/auth/browser/refresh', webOrigin)
  return new Promise((accept, reject) => {
    const request = httpsRequest({
      hostname: url.hostname,
      port: url.port,
      path: url.pathname,
      method: 'POST',
      rejectUnauthorized: false,
      headers: {
        cookie,
        'x-cowork-session-mode': 'browser-cookie',
        'content-length': '0',
      },
    }, (response) => {
      response.resume()
      response.on('end', () => accept(response.statusCode))
    })
    request.on('error', reject)
    request.end()
  })
}

async function api(path, { method = 'GET', token, body } = {}) {
  const response = await fetch(new URL(`/api/v1${path}`, apiOrigin), {
    method,
    headers: {
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...(body === undefined ? {} : { 'content-type': 'application/json' }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const text = await response.text()
  return { response, body: text ? JSON.parse(text) : null }
}

let browser
try {
  await listen()
  browser = await chromium.launch({ headless: true })
  const context = await browser.newContext({ ignoreHTTPSErrors: true })
  const page = await context.newPage()
  const consoleErrors = []
  page.on('pageerror', (error) => consoleErrors.push(error.message))
  page.on('console', (message) => {
    if (message.type() === 'error') consoleErrors.push(message.text())
  })

  await page.goto(`${webOrigin}/server`, { waitUntil: 'networkidle' })
  await expect(page.getByRole('heading', { name: 'Connect to Open Cowork Server' })).toBeVisible()
  const serverInput = page.getByRole('textbox', { name: /server/i })
  await expect(serverInput).toHaveValue(webOrigin)
  await expect(serverInput).toHaveAttribute('readonly', '')
  await page.getByLabel('Email').fill(email)
  await page.getByLabel('Password', { exact: true }).fill(password)
  const tokenResponse = page.waitForResponse((response) => response.url().endsWith('/auth/native/token'))
  await page.getByRole('button', { name: 'Sign in', exact: true }).click()
  const tokenBody = await (await tokenResponse).json()
  if ('refresh_token' in tokenBody) throw new Error('browser token response exposed refresh_token')
  await expect(page.getByRole('button', { name: 'Sign out' })).toBeVisible()

  let cookies = await context.cookies(webOrigin)
  const first = cookies.find((cookie) => cookie.name === '__Host-cowork_refresh')
  if (!first?.httpOnly || !first.secure || first.sameSite !== 'Strict') {
    throw new Error('browser refresh cookie is not Secure, HttpOnly, and SameSite=Strict')
  }
  const storage = await page.evaluate(() => JSON.stringify({ ...localStorage }))
  if (storage.includes(first.value) || storage.includes(tokenBody.access_token)) {
    throw new Error('authentication token leaked into localStorage')
  }

  const secondaryDeviceId = randomUUID()
  const secondaryLogin = await api('/auth/login', {
    method: 'POST',
    body: { email, password, device_id: secondaryDeviceId },
  })
  if (!secondaryLogin.response.ok) throw new Error(`secondary device login returned ${secondaryLogin.response.status}`)
  const outsiderEmail = `browser-session-outsider-${randomUUID()}@opencowork.invalid`
  const outsiderDeviceId = randomUUID()
  const invitation = await api('/auth/invitations', {
    method: 'POST',
    token: tokenBody.access_token,
    body: { email: outsiderEmail },
  })
  if (invitation.response.status !== 201) throw new Error(`outsider invitation returned ${invitation.response.status}`)
  const outsider = await api('/auth/invitations/accept', {
    method: 'POST',
    body: {
      token: invitation.body.token,
      display_name: 'Browser Session Outsider',
      password: 'Browser-Session-Outsider-Password-42!',
      device_id: outsiderDeviceId,
    },
  })
  if (outsider.response.status !== 201) throw new Error(`outsider registration returned ${outsider.response.status}`)
  const beforeRevoke = await api('/auth/sessions', { token: tokenBody.access_token })
  if (!beforeRevoke.response.ok) throw new Error(`session listing returned ${beforeRevoke.response.status}`)
  const current = beforeRevoke.body.find((session) => session.current)
  const secondary = beforeRevoke.body.find((session) => session.device_id === secondaryDeviceId)
  if (!current?.active || !secondary?.active || secondary.current
    || beforeRevoke.body.some((session) => session.device_id === outsiderDeviceId)) {
    throw new Error('session listing did not identify the current and secondary devices')
  }
  const outsiderSessions = await api('/auth/sessions', { token: outsider.body.access_token })
  if (!outsiderSessions.response.ok
    || outsiderSessions.body.length !== 1
    || outsiderSessions.body[0].device_id !== outsiderDeviceId
    || !outsiderSessions.body[0].current) {
    throw new Error('session listing crossed the user ownership boundary')
  }
  const outsiderRevoke = await api(`/auth/sessions/${current.id}`, {
    method: 'DELETE', token: outsider.body.access_token,
  })
  if (outsiderRevoke.response.status !== 404) {
    throw new Error(`cross-user session revocation returned ${outsiderRevoke.response.status}`)
  }
  const adminAfterOutsider = await api('/auth/sessions', { token: tokenBody.access_token })
  if (!adminAfterOutsider.response.ok) throw new Error('cross-user revocation affected the owning session')
  const revoked = await api(`/auth/sessions/${secondary.id}`, {
    method: 'DELETE', token: tokenBody.access_token,
  })
  if (revoked.response.status !== 204) throw new Error(`session revocation returned ${revoked.response.status}`)
  const secondaryAccess = await api('/auth/sessions', { token: secondaryLogin.body.access_token })
  if (secondaryAccess.response.status !== 401) throw new Error('revoked device access token remained usable')
  const afterRevoke = await api('/auth/sessions', { token: tokenBody.access_token })
  const revokedRecord = afterRevoke.body.find((session) => session.id === secondary.id)
  if (revokedRecord?.active || revokedRecord?.revoke_reason !== 'user_device_revoked') {
    throw new Error('revoked session history did not expose the revocation state')
  }

  const refreshValues = new Set([first.value])
  for (let index = 0; index < refreshRotations; index++) {
    await page.reload({ waitUntil: 'networkidle' })
    await expect(page.getByRole('button', { name: 'Sign out' })).toBeVisible()
    cookies = await context.cookies(webOrigin)
    const rotated = cookies.find((cookie) => cookie.name === '__Host-cowork_refresh')
    if (!rotated || refreshValues.has(rotated.value)) {
      throw new Error(`refresh rotation ${index + 1} did not issue a unique cookie`)
    }
    refreshValues.add(rotated.value)
  }
  if (consoleErrors.length) throw new Error(`browser emitted runtime errors before replay test: ${consoleErrors.join(' | ')}`)

  const staleStatus = await rawRefresh(`__Host-cowork_refresh=${first.value}`)
  if (staleStatus !== 401) throw new Error(`stale refresh replay returned ${staleStatus}, expected 401`)
  await page.reload({ waitUntil: 'networkidle' })
  await expect(page.getByRole('heading', { name: 'Connect to Open Cowork Server' })).toBeVisible()
  const expectedReplayErrors = consoleErrors.splice(0)
  if (expectedReplayErrors.length !== 1 || !expectedReplayErrors[0].includes('401 (Unauthorized)')) {
    throw new Error(`unexpected replay failure diagnostics: ${expectedReplayErrors.join(' | ')}`)
  }
  if ((await context.cookies(webOrigin)).some((cookie) => cookie.name === '__Host-cowork_refresh')) {
    throw new Error('revoked browser cookie was not cleared')
  }

  await page.getByLabel('Email').fill(email)
  await page.getByLabel('Password', { exact: true }).fill(password)
  await page.getByRole('button', { name: 'Sign in', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Sign out' })).toBeVisible()
  const logoutResponse = page.waitForResponse((response) => response.url().endsWith('/auth/logout'))
  await page.getByRole('button', { name: 'Sign out' }).click()
  if ((await logoutResponse).status() !== 204) throw new Error('browser logout request failed')
  await expect(page.getByRole('heading', { name: 'Connect to Open Cowork Server' })).toBeVisible()
  if ((await context.cookies(webOrigin)).some((cookie) => cookie.name === '__Host-cowork_refresh')) {
    throw new Error('logout did not remove the browser refresh cookie')
  }
  if (consoleErrors.length) throw new Error(`browser emitted runtime errors: ${consoleErrors.join(' | ')}`)

  console.log('canonical_same_origin=ok')
  console.log('refresh_token_hidden_from_javascript=ok')
  console.log('secure_httponly_samesite_cookie=ok')
  console.log('cross_user_session_isolation=ok')
  console.log('device_session_revocation=ok')
  console.log(`reload_rotations=${refreshRotations}`)
  console.log('refresh_reuse_family_revocation=ok')
  console.log('logout_cookie_clear=ok')
} finally {
  await browser?.close()
  await close().catch(() => undefined)
}
