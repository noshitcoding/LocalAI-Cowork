import { z } from 'zod'

const id = z.uuid()
const revision = z.number().int().positive().max(Number.MAX_SAFE_INTEGER)
const digest = z.string().regex(/^[a-f0-9]{64}$/)
export const prepareImportSchema = z.object({ source_instance_id: id, target_instance_id: id, source_revision: revision, target_revision: revision, backup_id: id, idempotency_key: id }).strict().refine(value => value.source_instance_id !== value.target_instance_id, 'Choose distinct instances')
export const cutoverImportSchema = z.object({ expected_revision: revision, expected_route_revision: revision, target_instance_id: id, idempotency_key: id }).strict()
export const abortImportSchema = z.object({ expected_revision: revision, idempotency_key: id }).strict()
export const preparedImportReceiptSchema = z.object({ id, phase: z.literal('prepared'), revision: z.literal(1), operation_id: id }).strict()
export const cutoverImportReceiptSchema = z.object({ id, source_instance_id: id, target_instance_id: id, request_digest: digest, idempotency_key: id, revision, route_revision: revision, target_state: z.literal('stopped'), schedules_paused: z.literal(true) }).strict()
export const abortImportReceiptSchema = z.object({ id, phase: z.literal('aborted'), revision, request_digest: digest, idempotency_key: id, source_instance_id: id, target_instance_id: id, source_state: z.literal('stopped'), target_state: z.literal('stopped'), charges_retained: z.literal(true) }).strict()
export const instanceImportSchema = z.object({ id, organisation_id: id, source_instance_id: id, target_instance_id: id, backup_id: id, binding_sha256: digest, phase: z.enum(['prepared', 'source_quiescent', 'export_verified', 'target_restored_paused', 'qualified', 'source_retained', 'aborted']), revision })
export const instanceRouteSchema = z.object({ organisation_id: id, source_instance_id: id, target_instance_id: id, revision, import_id: id.nullable() })
export type PrepareImport = z.infer<typeof prepareImportSchema>
export type CutoverImport = z.infer<typeof cutoverImportSchema>
export type AbortImport = z.infer<typeof abortImportSchema>
export type InstanceImport = z.infer<typeof instanceImportSchema>
export type InstanceRoute = z.infer<typeof instanceRouteSchema>

export const importIntentSchema = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('prepare'), body: prepareImportSchema }).strict(),
  z.object({ kind: z.literal('cutover'), import_id: id, source_instance_id: id, body: cutoverImportSchema }).strict(),
  z.object({ kind: z.literal('abort'), import_id: id, source_instance_id: id, target_instance_id: id, body: abortImportSchema }).strict(),
])
export const importReceiptSchema = z.union([preparedImportReceiptSchema, cutoverImportReceiptSchema, abortImportReceiptSchema])
export type ImportIntent = z.infer<typeof importIntentSchema>
export type ImportReceipt = z.infer<typeof importReceiptSchema>

export function checkImportReceipt(intent: ImportIntent, receipt: ImportReceipt): void {
  if (intent.kind === 'prepare') {
    preparedImportReceiptSchema.parse(receipt)
    return
  }
  const value = intent.kind === 'cutover' ? cutoverImportReceiptSchema.parse(receipt) : abortImportReceiptSchema.parse(receipt)
  if (value.id !== intent.import_id || value.idempotency_key !== intent.body.idempotency_key || value.revision !== intent.body.expected_revision + 1 || value.source_instance_id !== intent.source_instance_id || value.target_instance_id !== (intent.kind === 'cutover' ? intent.body.target_instance_id : intent.target_instance_id)) throw new Error('Import receipt binding changed')
  if (intent.kind === 'cutover' && cutoverImportReceiptSchema.parse(value).route_revision !== intent.body.expected_route_revision + 1) throw new Error('Import route receipt changed')
}
