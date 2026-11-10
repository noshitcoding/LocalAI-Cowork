import { act, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useCrewRuntimeStore } from '../../stores/crewRuntimeStore'
import { hasTauriRuntime } from '../../utils/safeInvoke'
import CrewRuntimePanel from './CrewRuntimePanel'

vi.mock('../../utils/safeInvoke', () => ({
  hasTauriRuntime: vi.fn(() => false),
  safeInvoke: vi.fn(),
}))

const hasTauriRuntimeMock = vi.mocked(hasTauriRuntime)

describe('CrewRuntimePanel', () => {
  beforeEach(() => {
    hasTauriRuntimeMock.mockReset()
    hasTauriRuntimeMock.mockReturnValue(false)
    useCrewRuntimeStore.setState({
      status: null,
      snapshot: { state: 'unknown', reasonCode: null, checkedAt: null, generation: 0, operationId: null, recoverable: true },
      loading: false,
      bootstrapping: false,
      error: null,
      loadStatus: vi.fn().mockResolvedValue(undefined),
      bootstrap: vi.fn().mockResolvedValue(undefined),
      ensureReady: vi.fn().mockResolvedValue(undefined),
    })
  })

  it('does not continuously request desktop runtime status in a browser preview', async () => {
    await act(async () => {
      render(<CrewRuntimePanel />)
    })

    expect(useCrewRuntimeStore.getState().loadStatus).not.toHaveBeenCalled()
    expect(screen.getByText('Unknown')).toBeInTheDocument()
  })

  it('subscribes without independently loading status in the desktop runtime', async () => {
    hasTauriRuntimeMock.mockReturnValue(true)

    await act(async () => {
      render(<CrewRuntimePanel />)
    })

    expect(useCrewRuntimeStore.getState().loadStatus).not.toHaveBeenCalled()
  })
})
