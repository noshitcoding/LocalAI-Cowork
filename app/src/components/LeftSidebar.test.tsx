import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import LeftSidebar from './LeftSidebar'
import { useChatStore } from '../stores/chatStore'
import { useConfigStore } from '../stores/configStore'
import { useCoworkStore } from '../stores/coworkStore'
import { useProjectStore } from '../stores/projectStore'
import { getProductRouteById } from '../product/routeRegistry'
import i18n from '../i18n'

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
  beforeEach(async () => {
    await i18n.changeLanguage('en')
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

  })

  it('uses route registry paths for sidebar navigation actions', () => {
    render(
      <MemoryRouter>
        <LeftSidebar />
      </MemoryRouter>,
    )

    fireEvent.click(screen.getByRole('button', { name: '+ New chat' }))
    expect(navigateMock).toHaveBeenLastCalledWith(getProductRouteById('cowork').path)

    navigateMock.mockClear()
    fireEvent.click(screen.getByRole('button', { name: '+ New project' }))
    expect(navigateMock).toHaveBeenLastCalledWith(getProductRouteById('projects').path)

    navigateMock.mockClear()
    fireEvent.click(screen.getByRole('button', { name: 'Crew Studio' }))
    expect(navigateMock).toHaveBeenLastCalledWith(getProductRouteById('crew').path)
  })

  it('localizes the canonical empty chat title without changing stored data', async () => {
    await i18n.changeLanguage('de')
    useChatStore.setState({
      threads: [{
        id: 'thread-empty',
        title: 'New chat',
        messages: [],
        createdAt: 10,
        updatedAt: 10,
      }],
      activeThreadId: 'thread-empty',
    })

    render(
      <MemoryRouter>
        <LeftSidebar />
      </MemoryRouter>,
    )

    expect(screen.getByRole('button', { name: /^Neuer Chat$/ })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Chat löschen' })).toBeInTheDocument()
    expect(useChatStore.getState().threads[0]?.title).toBe('New chat')
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
