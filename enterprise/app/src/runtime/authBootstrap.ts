import { normalizeServerUrl, RuntimeHttpError } from './runtimeClient'

/** Only public setup state is requested; no account identifiers or tokens are sent. */
export async function getBootstrapAvailability(serverUrl: string, signal?: AbortSignal, fetchImplementation = globalThis.fetch): Promise<boolean> {
  const response = await fetchImplementation(`${normalizeServerUrl(serverUrl)}/api/v1/auth/bootstrap/status`, {
    method: 'GET', credentials: 'omit', cache: 'no-store', redirect: 'error',
    signal: signal ? AbortSignal.any([signal, AbortSignal.timeout(10_000)]) : AbortSignal.timeout(10_000),
  })
  if (!response.ok) throw await RuntimeHttpError.fromResponse(response)
  const value: unknown = await response.json()
  if (!value || typeof value !== 'object' || !('bootstrap_available' in value) || typeof value.bootstrap_available !== 'boolean') {
    throw new Error('Invalid server setup status')
  }
  return value.bootstrap_available
}
