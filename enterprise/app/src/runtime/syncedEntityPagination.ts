import type { SyncedEntity } from './contracts'
import type { RemoteRuntimeClient } from './runtimeClient'

export async function listAllSyncedEntities(
  client: RemoteRuntimeClient,
  entityType: string,
  pageSize = 500,
): Promise<SyncedEntity[]> {
  const entities: SyncedEntity[] = []
  const seenCursors = new Set<string>()
  let after: string | null = null

  do {
    const page = await client.listSyncedEntities(entityType, after, pageSize)
    entities.push(...page.items)
    const nextAfter = page.next_after
    if (nextAfter && seenCursors.has(nextAfter)) {
      throw new Error('Server returned a repeated metadata cursor')
    }
    if (nextAfter) seenCursors.add(nextAfter)
    after = nextAfter
  } while (after)

  return entities
}
