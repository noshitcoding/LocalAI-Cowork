import { organisationListSchema } from './enterpriseContracts'
import { RemoteRuntimeClient, normalizeServerUrl, RuntimeHttpError } from './runtimeClient'

export type ServerKind = 'management' | 'workspace'

/** Probe only the selected server; never follow a redirect carrying a bearer token. */
export async function detectServerKind(serverUrl: string, accessToken: () => Promise<string>, signal?: AbortSignal): Promise<ServerKind> {
  const baseUrl = normalizeServerUrl(serverUrl)
  // The version route is present on legacy workspace servers, including those
  // behind gateways that serve HTML for every unknown path.
  try {
    await new RemoteRuntimeClient({ baseUrl, accessToken }).version()
    return 'workspace'
  } catch (error) {
    if (!(error instanceof RuntimeHttpError) || error.status !== 404) throw error
  }
  const response = await fetch(`${baseUrl}/management/v1/organisations`, {
    headers: { Authorization: `Bearer ${await accessToken()}` }, credentials: 'same-origin', redirect: 'error',
    signal: signal ? AbortSignal.any([signal, AbortSignal.timeout(15_000)]) : AbortSignal.timeout(15_000),
  })
  if (response.ok) {
    organisationListSchema.parse(await response.json())
    return 'management'
  }
  if (response.status===403) {
    const body:unknown=await response.clone().json().catch(()=>null)
    if (body && typeof body==='object' && 'error' in body && body.error==='mfa_required') return 'management'
  }
  throw await RuntimeHttpError.fromResponse(response)
}
