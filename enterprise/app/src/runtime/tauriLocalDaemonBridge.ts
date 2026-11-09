import { invoke } from '@tauri-apps/api/core'

import type { LocalDaemonBridge } from './localDaemonClient'

export const tauriLocalDaemonBridge: LocalDaemonBridge = {
  call: (method, params) =>
    invoke<unknown>('local_daemon_call', {
      method,
      params: params ?? null,
    }),
}
