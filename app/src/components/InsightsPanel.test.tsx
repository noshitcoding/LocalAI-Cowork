import { render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import InsightsPanel from './InsightsPanel'
import { useInsightsStore } from '../stores/insightsStore'

const invokeMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

describe('InsightsPanel', () => {
  beforeEach(() => {
    invokeMock.mockReset()
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      value: {},
      configurable: true,
      writable: true,
    })
    useInsightsStore.setState({
      events: [],
      summary: null,
      loading: false,
      error: null,
    })
  })

  it('renders partial summary payloads without crashing', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'insights_summary') {
        return Promise.resolve({
          totalSessions: 2,
          totalEvents: 1,
        })
      }
      if (command === 'insights_list') {
        return Promise.resolve([])
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`))
    })

    render(<InsightsPanel />)

    await waitFor(() => {
      expect(screen.getByText('Sessions')).toBeInTheDocument()
    })
    expect(screen.getByText('2')).toBeInTheDocument()
    expect(screen.getByText(/0\.0\s+min/)).toBeInTheDocument()
  })

  it('accepts snake_case event payloads from Rust', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'insights_summary') {
        return Promise.resolve({
          total_events: 1,
          recent_events: [{ event_type: 'app_opened', category: 'system', created_at: '2026-04-17T10:00:00Z' }],
        })
      }
      if (command === 'insights_list') {
        return Promise.resolve([
          { id: 'e1', event_type: 'app_opened', category: 'system', created_at: '2026-04-17T10:00:00Z' },
        ])
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`))
    })

    render(<InsightsPanel />)

    await waitFor(() => {
      expect(screen.getAllByText('app_opened').length).toBeGreaterThan(0)
    })
    expect(screen.getAllByText('system').length).toBeGreaterThan(0)
  })
})
