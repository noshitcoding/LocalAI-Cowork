import { z } from 'zod'
import type { ManagementClient } from './managementClient'

const id = z.uuid()
export const resourceKindSchema = z.enum(['model', 'endpoint', 'mcp', 'skill', 'agent_role'])
export type ResourceKind = z.infer<typeof resourceKindSchema>
const reference = z.string().regex(/^(secret|vault|env):[A-Za-z0-9_./-]{1,180}$/)
const endpointUrl = z.url().refine(value => {
  const url = new URL(value)
  return ['http:', 'https:'].includes(url.protocol) && !url.username && !url.password && !url.search && !url.hash
}, 'Use an HTTP(S) URL without credentials, query or fragment')
export const resourceConfigurationSchemas = {
  model: z.object({ model_id: z.string().trim().min(1).max(200), endpoint_resource_id: id.nullish(), context_window: z.number().int().positive().max(1_000_000).nullish(), max_output_tokens: z.number().int().positive().max(131_072).nullish(),
    pricing: z.object({ currency: z.string().regex(/^[A-Z]{3}$/), input_micros_per_million: z.number().int().nonnegative().max(1_000_000_000_000), output_micros_per_million: z.number().int().nonnegative().max(1_000_000_000_000) }).strict().nullish(),
  }).strict().refine(value => value.context_window == null || value.max_output_tokens == null || value.max_output_tokens < value.context_window, 'Output limit must be smaller than the context window'),
  endpoint: z.object({ base_url: endpointUrl, protocol: z.enum(['openai_compatible', 'ollama', 'anthropic', 'azure_openai']) }).strict(),
  mcp: z.object({ transport: z.enum(['streamable_http', 'stdio']), url: endpointUrl.nullish(), command: z.string().max(512).nullish(), args: z.array(z.string().max(1024)).max(64).default([]) }).strict().superRefine((value, context) => {
    if (value.transport === 'streamable_http' && (!value.url || value.command != null || value.args.length > 0)) {
      context.addIssue({ code: 'custom', message: 'HTTP MCP requires a URL and cannot contain command or args' })
    }
    if (value.transport === 'stdio' && (!value.command?.trim() || value.url != null)) {
      context.addIssue({ code: 'custom', message: 'stdio MCP requires a command and cannot contain a URL' })
    }
  }),
  skill: z.object({ instructions: z.string().trim().min(1).max(16384) }).strict(),
  agent_role: z.object({ system_prompt: z.string().trim().min(1).max(16384), resource_ids: z.array(id).max(100).default([]) }).strict(),
}
export const resourceInputSchema = z.object({
  kind: resourceKindSchema, name: z.string().trim().min(1).max(120), description: z.string().max(4000).default(''),
  configuration: z.record(z.string(), z.unknown()), secret_reference: reference.nullish(), enabled: z.boolean().default(true),
}).strict().superRefine((value, context) => {
  const config = resourceConfigurationSchemas[value.kind].safeParse(value.configuration)
  if (!config.success) context.addIssue({ code: 'custom', path: ['configuration'], message: 'Configuration does not match this resource kind' })
})
export type ResourceInput = z.input<typeof resourceInputSchema>
export const resourceRecordSchema = z.object({
  id, kind: resourceKindSchema, name: z.string(), description: z.string(), configuration: z.record(z.string(), z.unknown()),
  secret_reference: reference.nullable(), enabled: z.boolean(), revision: z.number().int().positive(),
  created_by: id, updated_by: id, created_at: z.string(), updated_at: z.string(), deleted_at: z.string().nullable(), available: z.boolean(),
})
export type CatalogResource = z.infer<typeof resourceRecordSchema>
export const resourceDeliverySchema = z.object({ state: z.enum(['awaiting_consumer','partial_consumers']), enforced: z.literal(false), message: z.string(), supported_model_subset: z.object({
  consumer_kind: z.literal('model_provider'), version: z.literal(1), protocols: z.array(z.literal('openai_compatible')),
  scopes: z.array(z.enum(['organisation', 'instance'])), enforcement: z.literal('fresh_resolution_only'), requires_instance_upgrade: z.literal(true), immediate_server_runs_only: z.literal(true),
}) })
export const resourceListSchema = z.object({ items: z.array(resourceRecordSchema), delivery: resourceDeliverySchema })
export const resourceAssignmentInputSchema = z.object({ resource_id: id, instance_id: id.nullish(), environment_id: id.nullish() }).strict().refine(value => !value.environment_id || !!value.instance_id, 'Environment scope requires an instance')
export type ResourceAssignment = z.input<typeof resourceAssignmentInputSchema>
export const resourceAssignmentSetSchema = z.object({
  organisation_id: id, user_id: id, revision: z.number().int().nonnegative(),
  assignments: z.array(z.object({ resource_id: id, instance_id: id.nullable(), environment_id: id.nullable(), available: z.boolean() })),
  delivery: resourceDeliverySchema,
})
export type ResourceAssignmentSet = z.infer<typeof resourceAssignmentSetSchema>

/** Agent roles are model instructions, never platform or organisation roles.
 * Stored grants are delivered to opted-in consumers; legacy runtimes do not
 * enforce them. This client neither sends nor resolves provider credentials. */
export function resourceCatalog(client: ManagementClient) {
  return {
    list: (signal?: AbortSignal) => client.request('/resource-catalog', resourceListSchema, undefined, 'GET', signal),
    visible: (organisation: string, signal?: AbortSignal) => client.request(`/organisations/${id.parse(organisation)}/resources`, resourceListSchema, undefined, 'GET', signal),
    create: (input: ResourceInput) => client.request('/resource-catalog', resourceRecordSchema, resourceInputSchema.parse(input)),
    update: (resource: string, expectedRevision: number, input: ResourceInput) => client.request(`/resource-catalog/${id.parse(resource)}`, resourceRecordSchema, { ...resourceInputSchema.parse(input), expected_revision: z.number().int().positive().parse(expectedRevision) }, 'PUT'),
    remove: (resource: string, expectedRevision: number) => client.request(`/resource-catalog/${id.parse(resource)}`, z.object({ id, deleted: z.literal(true), revoked_assignments: z.number().int().nonnegative() }), { expected_revision: z.number().int().positive().parse(expectedRevision) }, 'DELETE'),
    assignments: (organisation: string, user: string, signal?: AbortSignal) => client.request(`/organisations/${id.parse(organisation)}/members/${id.parse(user)}/resource-assignments`, resourceAssignmentSetSchema, undefined, 'GET', signal),
    setAssignments: (organisation: string, user: string, expectedRevision: number, assignments: ResourceAssignment[]) => client.request(`/organisations/${id.parse(organisation)}/members/${id.parse(user)}/resource-assignments`, resourceAssignmentSetSchema, {
      expected_revision: z.number().int().nonnegative().parse(expectedRevision), assignments: z.array(resourceAssignmentInputSchema).max(200).parse(assignments),
    }, 'PUT'),
  }
}
