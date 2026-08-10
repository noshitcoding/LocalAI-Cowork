import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'

const IS_ANDROID_SHELL = import.meta.env.VITE_COWORK_ANDROID === 'true'

export type CredentialScope =
  | 'connector'
  | 'crew'
  | 'engine'
  | 'llm_profile'
  | 'mcp_env'
  | 'remote_server'

export type CredentialLocator = {
  scope: CredentialScope
  ownerId: string
  field: string
}

type CredentialReadResponse = {
  value: string | null
}

type CredentialExistsResponse = { exists: boolean }
type CredentialCopyResponse = { copied: boolean }

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

    if (IS_ANDROID_SHELL) {
      const { mobileSecureDelete, mobileSecureSet } = await import('../mobile/mobileSecure')
      if (value) {
        await mobileSecureSet('credentials', locatorKey(locator), value)
      } else {
        await mobileSecureDelete('credentials', locatorKey(locator))
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
  if (IS_ANDROID_SHELL) {
    const { mobileSecureGet } = await import('../mobile/mobileSecure')
    return mobileSecureGet('credentials', locatorKey(locator))
  }

  const response = await safeInvoke<CredentialReadResponse>('credential_get', {
    request: locator,
  })
  return response.value
}

export async function hasCredential(locator: CredentialLocator): Promise<boolean> {
  await waitForPendingWrite(locator)
  if (!hasTauriRuntime()) return volatileCredentials.has(locatorKey(locator))
  if (IS_ANDROID_SHELL) {
    const { mobileSecureGet } = await import('../mobile/mobileSecure')
    return (await mobileSecureGet('credentials', locatorKey(locator))) !== null
  }
  const response = await safeInvoke<CredentialExistsResponse>('credential_exists', { request: locator })
  return response.exists
}

export async function deleteCredential(locator: CredentialLocator): Promise<void> {
  await enqueue(locator, async () => {
    if (!hasTauriRuntime()) {
      volatileCredentials.delete(locatorKey(locator))
      return
    }
    if (IS_ANDROID_SHELL) {
      const { mobileSecureDelete } = await import('../mobile/mobileSecure')
      await mobileSecureDelete('credentials', locatorKey(locator))
      return
    }
    await safeInvoke<void>('credential_delete', { request: locator })
  })
}

export async function copyCredentialIfMissing(
  source: CredentialLocator,
  destination: CredentialLocator,
): Promise<boolean> {
  await Promise.all([waitForPendingWrite(source), waitForPendingWrite(destination)])
  if (!hasTauriRuntime()) {
    const destinationKey = locatorKey(destination)
    if (volatileCredentials.has(destinationKey)) return false
    const value = volatileCredentials.get(locatorKey(source))
    if (value === undefined) return false
    volatileCredentials.set(destinationKey, value)
    return true
  }
  if (IS_ANDROID_SHELL) {
    const { mobileSecureGet, mobileSecureSet } = await import('../mobile/mobileSecure')
    const destinationKey = locatorKey(destination)
    if (await mobileSecureGet('credentials', destinationKey)) return false
    const value = await mobileSecureGet('credentials', locatorKey(source))
    if (value === null) return false
    await mobileSecureSet('credentials', destinationKey, value)
    return true
  }
  const response = await safeInvoke<CredentialCopyResponse>('credential_copy', {
    request: { source, destination },
  })
  return response.copied
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
