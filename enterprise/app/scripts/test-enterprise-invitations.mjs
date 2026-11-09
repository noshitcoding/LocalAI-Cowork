import { nextManagementSecondFactor } from './enterprise-test-auth.mjs'
// Live invitation acceptance creates only dedicated test organisations and identities.
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { randomBytes, randomUUID } from 'node:crypto'
import http from 'node:http'

const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
const fixtureFile = new URL('organisation-invitations-fixture.json', privateDir)
const management = 'http://localhost:18082'
async function request(origin, route, token, body, expected = 200, method) {
  const url = new URL(route, origin)
  const response = await new Promise((resolve, reject) => {
    const req = http.request({ hostname: '127.0.0.1', port: url.port, path: url.pathname + url.search, method: method ?? (body === undefined ? 'GET' : 'POST'), headers: { Host: url.host, 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) } }, res => {
      let data = ''; res.setEncoding('utf8'); res.on('data', part => { data += part }); res.on('end', () => resolve({ status: res.statusCode, data }))
    }); req.on('error', reject); req.setTimeout(20000, () => req.destroy(new Error('HTTP timeout'))); req.end(body === undefined ? undefined : JSON.stringify(body))
  })
  assert.equal(response.status, expected, `${route}: HTTP ${response.status}`)
  return response.data && expected < 400 ? JSON.parse(response.data) : null
}
const token = (await request(management, '/api/v1/auth/login', null, { email: 'admin@cowork.local', password: (await readFile(new URL('management_admin_password', privateDir), 'utf8')).trim(), device_id: randomUUID(), second_factor: nextManagementSecondFactor() })).access_token
const admin = (route, body, expected, method) => request(management, route, token, body, expected, method)
const prefix = '/management/v1/organisations'
const organisation = await admin(prefix, { name: `Einladungs-Abnahme ${new Date().toISOString()}` })
const other = await admin(prefix, { name: `Einladungs-Abnahme zweite Organisation ${new Date().toISOString()}` })
const fixture = { organisation_id: organisation.id, second_organisation_id: other.id, email: `invitation-admin-${randomUUID()}@cowork.local`, password: randomBytes(30).toString('base64url'), member_email: `invitation-member-${randomUUID()}@cowork.local`, member_password: randomBytes(30).toString('base64url') }
await writeFile(fixtureFile, JSON.stringify(fixture, null, 2))
const invPath = org => `${prefix}/${org}/invitations`
const create = (auth, org, email, role = 'member') => request(management, invPath(org), auth, { email, role }, 201)
const inspect = value => request(management, '/api/v1/auth/organisation-invitations/inspect', null, { token: value.token })
const register = (value, password, display_name) => request(management, '/api/v1/auth/organisation-invitations/register', null, { token: value.token, password, display_name, device_id: randomUUID() }, 201)
const seed = await create(token, organisation.id, fixture.email, 'admin')
const orgAdmin = await register(seed, fixture.password, 'Einladungs-Testadministrator')
fixture.admin_id = orgAdmin.user_id
await writeFile(fixtureFile, JSON.stringify(fixture, null, 2))
const invitation = await create(orgAdmin.access_token, organisation.id, fixture.member_email)
const preview = await inspect(invitation)
assert.equal(preview.account_exists, false); assert.equal(preview.organisation_id, organisation.id)
const member = await register(invitation, fixture.member_password, 'Eingeladenes Testmitglied')
fixture.member_id = member.user_id
await writeFile(fixtureFile, JSON.stringify(fixture, null, 2))
const organisations = await request(management, prefix, member.access_token)
assert.equal(organisations.platform_admin, false)
assert.deepEqual(organisations.items.map(value => [value.id, value.role]), [[organisation.id, 'member']])
assert.deepEqual(await request(management, `${prefix}/${organisation.id}/instances`, member.access_token), [])
await request(management, invPath(organisation.id), member.access_token, undefined, 404)
await request(management, invPath(other.id), orgAdmin.access_token, undefined, 404)
await request(management, '/api/v1/auth/organisation-invitations/inspect', null, { token: invitation.token }, 401)
const existing = await create(token, other.id, fixture.member_email, 'auditor')
assert.equal((await inspect(existing)).account_exists, true)
await request(management, '/management/v1/invitations/accept', orgAdmin.access_token, { token: existing.token }, 401)
const accepted = await request(management, '/management/v1/invitations/accept', member.access_token, { token: existing.token })
assert.equal(accepted.organisation_id, other.id); assert.equal(accepted.role, 'auditor')
const revoked = await create(orgAdmin.access_token, organisation.id, `revoked-${randomUUID()}@cowork.local`)
await request(management, `${invPath(organisation.id)}/${revoked.id}`, orgAdmin.access_token, undefined, 200, 'DELETE')
await request(management, '/api/v1/auth/organisation-invitations/inspect', null, { token: revoked.token }, 401)
const listing = await request(management, invPath(organisation.id), orgAdmin.access_token)
assert.ok(listing.every(row => !('token' in row) && !('token_hash' in row)))
assert.equal(listing.find(row => row.id === invitation.id).state, 'accepted')
assert.equal(listing.find(row => row.id === revoked.id).state, 'revoked')
const audit = await admin(`${prefix}/${organisation.id}/audit`)
assert.ok(audit.some(row => row.action === 'invitation.accept' && row.target_id === invitation.id))
assert.ok(audit.some(row => row.action === 'invitation.revoke' && row.target_id === revoked.id))
const evidence = { verified_at: new Date().toISOString(), organisation_id: organisation.id, second_organisation_id: other.id, tests: ['org_admin_invitation', 'new_account_registration', 'no_platform_or_instance_access', 'foreign_org_denied', 'one_use', 'existing_identity_binding', 'second_org_join', 'revocation', 'secret_free_listing', 'audit'] }
await writeFile(new URL('organisation-invitations-results.json', privateDir), JSON.stringify(evidence, null, 2))
console.log('live_docker_organisation_invitation_acceptance=passed')
console.log(JSON.stringify(evidence))
