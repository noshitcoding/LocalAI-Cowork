import { createHash, generateKeyPairSync, randomBytes, sign } from 'node:crypto'
import { createServer } from 'node:http'

const port = Number(process.argv[2] ?? '18092')
const issuer = `http://127.0.0.1:${port}`
const clientId = process.env.MOCK_OIDC_CLIENT_ID ?? 'open-cowork-e2e'
const clientSecret = process.env.MOCK_OIDC_CLIENT_SECRET ?? 'open-cowork-e2e-secret'
const subject = process.env.MOCK_OIDC_SUBJECT ?? 'oidc-e2e-user'
const email = process.env.MOCK_OIDC_EMAIL ?? 'oidc-user@opencowork.invalid'
const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 })
const publicJwk = publicKey.export({ format: 'jwk' })
const codes = new Map()
let authorizationCount = 0

function base64url(value) {
  return Buffer.from(value).toString('base64url')
}

function json(response, status, value) {
  const body = Buffer.from(JSON.stringify(value))
  response.writeHead(status, {
    'content-type': 'application/json',
    'content-length': String(body.length),
    'cache-control': 'no-store',
  })
  response.end(body)
}

function redirect(response, location) {
  response.writeHead(302, { location, 'cache-control': 'no-store' })
  response.end()
}

function jwt(claims) {
  const header = base64url(JSON.stringify({ alg: 'RS256', typ: 'JWT', kid: 'oidc-e2e-key' }))
  const payload = base64url(JSON.stringify(claims))
  const signature = sign('RSA-SHA256', Buffer.from(`${header}.${payload}`), privateKey).toString('base64url')
  return `${header}.${payload}.${signature}`
}

function formBody(request) {
  return new Promise((resolve, reject) => {
    const chunks = []
    request.on('data', (chunk) => chunks.push(chunk))
    request.on('end', () => resolve(new URLSearchParams(Buffer.concat(chunks).toString('utf8'))))
    request.on('error', reject)
  })
}

const server = createServer(async (request, response) => {
  const url = new URL(request.url ?? '/', issuer)
  if (request.method === 'GET' && url.pathname === '/healthz') {
    return json(response, 200, { status: 'ok' })
  }
  if (request.method === 'GET' && url.pathname === '/.well-known/openid-configuration') {
    return json(response, 200, {
      issuer,
      authorization_endpoint: `${issuer}/authorize`,
      token_endpoint: `${issuer}/token`,
      jwks_uri: `${issuer}/jwks`,
      response_types_supported: ['code'],
      subject_types_supported: ['public'],
      id_token_signing_alg_values_supported: ['RS256'],
      scopes_supported: ['openid', 'email', 'profile'],
      claims_supported: ['sub', 'aud', 'exp', 'iat', 'iss', 'nonce', 'email', 'email_verified'],
      token_endpoint_auth_methods_supported: ['client_secret_basic'],
      code_challenge_methods_supported: ['S256'],
    })
  }
  if (request.method === 'GET' && url.pathname === '/jwks') {
    return json(response, 200, { keys: [{ ...publicJwk, use: 'sig', alg: 'RS256', kid: 'oidc-e2e-key' }] })
  }
  if (request.method === 'GET' && url.pathname === '/authorize') {
    const redirectUri = url.searchParams.get('redirect_uri') ?? ''
    const state = url.searchParams.get('state') ?? ''
    const nonce = url.searchParams.get('nonce') ?? ''
    const codeChallenge = url.searchParams.get('code_challenge') ?? ''
    if (url.searchParams.get('client_id') !== clientId
      || url.searchParams.get('response_type') !== 'code'
      || !url.searchParams.get('scope')?.split(' ').includes('openid')
      || url.searchParams.get('code_challenge_method') !== 'S256'
      || !redirectUri || !state || !nonce || !codeChallenge) {
      return json(response, 400, { error: 'invalid_request' })
    }
    const code = randomBytes(32).toString('base64url')
    authorizationCount += 1
    codes.set(code, {
      redirectUri,
      nonce,
      codeChallenge,
      subject: authorizationCount === 1 ? subject : `${subject}-${authorizationCount}`,
      email: authorizationCount === 1 ? email : `oidc-user-${authorizationCount}@opencowork.invalid`,
    })
    const callback = new URL(redirectUri)
    callback.searchParams.set('code', code)
    callback.searchParams.set('state', state)
    return redirect(response, callback.toString())
  }
  if (request.method === 'POST' && url.pathname === '/token') {
    const expectedBasic = `Basic ${Buffer.from(`${clientId}:${clientSecret}`).toString('base64')}`
    const body = await formBody(request)
    const code = body.get('code') ?? ''
    const transaction = codes.get(code)
    const verifier = body.get('code_verifier') ?? ''
    const actualChallenge = createHash('sha256').update(verifier).digest('base64url')
    if (request.headers.authorization !== expectedBasic
      || body.get('grant_type') !== 'authorization_code'
      || body.get('redirect_uri') !== transaction?.redirectUri
      || actualChallenge !== transaction?.codeChallenge) {
      return json(response, 400, { error: 'invalid_grant' })
    }
    codes.delete(code)
    const accessToken = randomBytes(32).toString('base64url')
    const now = Math.floor(Date.now() / 1000)
    const atHash = createHash('sha256').update(accessToken, 'ascii').digest().subarray(0, 16).toString('base64url')
    return json(response, 200, {
      access_token: accessToken,
      token_type: 'Bearer',
      expires_in: 300,
      id_token: jwt({
        iss: issuer,
        sub: transaction.subject,
        aud: clientId,
        exp: now + 300,
        iat: now,
        nonce: transaction.nonce,
        email: transaction.email,
        email_verified: true,
        at_hash: atHash,
      }),
    })
  }
  json(response, 404, { error: 'not_found' })
})

server.listen(port, '127.0.0.1')
