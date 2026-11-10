import { z } from 'zod'
import { checkImportReceipt, importIntentSchema, importReceiptSchema, type ImportIntent, type ImportReceipt } from './instanceImportContracts'

export type ImportJournalScope = { server: string; user: string; organisation: string }
const scopeSchema = z.object({ server: z.url(), user: z.uuid(), organisation: z.uuid() }).strict()
const acceptedSchema = z.object({ intent: importIntentSchema, receipt: importReceiptSchema }).strict()
const journalSchema = z.object({ format: z.literal(1), scope: scopeSchema, pending: importIntentSchema.nullable(), accepted: z.array(acceptedSchema).max(100) }).strict()
export type ImportJournal = z.infer<typeof journalSchema>
const key = (scope: ImportJournalScope) => `cowork-instance-import-v1:${encodeURIComponent(JSON.stringify(scopeSchema.parse(scope)))}`

export function readImportJournal(scope: ImportJournalScope): ImportJournal {
  const raw = localStorage.getItem(key(scope))
  if (raw === null) return { format: 1, scope: scopeSchema.parse(scope), pending: null, accepted: [] }
  if (raw.length > 256 * 1024) throw new Error('Import journal is too large; preserve it for review')
  const journal = journalSchema.parse(JSON.parse(raw))
  if (JSON.stringify(journal.scope) !== JSON.stringify(scopeSchema.parse(scope))) throw new Error('Import journal scope changed')
  for (const entry of journal.accepted) checkImportReceipt(entry.intent, entry.receipt)
  return journal
}

async function update(scope: ImportJournalScope, change: (current: ImportJournal) => ImportJournal): Promise<ImportJournal> {
  if (!navigator.locks?.request) throw new Error('Durable import actions require browser storage locks')
  return navigator.locks.request(key(scope), async () => {
    const next = journalSchema.parse(change(readImportJournal(scope)))
    const encoded = JSON.stringify(next)
    if (encoded.length > 256 * 1024) throw new Error('Import journal is full; preserve it for review')
    localStorage.setItem(key(scope), encoded)
    if (localStorage.getItem(key(scope)) !== encoded) throw new Error('Import journal could not be persisted')
    return next
  })
}

export function retainImportIntent(scope: ImportJournalScope, intent: ImportIntent): Promise<ImportJournal> {
  const checked = importIntentSchema.parse(intent)
  return update(scope, current => {
    if (current.pending && JSON.stringify(current.pending) !== JSON.stringify(checked)) throw new Error('An unconfirmed import action is already retained')
    if (current.accepted.some(entry => entry.intent.body.idempotency_key === checked.body.idempotency_key)) throw new Error('This action already has an accepted receipt')
    if (current.accepted.length >= 100) throw new Error('Import journal is full; preserve it for review')
    return { ...current, pending: checked }
  })
}

export function acceptImportReceipt(scope: ImportJournalScope, intent: ImportIntent, receipt: ImportReceipt): Promise<ImportJournal> {
  checkImportReceipt(intent, receipt)
  return update(scope, current => {
    if (JSON.stringify(current.pending) !== JSON.stringify(intent)) {
      const previous = current.accepted.find(entry => JSON.stringify(entry.intent) === JSON.stringify(intent))
      if (previous && JSON.stringify(previous.receipt) === JSON.stringify(receipt)) return current
      throw new Error('The retained import action changed')
    }
    return { ...current, pending: null, accepted: [...current.accepted, { intent, receipt }] }
  })
}
