import { z } from 'zod'
import { normalizeServerUrl } from './runtimeClient'

const key = 'open-cowork-organisation-invitation-v1'
const pendingSchema = z.object({ serverUrl: z.string(), token: z.string().min(32).max(256), expiresAt: z.number() }).strict()
export function invitationLink(serverUrl: string, token: string): string {
  const url = new URL(`${normalizeServerUrl(serverUrl)}/join`)
  url.hash = new URLSearchParams({ invitation: token }).toString()
  return url.href
}
export function pendingInvitation(serverUrl: string): string {
  try {
    const data = pendingSchema.parse(JSON.parse(window.sessionStorage.getItem(key) ?? 'null'))
    if (data.expiresAt <= Date.now()) { window.sessionStorage.removeItem(key); return '' }
    return data.serverUrl === normalizeServerUrl(serverUrl) ? data.token : ''
  } catch { return '' }
}
export function rememberInvitation(serverUrl: string, token: string, expiresAt: string): void {
  const data = pendingSchema.parse({ serverUrl: normalizeServerUrl(serverUrl), token, expiresAt: Math.min(Date.parse(expiresAt), Date.now() + 15 * 60 * 1000) })
  window.sessionStorage.setItem(key, JSON.stringify(data))
}
export function clearInvitation(): void { try { window.sessionStorage.removeItem(key) } catch { /* Storage may be disabled by the browser. */ } }
