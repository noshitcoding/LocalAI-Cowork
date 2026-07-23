/**
 * Safe invoke wrapper for Tauri IPC bridge.
 * Prevents `TypeError: Cannot read properties of undefined (reading 'invoke')`
 * when running outside the Tauri desktop runtime (e.g. in a browser dev server).
 */

import { invoke } from '@tauri-apps/api/core'

const NOT_AVAILABLE_MSG = 'Tauri runtime is not available - feature can only be used in the desktop app.'

/** Check whether the Tauri runtime is available. */
export function hasTauriRuntime(): boolean {
  return (
    typeof window !== 'undefined' &&
    '__TAURI_INTERNALS__' in window
  )
}

/**
 * Safe wrapper for `invoke<T>()`.
 * Returns `fallback` when the Tauri runtime is not available.
 */
export async function safeInvoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
  fallback?: T,
): Promise<T> {
  if (!hasTauriRuntime()) {
    if (fallback !== undefined) return fallback
    throw new Error(NOT_AVAILABLE_MSG)
  }
  return invoke<T>(cmd, args)
}

/**
 * Fire-and-forget variant of `safeInvoke`.
 * Silently ignores calls when Tauri is unavailable.
 */
export async function safeInvokeVoid(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<void> {
  if (!hasTauriRuntime()) return
  try {
    await invoke(cmd, args)
  } catch (error) {
    console.warn(`[safeInvokeVoid] ${cmd} failed:`, error)
  }
}
