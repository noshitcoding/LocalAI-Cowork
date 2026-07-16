import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'

export type CredentialScope =
  | 'connector'
  | 'crew'
  | 'engine'
  | 'llm_profile'
  | 'mcp_env'

export type CredentialLocator = {
  scope: CredentialScope
  ownerId: string
  field: string
}

type CredentialReadResponse = {
  value: string | null
}

const volatileCredentials = new Map<string, string>()
const writeQueues = new Map<string, Promise<unknown>>()

function locatorKey(locator: CredentialLocator): string {
  return `${locator.scope}\0${locator.ownerId}\0${locator.field}`
}

function enqueue<T>(locator: CredentialLocator, operation: () => Promise<T>): Promise<T> {
  const key = locatorKey(locator)
  const previous = writeQueues.get(key) ?? Promise.resolve()
  const current = previous.catch(() => undefined).then(operation)
  writeQueues.set(key, current)
  void current.finally(() => {
    if (writeQueues.get(key) === current) {
      writeQueues.delete(key)
    }
  }).catch(() => undefined)
  return current
}

async function waitForPendingWrite(locator: CredentialLocator): Promise<void> {
  const pending = writeQueues.get(locatorKey(locator))
  if (pending) {
    await pending
  }
}

export async function setCredential(locator: CredentialLocator, value: string): Promise<void> {
  await enqueue(locator, async () => {
    if (!hasTauriRuntime()) {
      const key = locatorKey(locator)
      if (value) {
        volatileCredentials.set(key, value)
      } else {
        volatileCredentials.delete(key)
      }
      return
    }

    await safeInvoke<void>('credential_set', {
      request: { ...locator, value },
    })
  })
}

export async function getCredential(locator: CredentialLocator): Promise<string | null> {
  await waitForPendingWrite(locator)
  if (!hasTauriRuntime()) {
    return volatileCredentials.get(locatorKey(locator)) ?? null
  }

  const response = await safeInvoke<CredentialReadResponse>('credential_get', {
    request: locator,
  })
  return response.value
}

export async function deleteCredential(locator: CredentialLocator): Promise<void> {
  await enqueue(locator, async () => {
    if (!hasTauriRuntime()) {
      volatileCredentials.delete(locatorKey(locator))
      return
    }
    await safeInvoke<void>('credential_delete', { request: locator })
  })
}

export async function replaceCredentialMap(
  scope: CredentialScope,
  ownerId: string,
  previous: Record<string, string>,
  next: Record<string, string>,
): Promise<void> {
  const fields = new Set([...Object.keys(previous), ...Object.keys(next)])
  await Promise.all(Array.from(fields, async (field) => {
    const locator = { scope, ownerId, field }
    if (field in next) {
      await setCredential(locator, next[field] ?? '')
    } else {
      await deleteCredential(locator)
    }
  }))
}

export function mcpCredentialOwner(server: { id?: string; name: string }): string {
  return server.id?.trim() || `legacy:${server.name.trim()}`
}

export function llmApiKeyLocator(profileId: string): CredentialLocator {
  return { scope: 'llm_profile', ownerId: profileId, field: 'api_key' }
}

export function connectorLocator(
  connectorKey: string,
  field: 'api_key' | 'webhook_url',
): CredentialLocator {
  return { scope: 'connector', ownerId: connectorKey, field }
}

export function crewProviderLocator(
  crewId: string,
  provider: 'openai_compatible' | 'openrouter',
): CredentialLocator {
  return { scope: 'crew', ownerId: crewId, field: `${provider}_api_key` }
}

export function resetVolatileCredentialsForTests(): void {
  volatileCredentials.clear()
  writeQueues.clear()
}
