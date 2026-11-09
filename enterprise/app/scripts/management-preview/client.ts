import { z } from 'zod'
import { controllers, delivery, environments, instances, members, now, organisations, resources, rollouts, templates, uid, userId, walletDetail, walletList } from './fixtures'
export * from '../../src/runtime/enterpriseContracts'

const readonly = () => { throw new Error('TESTDATEN: Diese Vorschau ist schreibgeschützt. Es werden keine echten Daten geändert.') }
export function instanceActions() { return [] }
export class ManagementClient {
  constructor(_baseUrl?: string) {}
  async request<T>(path: string, schema: z.ZodType<T>, body?: unknown, method = body === undefined ? 'GET' : 'POST', signal?: AbortSignal): Promise<T> {
    if (signal?.aborted) throw new DOMException('Aborted', 'AbortError')
    if (method !== 'GET') return readonly()
    const org = organisations.find(value => path.includes(value.id)) ?? organisations[0]
    let value: unknown
    if (path === '/capabilities') value = { schema_version: 1, organisation_settings: true, organisation_archive: true, resource_catalog: true, environments: true, environment_access: true, directory: true, organisation_mfa: true, wallets: true }
    else if (path.endsWith('/wallets')) value = walletList(org.id)
    else if (path.endsWith('/wallet')) value = walletDetail(org.id, members.find(member => path.includes(member.user_id))?.user_id ?? userId)
    else if (path.endsWith('/access') && path.includes('/environments/')) value = { organisation_id: org.id, instance_id: instances.find(instance => path.includes(instance.id))?.id ?? uid(11), environment_id: environments.find(environment => path.includes(environment.id))?.id ?? uid(21), revision: 2, mode: 'restricted', members: [{ user_id: userId, role: 'runner' }, { user_id: uid(91), role: 'viewer' }] }
    else if (path.endsWith('/settings')) value = { id: org.id, name: org.name, revision: 2, archived_at: null, archived_by: null }
    else if (path.endsWith('/resource-assignments')) value = { organisation_id: org.id, user_id: path.includes(uid(91)) ? uid(91) : userId, revision: 1, assignments: [{ resource_id: uid(62), instance_id: null, environment_id: null, available: true }, { resource_id: uid(64), instance_id: uid(11), environment_id: null, available: true }], delivery }
    else if (path === '/resource-catalog' || path.endsWith('/resources')) value = { items: resources, delivery }
    else if (path.endsWith('/members')) value = members
    else if (path.endsWith('/audit')) value = [{ sequence: 1, action: 'instance.create', created_at: now, target_id: uid(11) }, { sequence: 2, action: 'member.assign', created_at: now, target_id: uid(91) }]
    else if (path.endsWith('/operations')) value = [{ id: uid(71), action: 'provision', state: 'completed', created_at: now, result: { receipt_id: uid(72) } }]
    else if (path.endsWith('/templates')) value = templates
    else if (path.endsWith('/environment-controllers')) value = controllers
    else if (path.endsWith('/environments')) value = path.includes(uid(11)) ? environments : []
    else if (path.endsWith('/mfa-policy')) value = { organisation_id: org.id, revision: 1, epoch: uid(81), required: true, max_age_seconds: 43200, allow_validated_idp: true }
    else if (path.includes('/content-sharing/')) value = []
    else value = []
    return schema.parse(structuredClone(value))
  }
  async organisations() { return { schema_version: 3, platform_admin: true, execution_admission_enabled: true, release_selection_enabled: true, instance_imports_enabled: false, items: structuredClone(organisations) } }
  async instances(org: string) { return structuredClone(instances.filter(value => value.organisation_id === org)) }
  async rollouts(org: string) { return structuredClone(rollouts.filter(value => value.organisation_id === org)) }
  async releases() { return [{ id: 'b'.repeat(64), name: 'Werkzeugstand Oktober', source_release: 'a'.repeat(64), qualified: true }] }
  async provisioningReleases() { return [{ id: 'a'.repeat(64), name: 'Stabiler Teamserver' }] }
  async rollbacks() { return [] }
  async backups() { return [] }
  async invitations(org: string) { return [{ id: uid(73), organisation_id: org, email: 'taylor@example.invalid', role: 'member', state: 'pending', created_at: now, expires_at: '2026-10-24T06:00:00Z' }] }
  async instanceMembers() { return [{ ...members[0], role: 'owner', organisation_role: 'admin' }, { ...members[1], role: 'member', organisation_role: 'member' }] }
  async instanceMemberCandidates() { return members }
  async directory(org: string) { return { version: 1, items: org === uid(1) ? [{ id: uid(151), organisation_id: org, provider_id: 'preview-sso', issuer: 'https://idp.example.invalid', client_id: 'preview-directory', kind: 'scim', fingerprint: 'd'.repeat(64), operator_revision: 2, operator_active: true, configured: true, settings: { revision: 2, epoch: uid(152), state: 'preview', adopt_provider_memberships: false, provision_accounts: false, retention_days: 30, provider_fingerprint: 'd'.repeat(64), sso_revision: 1, policy_revision: 3 }, job: { phase: 'preview', pages: 2, entries: 84, last_success_at: now, last_error_code: null, next_poll_at: '2026-10-17T06:15:00Z' }, resources: 84, quarantined: 3 }] : [], scim_path: `/management/scim/v2/${org}`, legacy_logout_scope: 'user-wide-unclassified-logout' } }
  async sso(org: string) { return org === uid(2) ? { organisation_id: org, revision: 2, enabled: true, available: true, provider_id: 'preview-sso', auto_provision: true, group_mappings: [{ group: 'cowork-team', role: 'member' }, { group: 'cowork-audit', role: 'auditor' }], providers: [{ id: 'preview-sso', label: 'TESTDATEN · Team-Anmeldung', issuer: 'https://idp.example.invalid', groups_claim: 'groups' }] } : { organisation_id: org, revision: 1, enabled: false, available: false, provider_id: null, auto_provision: false, group_mappings: [], providers: [] } }
  async executionCapacity() { return { held_runs: 0, budget: { revision: 1, organisation_concurrent_runs: 4, user_concurrent_runs: 2 }, reservations: [] } }
  createOrganisation = readonly
  createInstance = readonly
  policy = readonly
  action = readonly
  launch = readonly
  createRollout = readonly
  controlRollout = readonly
  setExecutionBudget = readonly
  createInvitation = readonly
  revokeInvitation = readonly
  updateSso = readonly
  setDirectory = readonly
  prepareDirectoryContainment = readonly
  confirmDirectoryContainment = readonly
  setInstanceMember = readonly
  removeInstanceMember = readonly
  recoverExecutions = readonly
}
