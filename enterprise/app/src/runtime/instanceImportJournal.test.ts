import { beforeEach, describe, expect, it, vi } from 'vitest'
import { checkImportReceipt, prepareImportSchema, type ImportIntent } from './instanceImportContracts'
import { acceptImportReceipt, readImportJournal, retainImportIntent } from './instanceImportJournal'

const uuid = (n: number) => `${n.toString().padStart(8, '0')}-1111-4111-8111-111111111111`
const scope = { server: 'https://manager.test', user: uuid(1), organisation: uuid(2) }
const intent: ImportIntent = { kind: 'prepare', body: { source_instance_id: uuid(3), target_instance_id: uuid(4), source_revision: 2, target_revision: 3, backup_id: uuid(5), idempotency_key: uuid(6) } }
const receipt = { id: uuid(7), phase: 'prepared' as const, revision: 1 as const, operation_id: uuid(8) }

describe('durable import intention boundaries', () => {
  beforeEach(() => {
    localStorage.clear()
    Object.defineProperty(navigator, 'locks', { configurable: true, value: { request: vi.fn(async (_: string, callback: () => unknown) => callback()) } })
  })
  it('retains the exact request across reload, rejects another pending action and accepts only the original receipt', async () => {
    await retainImportIntent(scope, intent)
    expect(readImportJournal(scope).pending).toEqual(intent)
    await expect(retainImportIntent(scope, { ...intent, body: { ...intent.body, idempotency_key: uuid(9) } })).rejects.toThrow('already retained')
    await acceptImportReceipt(scope, intent, receipt)
    expect(readImportJournal(scope)).toMatchObject({ pending: null, accepted: [{ intent, receipt }] })
    await acceptImportReceipt(scope, intent, receipt)
    expect(readImportJournal(scope).accepted).toHaveLength(1)
    await expect(retainImportIntent(scope, intent)).rejects.toThrow('accepted receipt')
  })
  it('isolates pending bodies and receipts by server, actor and organisation', async () => {
    await retainImportIntent(scope, intent)
    for (const other of [{ ...scope, user: uuid(10) }, { ...scope, organisation: uuid(11) }, { ...scope, server: 'https://other.test' }]) expect(readImportJournal(other).pending).toBeNull()
    expect(readImportJournal(scope).pending).toEqual(intent)
  })
  it('preserves corrupt and unknown journals without overwriting or issuing admission', async () => {
    await retainImportIntent(scope, intent)
    const key = localStorage.key(0)!
    for (const raw of ['{bad', JSON.stringify({ ...readImportJournal(scope), format: 2 })]) {
      localStorage.setItem(key, raw)
      await expect(retainImportIntent(scope, intent)).rejects.toThrow()
      expect(localStorage.getItem(key)).toBe(raw)
    }
  })
  it('blocks mutations without storage locks or when durable storage fails', async () => {
    Object.defineProperty(navigator, 'locks', { configurable: true, value: undefined })
    await expect(retainImportIntent(scope, intent)).rejects.toThrow('storage locks')
    Object.defineProperty(navigator, 'locks', { configurable: true, value: { request: async (_: string, callback: () => unknown) => callback() } })
    const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('disk full') })
    await expect(retainImportIntent(scope, intent)).rejects.toThrow('disk full')
    write.mockRestore()
    expect(readImportJournal(scope).pending).toBeNull()
  })
  it('rejects unsafe revisions, self import and mismatched cutover receipts', () => {
    expect(prepareImportSchema.safeParse({ ...intent.body, target_instance_id: intent.body.source_instance_id }).success).toBe(false)
    expect(prepareImportSchema.safeParse({ ...intent.body, source_revision: 2 ** 54 }).success).toBe(false)
    const cutover: ImportIntent = { kind: 'cutover', import_id: uuid(7), source_instance_id: uuid(3), body: { expected_revision: 5, expected_route_revision: 1, target_instance_id: uuid(4), idempotency_key: uuid(9) } }
    const response = { id: uuid(7), source_instance_id: uuid(3), target_instance_id: uuid(4), request_digest: 'a'.repeat(64), idempotency_key: uuid(9), revision: 6, route_revision: 2, target_state: 'stopped' as const, schedules_paused: true as const }
    checkImportReceipt(cutover, response)
    for (const changed of [{ ...response, target_instance_id: uuid(12) }, { ...response, idempotency_key: uuid(13) }, { ...response, route_revision: 3 }, { ...response, revision: 7 }]) expect(() => checkImportReceipt(cutover, changed)).toThrow()
  })
})
