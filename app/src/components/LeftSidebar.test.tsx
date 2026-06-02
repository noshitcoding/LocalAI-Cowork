import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import LeftSidebar from './LeftSidebar'
import { useChatStore } from '../stores/chatStore'
import { useConfigStore } from '../stores/configStore'
import { useCoworkStore } from '../stores/coworkStore'
import { useEngineStore } from '../stores/engineStore'
import { useProjectStore } from '../stores/projectStore'

const navigateMock = vi.fn()

vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom')
  return {
    ...actual,
    useNavigate: () => navigateMock,
  }
})

function createMockDataTransfer(): DataTransfer {
  const data = new Map<string, string>()
  return {
    dropEffect: 'none',
    effectAllowed: 'all',
    files: [] as unknown as FileList,
    items: [] as unknown as DataTransferItemList,
    types: [],
    clearData: vi.fn((type?: string) => {
      if (type) data.delete(type)
      else data.clear()
    }),
    getData: vi.fn((type: string) => data.get(type) ?? ''),
    setData: vi.fn((type: string, value: string) => {
      data.set(type, value)
    }),
    setDragImage: vi.fn(),
  } as unknown as DataTransfer
}

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
        model: 'llama3.1:8b',
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

    useProjectStore.setState({
      projects: [],
      activeProjectId: null,
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

  it('moves a chat into a project via drag and drop', () => {
    useProjectStore.setState({
      projects: [
        {
          id: 'project-1',
          title: 'Alpha',
          instructions: '',
          resources: [],
          threadIds: [],
          createdAt: 100,
          updatedAt: 100,
        },
      ],
      activeProjectId: 'project-1',
    })

    render(
      <MemoryRouter>
        <LeftSidebar />
      </MemoryRouter>,
    )

    const dataTransfer = createMockDataTransfer()
    fireEvent.dragStart(screen.getByRole('button', { name: 'Lokaler Chat' }), { dataTransfer })
    fireEvent.drop(screen.getByRole('button', { name: /Alpha/i }), { dataTransfer })

    expect(useProjectStore.getState().projects[0].threadIds).toEqual(['thread-1'])
  })

  it('moves a chat into a project via pointer drag fallback', async () => {
    useProjectStore.setState({
      projects: [
        {
          id: 'project-1',
          title: 'Alpha',
          instructions: '',
          resources: [],
          threadIds: [],
          createdAt: 100,
          updatedAt: 100,
        },
      ],
      activeProjectId: 'project-1',
    })

    render(
      <MemoryRouter>
        <LeftSidebar />
      </MemoryRouter>,
    )

    const projectTarget = screen.getByRole('button', { name: /Alpha/i }).closest('[data-sidebar-project-id]')
    expect(projectTarget).not.toBeNull()
    const originalElementFromPoint = document.elementFromPoint
    Object.defineProperty(document, 'elementFromPoint', {
      configurable: true,
      value: vi.fn(() => projectTarget),
    })

    try {
      const chat = screen.getByRole('button', { name: 'Lokaler Chat' })
      fireEvent.pointerDown(chat, { pointerId: 1, button: 0, clientX: 10, clientY: 10 })
      fireEvent.pointerMove(window, { pointerId: 1, clientX: 40, clientY: 40 })
      fireEvent.pointerUp(window, { pointerId: 1, clientX: 40, clientY: 40 })

      await waitFor(() => {
        expect(useProjectStore.getState().projects[0].threadIds).toEqual(['thread-1'])
      })
    } finally {
      Object.defineProperty(document, 'elementFromPoint', {
        configurable: true,
        value: originalElementFromPoint,
      })
    }
  })

  it('detaches a project chat by dropping it onto the Chats group', () => {
    useProjectStore.setState({
      projects: [
        {
          id: 'project-1',
          title: 'Alpha',
          instructions: '',
          resources: [],
          threadIds: ['thread-1'],
          createdAt: 100,
          updatedAt: 100,
        },
      ],
      activeProjectId: 'project-1',
    })

    render(
      <MemoryRouter>
        <LeftSidebar />
      </MemoryRouter>,
    )

    const dataTransfer = createMockDataTransfer()
    fireEvent.dragStart(screen.getByRole('button', { name: 'Lokaler Chat' }), { dataTransfer })
    fireEvent.drop(screen.getByRole('button', { name: /Chats0/i }), { dataTransfer })

    expect(useProjectStore.getState().projects[0].threadIds).toEqual([])
  })
})
