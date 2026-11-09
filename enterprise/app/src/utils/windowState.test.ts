import { describe, expect, it } from 'vitest'
import { fitWindowStateToMonitors, type PhysicalWorkArea, type StoredWindowState } from './windowState'

const primary: PhysicalWorkArea = {
  position: { x: 0, y: 0 },
  size: { width: 1920, height: 1040 },
}

function state(overrides: Partial<StoredWindowState> = {}): StoredWindowState {
  return { width: 1280, height: 800, x: 100, y: 100, maximized: false, ...overrides }
}

describe('fitWindowStateToMonitors', () => {
  it('keeps saved physical pixels unchanged at 100 and 150 percent scaling', () => {
    expect(fitWindowStateToMonitors(state(), [primary], primary)).toEqual(state())
    expect(fitWindowStateToMonitors(state({ x: 150, y: 150 }), [primary], primary)).toEqual(
      state({ x: 150, y: 150 }),
    )
  })

  it('chooses the monitor with the largest overlap including negative coordinates', () => {
    const left = { position: { x: -2560, y: -200 }, size: { width: 2560, height: 1440 } }
    const stored = state({ x: -1800, y: -100, width: 1400, height: 900 })
    expect(fitWindowStateToMonitors(stored, [primary, left], primary)).toEqual(stored)
  })

  it('moves a window from a removed monitor onto the primary monitor', () => {
    expect(fitWindowStateToMonitors(state({ x: 5000, y: 3000 }), [primary], primary)).toEqual(
      state({ x: 640, y: 240 }),
    )
  })

  it('shrinks oversized windows and enforces the available minimum size', () => {
    const small = { position: { x: 10, y: 20 }, size: { width: 400, height: 250 } }
    expect(fitWindowStateToMonitors(state({ width: 4000, height: 3000 }), [primary], primary)).toEqual(
      state({ width: 1920, height: 1040, x: 0, y: 0 }),
    )
    expect(fitWindowStateToMonitors(state({ width: 100, height: 100 }), [small], small)).toEqual(
      state({ width: 400, height: 250, x: 10, y: 20 }),
    )
  })

  it('rejects invalid geometry and missing monitor data', () => {
    expect(fitWindowStateToMonitors(state({ width: Number.NaN }), [primary], primary)).toBeNull()
    expect(fitWindowStateToMonitors(state(), [], null)).toBeNull()
  })
})
