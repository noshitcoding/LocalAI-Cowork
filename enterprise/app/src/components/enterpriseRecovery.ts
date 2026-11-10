import type { RemoteRuntimeClient } from '../runtime/runtimeClient'

export type EnterpriseIntent = { format: 1; scope: string; path: string; purpose: string; request_id: string; sha256: string; body?: unknown }
const memory = new Map<string, string>()
const uuid = /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/
const sha = /^[0-9a-f]{64}$/
function scope(client: RemoteRuntimeClient, user: string) { if (!uuid.test(user)) throw new Error('Invalid original user'); return JSON.stringify([client.enterpriseSource, user]) }
function key(source: string) { return 'cowork-enterprise-intent/1:' + source }
function purpose(path: string) {
  if (path === '/device-grants') return 'device_grant_create'
  if (path === '/workflows') return 'workflow_create'
  if (path === '/transfer-grants') return 'transfer_grant_create'
  if (path === '/transfers') return 'transfer_begin'
  const match = /^\/(device-grants|workflows|transfer-grants|transfers)\/([0-9a-f-]{36})\/(stop|continue|reconcile|revoke|reassign)$/.exec(path)
  if (!match || !uuid.test(match[2])) throw new Error('Invalid original mutation path')
  const kinds: Record<string, string> = { 'device-grants/stop': 'device_stop', 'workflows/stop': 'workflow_stop', 'workflows/continue': 'workflow_continue', 'workflows/reconcile': 'workflow_reconcile', 'workflows/reassign':'workflow_reassign', 'transfer-grants/revoke': 'transfer_revoke', 'transfers/reconcile': 'transfer_environment_reconcile' }
  const value = kinds[match[1] + '/' + match[3]]; if (!value) throw new Error('Unsupported original operation'); return value
}
async function digest(bytes: string) { return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(bytes))), b => b.toString(16).padStart(2, '0')).join('') }
export function enterprisePending(client: RemoteRuntimeClient, user: string): EnterpriseIntent | undefined {
  const source = scope(client, user), text = localStorage.getItem(key(source)); if (!text) return
  if (text.length > 1024 * 1024) throw new Error('Original request metadata exceeds its bound')
  const value = JSON.parse(text) as EnterpriseIntent
  if (value.format !== 1 || value.scope !== source || !uuid.test(value.request_id) || !sha.test(value.sha256) || purpose(value.path) !== value.purpose || Object.keys(value).some(k => !['format', 'scope', 'path', 'purpose', 'request_id', 'sha256', 'body'].includes(k))) throw new Error('Original request metadata changed')
  return value
}
async function locked<T>(client: RemoteRuntimeClient, user: string, action: () => Promise<T>) {
  if (!navigator.locks) throw new Error('Exclusive browser request custody is unavailable')
  return navigator.locks.request(key(scope(client, user)), { mode: 'exclusive' }, action)
}
function settle(client: RemoteRuntimeClient, user: string, original: EnterpriseIntent, receipt: unknown) {
  const result = receipt as Record<string, unknown> | null
  if (!result || result.request_id !== original.request_id || result.receipt_pending === true) return false
  const current = enterprisePending(client, user)
  if (current?.request_id !== original.request_id || current.sha256 !== original.sha256) throw new Error('Original request custody changed')
  localStorage.removeItem(key(original.scope)); memory.delete(original.scope); return true
}
export async function enterprisePost(client: RemoteRuntimeClient, user: string, path: string, body: unknown) {
  return locked(client, user, async () => {
    if (enterprisePending(client, user)) throw new Error('Settle the original pending request before creating another request')
    const serialized = JSON.stringify(body), request = (body as Record<string, unknown>).request_id
    if (typeof request !== 'string' || !uuid.test(request) || new TextEncoder().encode(serialized).byteLength > 1024 * 1024) throw new Error('Invalid bounded original request')
    const source = scope(client, user), kind = purpose(path)
    // Prompts and task arguments remain in memory. Durable custody retains only
    // their original request ID and digest, never model content or credentials.
    const original: EnterpriseIntent = { format: 1, scope: source, path, purpose: kind, request_id: request, sha256: await digest(serialized), ...((kind==='workflow_create'||kind==='workflow_reassign'&&(body as Record<string,unknown>).payload!==undefined) ? {} : { body }) }
    localStorage.setItem(key(source), JSON.stringify(original)); memory.set(source, serialized)
    const receipt = await client.enterprise(path, 'POST', JSON.parse(serialized),user)
    if (!settle(client, user, original, receipt)) throw new Error('The original request receipt is not yet confirmed')
    return receipt
  })
}
export async function enterpriseRecover(client: RemoteRuntimeClient, user: string) {
  return locked(client, user, async () => { const original = enterprisePending(client, user); if (!original) return null; const receipt = await client.enterprise('/operations/' + original.request_id,'GET',undefined,user); settle(client, user, original, receipt); return receipt })
}
export async function enterpriseRetryOriginal(client: RemoteRuntimeClient, user: string) {
  return locked(client, user, async () => {
    const original = enterprisePending(client, user); if (!original) return null
    const serialized = memory.get(original.scope) ?? (original.body === undefined ? undefined : JSON.stringify(original.body))
    if (!serialized) throw new Error('Original prompt bytes are no longer available; read its receipt or explicitly reject an undispatched request')
    if (await digest(serialized) !== original.sha256 || JSON.parse(serialized).request_id !== original.request_id) throw new Error('Original request bytes changed')
    const receipt = await client.enterprise(original.path, 'POST', JSON.parse(serialized),user); settle(client, user, original, receipt); return receipt
  })
}
export async function enterpriseRejectUndispatched(client: RemoteRuntimeClient, user: string) {
  return locked(client, user, async () => {
    const original = enterprisePending(client, user); if (!original) return null
    const receipt = await client.enterprise('/operations/' + original.request_id + '/reject', 'POST', { expected_request_sha256: original.sha256, purpose: original.purpose },user) as Record<string, unknown>
    if (receipt.state !== 'rejected' || receipt.admitted !== false || receipt.request_sha256 !== original.sha256) throw new Error('No positive rejection of the original undispatched request')
    settle(client, user, original, receipt); return receipt
  })
}
