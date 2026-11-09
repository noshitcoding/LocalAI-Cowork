export const APP_UPDATE_SNOOZE_STORAGE_KEY = 'localai-cowork-update-snooze-until'
export const APP_UPDATE_SNOOZE_MS = 24 * 60 * 60_000

let sessionSnoozeUntil = 0

export function readAppUpdateSnoozeUntil(): number {
  let storedSnoozeUntil = 0
  try {
    const storedValue = window.localStorage.getItem(APP_UPDATE_SNOOZE_STORAGE_KEY)
    const parsedValue = storedValue ? Number(storedValue) : 0
    if (Number.isFinite(parsedValue) && parsedValue > 0) storedSnoozeUntil = parsedValue
  } catch {
    // The in-memory value still suppresses the dialog for this app session.
  }
  return Math.max(sessionSnoozeUntil, storedSnoozeUntil)
}

export function storeAppUpdateSnoozeUntil(snoozeUntil: number): void {
  sessionSnoozeUntil = snoozeUntil
  try {
    window.localStorage.setItem(APP_UPDATE_SNOOZE_STORAGE_KEY, String(snoozeUntil))
  } catch {
    // The in-memory value still suppresses the dialog for this app session.
  }
}

export function clearAppUpdateSnooze(): void {
  sessionSnoozeUntil = 0
  try {
    window.localStorage.removeItem(APP_UPDATE_SNOOZE_STORAGE_KEY)
  } catch {
    // Nothing else is required when WebView storage is unavailable.
  }
}

export function resetAppUpdateDialogForTests(): void {
  sessionSnoozeUntil = 0
}
