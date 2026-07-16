import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { redactRecord, redactText } from '../security/redaction'

export type AppLogLevel = 'info' | 'warn' | 'error'

export type AppLogEntry = {
  id: string
  timestamp: number
  level: AppLogLevel
  area: string
  message: string
  details?: Record<string, unknown>
}

type LogState = {
  entries: AppLogEntry[]
  addLog: (entry: Omit<AppLogEntry, 'id' | 'timestamp'>) => void
  clearLogs: () => void
}

function generateId(): string {
  return `${Date.now()}-${Math.random().toString(36).slice(2, 9)}`
}

export function sanitizeAppLogEntry(entry: AppLogEntry): AppLogEntry {
  return {
    ...entry,
    message: redactText(entry.message),
    details: entry.details ? redactRecord(entry.details) : undefined,
  }
}

export const useLogStore = create<LogState>()(
  persist(
    (set) => ({
      entries: [],
      addLog: (entry) =>
        set((state) => ({
          entries: [
            sanitizeAppLogEntry({
              ...entry,
              id: generateId(),
              timestamp: Date.now(),
            }),
            ...state.entries,
          ].slice(0, 200),
        })),
      clearLogs: () => set({ entries: [] }),
    }),
    {
      name: 'open-cowork-logs',
      partialize: (state) => ({
        entries: state.entries.map(sanitizeAppLogEntry),
      }),
      merge: (persisted, current) => {
        const state = persisted as Partial<LogState>
        return {
          ...current,
          entries: (state.entries ?? []).map(sanitizeAppLogEntry).slice(0, 200),
        }
      },
    }
  )
)
