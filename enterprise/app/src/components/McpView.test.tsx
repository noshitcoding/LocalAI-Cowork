import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import McpView from './McpView'

const mocks = vi.hoisted(() => ({
  safeInvoke: vi.fn(),
  setMcpServer: vi.fn(),
}))

vi.mock('../utils/safeInvoke', () => ({
  safeInvoke: (...args: unknown[]) => mocks.safeInvoke(...args),
}))

vi.mock('../stores/configStore', () => ({
  useConfigStore: () => ({
    mcpServer: { id: 'current-id', name: 'current-runtime', command: 'node', args: 'current.js', env: {} },
    mcpServers: [{ id: 'current-id', name: 'current-runtime', command: 'node', args: 'current.js', env: {} }],
    activeMcpServerName: 'current-runtime',
    preferences: { mcpAllowManualImport: false, mcpEnvEditorEnabled: true },
    setMcpServer: mocks.setMcpServer,
    setMcpServerEnv: vi.fn(async () => undefined),
    setActiveMcpServer: vi.fn(),
    importMcpServers: vi.fn(async () => undefined),
    deleteMcpServer: vi.fn(async () => undefined),
  }),
}))

describe('McpView runtime identity', () => {
  beforeEach(() => {
    mocks.safeInvoke.mockReset()
    mocks.setMcpServer.mockReset()
    mocks.safeInvoke.mockImplementation(async (command: string) => {
      if (command === 'mcp_runtime_list') {
        return [
          { name: 'current-runtime', command: 'node', args: ['current.js'], pid: 10, startedAt: new Date().toISOString(), lastError: null },
          { name: 'orphaned-runtime', command: 'node', args: ['old.js'], pid: 11, startedAt: new Date().toISOString(), lastError: null },
        ]
      }
      if (command === 'mcp_runtime_stop') return true
      return undefined
    })
  })

  it('locks the active runtime name and can stop any listed runtime by its actual name', async () => {
    render(<McpView />)

    expect(await screen.findByRole('button', { name: 'Stop runtime orphaned-runtime' })).toBeInTheDocument()
    expect(screen.getByLabelText('Name')).toBeDisabled()

    fireEvent.click(screen.getByRole('button', { name: 'Stop runtime orphaned-runtime' }))

    await vi.waitFor(() => expect(mocks.safeInvoke).toHaveBeenCalledWith(
      'mcp_runtime_stop',
      { name: 'orphaned-runtime' },
      false,
    ))
  })
})
