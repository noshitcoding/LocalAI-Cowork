import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import LeftSidebar from './LeftSidebar'
import { useChatStore } from '../stores/chatStore'
import { useConfigStore } from '../stores/configStore'
import { useCoworkStore } from '../stores/coworkStore'
import { useEngineStore } from '../stores/engineStore'

const navigateMock = vi.fn()

vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom')
  return {
    ...actual,
    useNavigate: () => navigateMock,
  }
})

describe('LeftSidebar', () => {
  beforeEach(() => {
    navigateMock.mockReset()

    useChatStore.setState({
      threads: [
        {
          id: 'thread-1',
          title: 'Lokaler Chat',
          messages: [],
          createdAt: 10,
          updatedAt: 10,
        },
      ],
      activeThreadId: 'thread-1',
      pendingApproval: [],
      busy: false,
      error: null,
    })

    useConfigStore.setState({
      ...useConfigStore.getState(),
      ollama: {
        ...useConfigStore.getState().ollama,
        model: 'gpt-oss:20b',
      },
      mcpServer: { name: 'local-mcp', command: '', args: '', env: {} },
    })

    useCoworkStore.setState({
      ...useCoworkStore.getState(),
      connectors: [
        { key: 'chrome', label: 'Chrome', enabled: true, note: '' },
      ],
      plugins: [
        { id: 'git', name: 'Git', domain: 'custom', enabled: true, skills: [] },
      ],
    })

    useEngineStore.setState({
      ...useEngineStore.getState(),
      currentSessionId: null,
      getSessions: vi.fn().mockResolvedValue([
        {
          id: 'session-1',
          title: 'Persistierte Analyse',
          cwd: 'C:/repo',
          createdAt: 100,
          updatedAt: 100,
          messageCount: 2,
        },
      ]),
      loadSessionById: vi.fn().mockResolvedValue({
        id: 'session-1',
        title: 'Persistierte Analyse',
        cwd: 'C:/repo',
        messages: [
          {
            type: 'user',
            uuid: 'message-1',
            timestamp: 100,
            content: [{ type: 'text', text: 'Bitte pruefe den Build.' }],
          },
        ],
        totalUsage: {
          inputTokens: 0,
          outputTokens: 0,
          cacheCreationInputTokens: 0,
          cacheReadInputTokens: 0,
        },
        totalCostUsd: 0,
        appState: {},
        createdAt: 100,
        updatedAt: 200,
      }),
    })
  })

  it('loads a persisted session from the sidebar and navigates to cowork', async () => {
    render(
      <MemoryRouter>
        <LeftSidebar />
      </MemoryRouter>,
    )

    await waitFor(() => expect(screen.getByText('Persistierte Analyse')).toBeInTheDocument())

    fireEvent.click(screen.getByRole('button', { name: /Persistierte Analyse/i }))

    await waitFor(() => {
      // Check that the session was loaded and navigation occurred
      expect(navigateMock).toHaveBeenCalledWith('/')
      // Verify the engine store's loadSessionById was called
      const engineState = useEngineStore.getState()
      expect(engineState.loadSessionById).toHaveBeenCalledWith('session-1')
    })

    expect(navigateMock).toHaveBeenCalledWith('/')
  })
})
