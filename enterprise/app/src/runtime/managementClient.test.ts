import { describe, expect, it } from 'vitest'
import { ManagementClient, enterprisePolicySchema, instanceActions, organisationListSchema } from './managementClient'

describe('management contract boundaries', () => {
  it('rejects an explicitly empty version instead of silently falling back to defaults', () => {
    const client=new ManagementClient('https://management.example')
    expect(()=>client.createInstance('11111111-1111-4111-8111-111111111111','Workspace','compose','')).toThrow()
  })
  it('never offers execution actions while provisioning or after deletion', () => {
    expect(instanceActions('provisioning')).toEqual([])
    expect(instanceActions('deleted')).toEqual([])
    expect(instanceActions('suspended')).not.toContain('start')
    expect(instanceActions('stopped')).toContain('restore')
  })
  it('requires the enterprise protocol instead of interpreting a legacy list', () => {
    expect(organisationListSchema.safeParse({ schema_version: 2, platform_admin: false, items: [] }).success).toBe(false)
    expect(organisationListSchema.parse({ schema_version: 3, platform_admin: false, items: [] }).items).toEqual([])
  })
  it('rejects host access extensions and impossible resource limits', () => {
    const policy = { revision: 1, extensions: 'catalog', content_access: 'support_grant', minimum_isolation: 'container', allow_laptop_full_access: false, allow_offline_continuation: true, allowed_network_origins: [], limits: { cpu_millis: 1000, memory_mib: 1024, storage_gib: 10, instances: 1, concurrent_runs: 1 }, audit_retention_days: 365, content_logging: false }
    expect(enterprisePolicySchema.safeParse(policy).success).toBe(true)
    expect(enterprisePolicySchema.safeParse({ ...policy, allow_host_mounts: true }).success).toBe(false)
    expect(enterprisePolicySchema.safeParse({ ...policy, limits: { ...policy.limits, concurrent_runs: 0 } }).success).toBe(false)
  })
})
