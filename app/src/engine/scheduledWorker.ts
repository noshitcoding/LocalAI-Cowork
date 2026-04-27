import { useCoworkStore } from '../stores/coworkStore'

let intervalId: ReturnType<typeof setInterval> | null = null
let running = false

async function tick(): Promise<void> {
  if (running) return
  running = true

  try {
    const store = useCoworkStore.getState()
    await Promise.all([store.loadScheduledTasks(), store.loadScheduledRuns(20)])
  } catch (e) {
    console.error('[ScheduledWorker] Error:', e)
  } finally {
    running = false
  }
}

export function startScheduledWorker(): void {
  if (intervalId !== null) return
  console.log('[ScheduledWorker] Started')
  intervalId = setInterval(tick, 30_000)
  void tick()
}

export function stopScheduledWorker(): void {
  if (intervalId !== null) {
    clearInterval(intervalId)
    intervalId = null
    console.log('[ScheduledWorker] Stopped')
  }
}

export function isScheduledWorkerRunning(): boolean {
  return intervalId !== null
}
