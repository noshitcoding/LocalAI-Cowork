import { z } from 'zod'
import type { ManagementClient } from './managementClient'
export const organisationMfaPolicySchema=z.object({organisation_id:z.uuid(),revision:z.number().int().positive(),epoch:z.uuid(),required:z.boolean(),max_age_seconds:z.number().int().min(300).max(43200),allow_validated_idp:z.boolean()}).strict()
export const organisationMfaUpdateSchema=z.object({request_id:z.uuid(),expected_revision:z.number().int().positive(),required:z.boolean(),max_age_seconds:z.number().int().min(300).max(43200),allow_validated_idp:z.boolean(),confirmation:z.literal('CHANGE_ORGANISATION_MFA')}).strict()
export const organisationMfaReceiptSchema=z.object({request_id:z.uuid(),actor_user_id:z.uuid(),policy:organisationMfaPolicySchema,previous_revision:z.number().int().positive(),applied_at:z.string().datetime({offset:true})}).strict()
export const instanceMfaConfigurationSchema=z.object({version:z.literal(1),instance_id:z.uuid(),policy:organisationMfaPolicySchema}).strict()
export type OrganisationMfaPolicy=z.infer<typeof organisationMfaPolicySchema>
export type OrganisationMfaUpdate=z.infer<typeof organisationMfaUpdateSchema>
export type OrganisationMfaReceipt=z.infer<typeof organisationMfaReceiptSchema>
export function organisationMfa(client:ManagementClient,organisation:string){const path=`/organisations/${z.uuid().parse(organisation)}/mfa-policy`;return {read:()=>client.request(path,organisationMfaPolicySchema),update:(body:OrganisationMfaUpdate)=>client.request(path,organisationMfaReceiptSchema,organisationMfaUpdateSchema.parse(body),'PUT')}}
