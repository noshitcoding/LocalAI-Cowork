import { describe, expect, it, vi } from 'vitest'
import type { ManagementClient } from './managementClient'
import { resourceCatalog, resourceInputSchema } from './resourceCatalog'

const resourceId = '10000000-0000-4000-8000-000000000001'
const orgId = '10000000-0000-4000-8000-000000000002'
const userId = '10000000-0000-4000-8000-000000000003'
const instanceId = '10000000-0000-4000-8000-000000000004'
const environmentId = '10000000-0000-4000-8000-000000000005'
const endpoint = { kind: 'endpoint' as const, name: 'Model provider', configuration: { base_url: 'https://models.example/v1', protocol: 'openai_compatible' } }

function client() { const request = vi.fn(); return { api: resourceCatalog({ request } as unknown as ManagementClient), request } }

describe('resource catalog request boundaries', () => {
  it('uses revision checks for updates and retirement without resolving credential references', () => {
    const { api, request } = client()
    api.update(resourceId, 4, { ...endpoint, secret_reference: 'env:MODEL_PROVIDER_SECRET' })
    expect(request).toHaveBeenCalledWith(`/resource-catalog/${resourceId}`, expect.anything(), expect.objectContaining({ expected_revision: 4, secret_reference: 'env:MODEL_PROVIDER_SECRET' }), 'PUT')
    api.remove(resourceId, 4)
    expect(request).toHaveBeenLastCalledWith(`/resource-catalog/${resourceId}`, expect.anything(), { expected_revision: 4 }, 'DELETE')
    expect(() => api.update(resourceId, 0, endpoint)).toThrow()
    expect(request).toHaveBeenCalledTimes(2)
  })

  it.each([
    'https://name:password@models.example/v1',
    'https://models.example/v1?api_key=secret',
    'https://models.example/v1#secret',
    'file:///models',
    'javascript:alert(1)',
  ])('rejects endpoint credentials and unsupported URL syntax before making requests: %s', base_url => {
    const { api, request } = client()
    expect(() => api.create({ ...endpoint, configuration: { ...endpoint.configuration, base_url } })).toThrow()
    expect(() => api.create({ kind: 'mcp', name: 'Tools', configuration: { transport: 'streamable_http', url: base_url } })).toThrow()
    expect(request).not.toHaveBeenCalled()
  })

  it('rejects unknown fields, inline secrets and kind-mismatched configuration', () => {
    const { api, request } = client()
    expect(() => api.create({ ...endpoint, configuration: { ...endpoint.configuration, api_key: 'forbidden' } })).toThrow()
    expect(() => api.create({ ...endpoint, secret_reference: 'inline-token' })).toThrow()
    expect(resourceInputSchema.safeParse({ ...endpoint, api_key: 'forbidden' }).success).toBe(false)
    expect(() => api.create({ kind: 'model', name: 'Model', configuration: endpoint.configuration })).toThrow()
    expect(request).not.toHaveBeenCalled()
  })

  it.each([
    { transport: 'streamable_http' },
    { transport: 'streamable_http', url: 'https://tools.example/mcp', command: 'tool-server' },
    { transport: 'streamable_http', url: 'https://tools.example/mcp', args: ['--verbose'] },
    { transport: 'stdio' },
    { transport: 'stdio', command: '   ' },
    { transport: 'stdio', command: 'tool-server', url: 'https://tools.example/mcp' },
  ])('requires transport-specific MCP fields: %j', configuration => {
    const { api, request } = client()
    expect(() => api.create({ kind: 'mcp', name: 'Tools', configuration })).toThrow()
    expect(request).not.toHaveBeenCalled()
  })

  it('binds assignment paths to UUID identities and environment scopes to an instance', () => {
    const { api, request } = client()
    expect(() => api.setAssignments(orgId, userId, 0, [{ resource_id: resourceId, environment_id: environmentId }])).toThrow()
    expect(() => api.assignments('../another-org', userId)).toThrow()
    expect(() => api.setAssignments(orgId, userId, -1, [])).toThrow()
    expect(request).not.toHaveBeenCalled()
    api.setAssignments(orgId, userId, 0, [{ resource_id: resourceId, instance_id: instanceId, environment_id: environmentId }])
    expect(request).toHaveBeenCalledWith(`/organisations/${orgId}/members/${userId}/resource-assignments`, expect.anything(), { expected_revision: 0, assignments: [{ resource_id: resourceId, instance_id: instanceId, environment_id: environmentId }] }, 'PUT')
  })
})
