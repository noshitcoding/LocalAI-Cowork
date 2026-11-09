import { z } from 'zod'
import type { ManagedInstance, ManagementClient } from './managementClient'

export const environmentAccessMemberSchema = z.object({ user_id: z.uuid(), role: z.enum(['viewer', 'runner']) }).strict()
export const environmentAccessSchema = z.object({
  organisation_id: z.uuid(), instance_id: z.uuid(), environment_id: z.uuid(), revision: z.number().int().positive(),
  mode: z.enum(['inherit_instance', 'restricted']), members: z.array(environmentAccessMemberSchema),
}).strict()
export const environmentAccessUpdateSchema = z.object({
  expected_revision: z.number().int().positive(), mode: z.enum(['inherit_instance', 'restricted']),
  members: z.array(environmentAccessMemberSchema).max(200),
}).strict()
export type EnvironmentAccess = z.infer<typeof environmentAccessSchema>
export type EnvironmentAccessUpdate = z.infer<typeof environmentAccessUpdateSchema>
export type EnvironmentAccessMember = z.infer<typeof environmentAccessMemberSchema>

export function environmentAccess(client: ManagementClient, instance: ManagedInstance, environmentId: string) {
  const path = `/organisations/${z.uuid().parse(instance.organisation_id)}/instances/${z.uuid().parse(instance.id)}/environments/${z.uuid().parse(environmentId)}/access`
  return {
    read: (signal?: AbortSignal) => client.request(path, environmentAccessSchema, undefined, 'GET', signal),
    update: (body: EnvironmentAccessUpdate) => client.request(path, environmentAccessSchema, environmentAccessUpdateSchema.parse(body), 'PUT'),
  }
}
