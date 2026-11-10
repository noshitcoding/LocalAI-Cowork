import { z } from 'zod'
import type { ManagementClient } from './managementClient'

export const walletMicrosSchema = z.number().int().min(0).max(9_000_000_000_000)
const uuid = z.uuid()
export const memberWalletSchema = z.object({
  organisation_id: uuid, user_id: uuid, email: z.string(), display_name: z.string().nullable(),
  currency: z.string().regex(/^[A-Z]{3}$/), revision: z.number().int().positive(), enabled: z.boolean(),
  balance_micros: walletMicrosSchema, reserved_micros: walletMicrosSchema, available_micros: walletMicrosSchema,
  monthly_limit_micros: walletMicrosSchema.nullable(), monthly_spent_micros: walletMicrosSchema,
  monthly_reserved_micros: walletMicrosSchema, period_start: z.string(),
  enforcement: z.object({ state: z.enum(['enforced', 'disabled']), coverage: z.literal('server_managed_models'), automatic_charging: z.literal(true) }).strict(),
}).strict()
export const walletEntrySchema = z.object({
  id: uuid, request_id: uuid, kind: z.enum(['credit', 'debit', 'usage_charge']), amount_micros: walletMicrosSchema,
  balance_after_micros: walletMicrosSchema, reason: z.string(), created_at: z.string(), actor_user_id: uuid.nullable(),
}).strict()
export const walletReservationSchema = z.object({
  id: uuid, state: z.enum(['reserved', 'held', 'settled']), maximum_micros: walletMicrosSchema,
  charged_micros: walletMicrosSchema.nullable(), created_at: z.string(),
}).strict()
export const walletDetailSchema = memberWalletSchema.extend({
  entries: z.array(walletEntrySchema), reservations: z.array(walletReservationSchema), has_more: z.boolean(),
}).strict()
export const walletListSchema = z.object({
  organisation_id: uuid, currency: z.string().regex(/^[A-Z]{3}$/), settings_revision: z.number().int().positive(),
  currency_locked: z.boolean(), coverage: z.literal('managed_model_proxy'), wallets: z.array(memberWalletSchema),
}).strict()
export const walletUpdateSchema = z.object({ expected_revision: z.number().int().positive(), enabled: z.boolean(), monthly_limit_micros: walletMicrosSchema.nullable() }).strict()
export const walletEntryInputSchema = z.object({
  request_id: uuid, expected_revision: z.number().int().positive(), kind: z.enum(['credit', 'debit']),
  amount_micros: walletMicrosSchema.refine(value => value > 0), reason: z.string().trim().min(1).max(500),
}).strict()
export const walletResolveSchema = z.object({
  request_id: uuid, expected_revision: z.number().int().positive(), actual_micros: walletMicrosSchema,
  reason: z.string().trim().min(1).max(500),
}).strict()
export const walletSettingsSchema = z.object({ organisation_id: uuid, currency: z.string().regex(/^[A-Z]{3}$/), revision: z.number().int().positive(), currency_locked: z.boolean() }).strict()
export const walletSettingsUpdateSchema = z.object({ expected_revision: z.number().int().positive(), currency: z.string().regex(/^[A-Z]{3}$/) }).strict()
export type MemberWallet = z.infer<typeof memberWalletSchema>
export type WalletDetail = z.infer<typeof walletDetailSchema>
export type WalletList = z.infer<typeof walletListSchema>
export type WalletEntry = z.infer<typeof walletEntrySchema>
export type WalletUpdate = z.infer<typeof walletUpdateSchema>
export type WalletEntryInput = z.infer<typeof walletEntryInputSchema>
export type WalletResolve = z.infer<typeof walletResolveSchema>
export type WalletSettings = z.infer<typeof walletSettingsSchema>

export function wallets(client: ManagementClient, organisationId: string) {
  const base = `/organisations/${uuid.parse(organisationId)}`
  const path = (userId: string) => `${base}/members/${uuid.parse(userId)}/wallet`
  return {
    list: (signal?: AbortSignal) => client.request(`${base}/wallets`, walletListSchema, undefined, 'GET', signal),
    read: (userId: string, signal?: AbortSignal) => client.request(path(userId), walletDetailSchema, undefined, 'GET', signal),
    update: (userId: string, body: WalletUpdate) => client.request(path(userId), walletDetailSchema, walletUpdateSchema.parse(body), 'PUT'),
    entry: (userId: string, body: WalletEntryInput) => client.request(`${path(userId)}/entries`, walletDetailSchema, walletEntryInputSchema.parse(body), 'POST'),
    resolve: (userId: string, reservationId: string, body: WalletResolve) => client.request(`${path(userId)}/reservations/${uuid.parse(reservationId)}/resolve`, walletDetailSchema, walletResolveSchema.parse(body), 'POST'),
    settings: (body: z.infer<typeof walletSettingsUpdateSchema>) => client.request(`${base}/wallet-settings`, walletSettingsSchema, walletSettingsUpdateSchema.parse(body), 'PUT'),
  }
}
