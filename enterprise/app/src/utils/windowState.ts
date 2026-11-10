import {
  PhysicalPosition,
  PhysicalSize,
  availableMonitors,
  getCurrentWindow,
  primaryMonitor,
} from '@tauri-apps/api/window'

export type StoredWindowState = {
  width: number
  height: number
  x: number
  y: number
  maximized: boolean
}

export type PhysicalWorkArea = {
  position: { x: number; y: number }
  size: { width: number; height: number }
}

const STORAGE_KEY = 'open-cowork-window-state-v1'
const MIN_WINDOW_WIDTH = 480
const MIN_WINDOW_HEIGHT = 320

function canUseDesktopApis(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

function saveToStorage(state: StoredWindowState): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(state))
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value)
}

function loadFromStorage(): StoredWindowState | null {
  const raw = localStorage.getItem(STORAGE_KEY)
  if (!raw) return null

  try {
    const parsed = JSON.parse(raw) as StoredWindowState
    if (
      !isFiniteNumber(parsed.width) || parsed.width <= 0 ||
      !isFiniteNumber(parsed.height) || parsed.height <= 0 ||
      !isFiniteNumber(parsed.x) ||
      !isFiniteNumber(parsed.y) ||
      typeof parsed.maximized !== 'boolean'
    ) {
      return null
    }
    return parsed
  } catch {
    return null
  }
}

function isValidWorkArea(workArea: PhysicalWorkArea): boolean {
  return isFiniteNumber(workArea.position.x)
    && isFiniteNumber(workArea.position.y)
    && isFiniteNumber(workArea.size.width)
    && workArea.size.width > 0
    && isFiniteNumber(workArea.size.height)
    && workArea.size.height > 0
}

function intersectionArea(state: StoredWindowState, workArea: PhysicalWorkArea): number {
  const left = Math.max(state.x, workArea.position.x)
  const top = Math.max(state.y, workArea.position.y)
  const right = Math.min(state.x + state.width, workArea.position.x + workArea.size.width)
  const bottom = Math.min(state.y + state.height, workArea.position.y + workArea.size.height)
  return Math.max(0, right - left) * Math.max(0, bottom - top)
}

function sameWorkArea(left: PhysicalWorkArea, right: PhysicalWorkArea): boolean {
  return left.position.x === right.position.x
    && left.position.y === right.position.y
    && left.size.width === right.size.width
    && left.size.height === right.size.height
}

export function fitWindowStateToMonitors(
  state: StoredWindowState,
  workAreas: PhysicalWorkArea[],
  primaryWorkArea: PhysicalWorkArea | null,
): StoredWindowState | null {
  if (
    !isFiniteNumber(state.width) || state.width <= 0 ||
    !isFiniteNumber(state.height) || state.height <= 0 ||
    !isFiniteNumber(state.x) ||
    !isFiniteNumber(state.y)
  ) {
    return null
  }

  const validWorkAreas = workAreas.filter(isValidWorkArea)
  const validPrimary = primaryWorkArea && isValidWorkArea(primaryWorkArea)
    ? primaryWorkArea
    : null
  if (validWorkAreas.length === 0 && !validPrimary) return null

  let target = validWorkAreas[0] ?? validPrimary!
  let largestIntersection = 0
  for (const workArea of validWorkAreas) {
    const area = intersectionArea(state, workArea)
    if (area > largestIntersection) {
      target = workArea
      largestIntersection = area
    }
  }
  if (largestIntersection === 0 && validPrimary) {
    target = validWorkAreas.find((workArea) => sameWorkArea(workArea, validPrimary)) ?? validPrimary
  }

  const width = Math.round(Math.min(
    target.size.width,
    Math.max(Math.min(MIN_WINDOW_WIDTH, target.size.width), state.width),
  ))
  const height = Math.round(Math.min(
    target.size.height,
    Math.max(Math.min(MIN_WINDOW_HEIGHT, target.size.height), state.height),
  ))
  const maxX = target.position.x + target.size.width - width
  const maxY = target.position.y + target.size.height - height

  return {
    ...state,
    width,
    height,
    x: Math.round(Math.min(Math.max(state.x, target.position.x), maxX)),
    y: Math.round(Math.min(Math.max(state.y, target.position.y), maxY)),
  }
}

async function captureAndStoreWindowState(): Promise<void> {
  const currentWindow = getCurrentWindow()
  const size = await currentWindow.outerSize()
  const position = await currentWindow.outerPosition()
  const maximized = await currentWindow.isMaximized()

  saveToStorage({
    width: size.width,
    height: size.height,
    x: position.x,
    y: position.y,
    maximized,
  })
}

async function restoreStoredWindowState(state: StoredWindowState): Promise<boolean> {
  try {
    const [monitors, primary] = await Promise.all([availableMonitors(), primaryMonitor()])
    const fitted = fitWindowStateToMonitors(
      state,
      monitors.map((monitor) => monitor.workArea),
      primary?.workArea ?? null,
    )
    if (!fitted) return false

    const currentWindow = getCurrentWindow()
    await currentWindow.setSize(new PhysicalSize(fitted.width, fitted.height))
    await currentWindow.setPosition(new PhysicalPosition(fitted.x, fitted.y))
    if (fitted.maximized) {
      await currentWindow.maximize()
    }
    return true
  } catch {
    return false
  }
}

export async function setupWindowStatePersistence(): Promise<void> {
  if (!canUseDesktopApis()) return

  const currentWindow = getCurrentWindow()
  const storedState = loadFromStorage()

  if (storedState) {
    await restoreStoredWindowState(storedState)
  }

  await captureAndStoreWindowState()

  await currentWindow.onMoved(async () => {
    await captureAndStoreWindowState()
  })

  await currentWindow.onResized(async () => {
    await captureAndStoreWindowState()
  })
}
