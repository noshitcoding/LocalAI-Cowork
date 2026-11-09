import { z } from 'zod'
import type { ManagementClient } from './managementClient'

export const managementCapabilitiesSchema = z.object({
  schema_version: z.number().int(), organisation_settings: z.boolean().default(false), organisation_archive: z.boolean().default(false),
  resource_catalog: z.boolean().default(false), environments: z.boolean().default(false), environment_access: z.boolean().default(false), directory: z.boolean().default(false), organisation_mfa: z.boolean().default(false),
  wallets: z.boolean().default(false),
})
export type ManagementCapabilities = z.infer<typeof managementCapabilitiesSchema>
export const legacyManagementCapabilities: ManagementCapabilities = { schema_version: 0, organisation_settings: false, organisation_archive: false, resource_catalog: false, environments: false, environment_access: false, directory: false, organisation_mfa: false, wallets: false }
export async function readManagementCapabilities(client: ManagementClient, signal?: AbortSignal) {
  return managementCapabilitiesSchema.parse(await client.request('/capabilities', managementCapabilitiesSchema, undefined, 'GET', signal))
}
