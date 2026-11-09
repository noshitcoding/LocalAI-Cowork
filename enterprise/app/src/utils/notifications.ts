import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from '@tauri-apps/plugin-notification'

function hasTauriRuntime(): boolean {
  return typeof window !== 'undefined' && Boolean((window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
}

export async function showDesktopNotification(title: string, body: string): Promise<boolean> {
  if (!hasTauriRuntime()) {
    return false
  }

  try {
    let permissionGranted = await isPermissionGranted()

    if (!permissionGranted) {
      const permission = await requestPermission()
      permissionGranted = permission === 'granted'
    }

    if (!permissionGranted) {
      return false
    }

    sendNotification({ title, body })
    return true
  } catch {
    return false
  }
}
