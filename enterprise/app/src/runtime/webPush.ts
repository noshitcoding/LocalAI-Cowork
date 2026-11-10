import type { RemoteRuntimeClient } from './runtimeClient'

function base64UrlToBytes(value: string): Uint8Array<ArrayBuffer> {
  const padded = value.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - value.length % 4) % 4)
  const binary = atob(padded)
  return Uint8Array.from(binary, (character) => character.charCodeAt(0))
}

export async function enableWebPush(
  client: RemoteRuntimeClient,
  deviceId: string,
): Promise<'enabled' | 'unsupported' | 'server_disabled' | 'denied'> {
  if (!('serviceWorker' in navigator) || !('PushManager' in window) || !('Notification' in window)) {
    return 'unsupported'
  }
  const configuration = await client.pushConfiguration()
  if (!configuration.web_push_public_key) return 'server_disabled'
  const permission = await Notification.requestPermission()
  if (permission !== 'granted') return 'denied'
  const registration = await navigator.serviceWorker.register('/push-sw.js', { scope: '/' })
  const existing = await registration.pushManager.getSubscription()
  const subscription = existing ?? await registration.pushManager.subscribe({
    userVisibleOnly: true,
    applicationServerKey: base64UrlToBytes(configuration.web_push_public_key),
  })
  const json = subscription.toJSON()
  if (!json.endpoint || !json.keys?.p256dh || !json.keys.auth) {
    throw new Error('Browser returned an incomplete WebPush subscription')
  }
  await client.registerWebPush(deviceId, {
    endpoint: json.endpoint,
    p256dh: json.keys.p256dh,
    auth: json.keys.auth,
  })
  return 'enabled'
}
