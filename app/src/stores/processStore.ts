import { create } from 'zustand'
import { safeInvoke } from '../utils/safeInvoke'

export type ManagedProcess = {
  id: string
  label: string
  command: string
  status: string
  pid: number | null
  backend_id: string | null
  exit_code: number | null
  requires_admin: boolean
  admin_approved: boolean
  log_path: string | null
  started_at: string
  stopped_at: string | null
  created_at: string
}

export type ProcessStartResult = {
  processId: string
  pid: number | null
  status: string
  message: string
}

export type ProcessStatusResult = {
  processId: string
  label: string
  command: string
  status: string
  pid: number | null
  exitCode: number | null
  requiresAdmin: boolean
  adminApproved: boolean
}

type ProcessState = {
  processes: ProcessStatusResult[]
  loading: boolean
  error: string | null

  loadProcesses: () => Promise<void>
  startProcess: (label: string, command: string, backendId?: string, requiresAdmin?: boolean) => Promise<ProcessStartResult>
  stopProcess: (processId: string) => Promise<void>
  approveProcess: (processId: string, approved: boolean) => Promise<ProcessStartResult>
}

export const useProcessStore = create<ProcessState>()((set) => ({
  processes: [],
  loading: false,
  error: null,

  loadProcesses: async () => {
    set({ loading: true, error: null })
    try {
      const processes = await safeInvoke<ProcessStatusResult[]>('process_list', undefined, [])
      set({ processes, loading: false })
    } catch (e) {
      set({ error: String(e), loading: false })
    }
  },

  startProcess: async (label, command, backendId, requiresAdmin = false) => {
    try {
      return await safeInvoke<ProcessStartResult>('process_start', {
        label,
        command,
        backendId: backendId ?? null,
        requiresAdmin,
      }, {
        processId: `proc-${Date.now()}`,
        pid: null,
        status: 'unavailable',
        message: 'Process management is only available in the desktop app.',
      })
    } catch (e) {
      return {
        processId: `proc-err-${Date.now()}`,
        pid: null,
        status: 'error',
        message: String(e),
      }
    }
  },

  stopProcess: async (processId) => {
    try {
      await safeInvoke('process_stop', { processId }, undefined)
    } catch (e) {
      set({ error: String(e) })
    }
  },

  approveProcess: async (processId, approved) => {
    try {
      return await safeInvoke<ProcessStartResult>('process_approve', {
        processId,
        approved,
      }, {
        processId,
        pid: null,
        status: 'unavailable',
        message: 'Process approval is only available in the desktop app.',
      })
    } catch (e) {
      return {
        processId,
        pid: null,
        status: 'error',
        message: String(e),
      }
    }
  },
}))
