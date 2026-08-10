import { invoke } from '@tauri-apps/api/core'

import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import { IS_ANDROID_SHELL } from './mobileSecure'

type FirebaseBuildConfig = {
  projectId: string
  applicationId: string
  apiKey: string
  senderId: string
}

type PermissionResponse = { granted: boolean; requested: boolean }
type TokenResponse = { token: string }
export type NativePushEvent = {
  runId: string
  eventKind: string
  sequence: number
  receivedAt: number
}

function firebaseBuildConfig(): FirebaseBuildConfig | null {
  const config = {
    projectId: import.meta.env.VITE_COWORK_FIREBASE_PROJECT_ID ?? '',
    applicationId: import.meta.env.VITE_COWORK_FIREBASE_APPLICATION_ID ?? '',
    apiKey: import.meta.env.VITE_COWORK_FIREBASE_API_KEY ?? '',
    senderId: import.meta.env.VITE_COWORK_FIREBASE_SENDER_ID ?? '',
  }
  return Object.values(config).every(Boolean) ? config : null
}

export function androidPushBuildConfigured(): boolean {
  return IS_ANDROID_SHELL && firebaseBuildConfig() !== null
}

export async function enableAndroidPush(
  client: RemoteRuntimeClient,
  deviceId: string,
): Promise<'enabled' | 'not_configured' | 'server_disabled'> {
  if (!IS_ANDROID_SHELL) return 'not_configured'
  const config = firebaseBuildConfig()
  if (!config) return 'not_configured'
  const server = await client.pushConfiguration()
  if (!server.fcm_enabled) return 'server_disabled'
  await invoke<PermissionResponse>('plugin:mobile-push|request_permission')
  const response = await invoke<TokenResponse>('plugin:mobile-push|token', config)
  await client.registerFcmPush(deviceId, response.token)
  return 'enabled'
}

export async function consumeAndroidPushEvents(): Promise<NativePushEvent[]> {
  if (!IS_ANDROID_SHELL || !firebaseBuildConfig()) return []
  const response = await invoke<{ events: NativePushEvent[] }>('plugin:mobile-push|consume_events')
  return response.events
}
