import { z } from 'zod'
import type { ManagementClient } from './managementClient'

export const organisationSettingsSchema = z.object({
  id: z.uuid(), name: z.string().min(1).max(120), revision: z.number().int().positive(),
  archived_at: z.string().datetime({ offset: true }).nullable(), archived_by: z.uuid().nullable(),
}).strict()
export const managementCapabilitiesSchema = z.object({
  schema_version: z.literal(1), organisation_settings: z.boolean(), organisation_archive: z.boolean(),
  resource_catalog: z.boolean(), central_team_management: z.boolean().default(false),
  environments: z.boolean(), directory: z.boolean(), organisation_mfa: z.boolean(),
}).passthrough()
export type OrganisationSettings = z.infer<typeof organisationSettingsSchema>
export type ManagementCapabilities = z.infer<typeof managementCapabilitiesSchema>

export function organisationLifecycle(client: ManagementClient, organisation: string) {
  const path = `/organisations/${z.uuid().parse(organisation)}`
  return {
    read: (signal?: AbortSignal) => client.request(`${path}/settings`, organisationSettingsSchema, undefined, 'GET', signal),
    rename: (name: string, revision: number) => client.request(`${path}/settings`, organisationSettingsSchema, { name, expected_revision: revision }, 'PUT'),
    archive: (confirmationName: string, revision: number) => client.request(`${path}/archive`, organisationSettingsSchema, { confirmation_name: confirmationName, expected_revision: revision }),
  }
}

export function managementCapabilities(client: ManagementClient, signal?: AbortSignal) {
  return client.request('/capabilities', managementCapabilitiesSchema, undefined, 'GET', signal)
}
