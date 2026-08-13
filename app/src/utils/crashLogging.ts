import { useLogStore } from '../stores/logStore'
import { writeAuditEvent } from './audit'

let crashLoggingRegistered = false

function normalizeError(value: unknown): { message: string; stack?: string } {
  if (value instanceof Error) {
    return {
      message: value.message,
      stack: value.stack,
    }
  }

  if (typeof value === 'string') {
    return { message: value }
  }

  try {
    return { message: JSON.stringify(value) }
  } catch {
    return { message: String(value) }
  }
}

export function registerGlobalCrashLogging(): void {
  if (typeof window === 'undefined') return
  if (crashLoggingRegistered) return
  crashLoggingRegistered = true

  window.addEventListener('error', (event) => {
    const normalized = normalizeError(event.error ?? event.message)
    const details = {
      message: normalized.message,
      stack: normalized.stack,
      source: event.filename,
      line: event.lineno,
      column: event.colno,
    }

    useLogStore.getState().addLog({
      level: 'error',
      area: 'runtime',
      message: 'Unhandled frontend error',
      details,
    })

    void writeAuditEvent('runtime', 'frontend_unhandled_error', details)
  })

  window.addEventListener('unhandledrejection', (event) => {
    const normalized = normalizeError(event.reason)
    const details = {
      message: normalized.message,
      stack: normalized.stack,
    }

    useLogStore.getState().addLog({
      level: 'error',
      area: 'runtime',
      message: 'Unhandled promise rejection',
      details,
    })

    void writeAuditEvent('runtime', 'frontend_unhandled_rejection', details)
  })
}
