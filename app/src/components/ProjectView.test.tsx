import { fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import ProjectView from './ProjectView'
import { useChatStore } from '../stores/chatStore'
import { useProjectStore } from '../stores/projectStore'

const navigateMock = vi.fn()

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}))

vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom')
  return {
    ...actual,
    useNavigate: () => navigateMock,
  }
})

function renderProjectView() {
  return render(
    <MemoryRouter>
      <ProjectView />
    </MemoryRouter>,
  )
}

describe('ProjectView', () => {
  beforeEach(() => {
    navigateMock.mockReset()

    useChatStore.setState({
      threads: [
        {
          id: 'thread-1',
          title: 'Projektchat',
          messages: [],
          createdAt: 100,
          updatedAt: 100,
        },
      ],
      activeThreadId: 'thread-1',
      pendingApproval: [],
      busy: false,
      error: null,
    })

    useProjectStore.setState({
      projects: [
        {
          id: 'project-1',
          title: 'Alpha',
          instructions: 'Use short answers.',
          resources: [],
          threadIds: ['thread-1'],
          createdAt: 100,
          updatedAt: 100,
        },
      ],
      activeProjectId: 'project-1',
    })
  })

  it('edits project instructions', () => {
    renderProjectView()

    const textarea = screen.getByLabelText('Project instructions')
    fireEvent.change(textarea, { target: { value: 'Work strictly in the project context.' } })
    fireEvent.blur(textarea)

    expect(useProjectStore.getState().projects[0].instructions).toBe('Work strictly in the project context.')
  })

  it('adds link sources as project resources', () => {
    renderProjectView()

    fireEvent.change(screen.getByPlaceholderText('https://...'), {
      target: { value: 'https://example.com/spec' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Link' }))

    expect(screen.getAllByText('https://example.com/spec').length).toBeGreaterThan(0)
    expect(useProjectStore.getState().projects[0].resources).toEqual([
      expect.objectContaining({
        kind: 'link',
        path: 'https://example.com/spec',
        enabled: true,
      }),
    ])
  })

  it('can delete a project together with its chats', () => {
    renderProjectView()

    fireEvent.click(screen.getByTitle('Delete project'))
    expect(screen.getByRole('dialog', { name: 'Delete project' })).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Delete project and assigned chats' }))

    expect(useProjectStore.getState().projects).toEqual([])
    expect(useChatStore.getState().threads).toEqual([])
  })

  it('guides an empty workspace into a first project', () => {
    useProjectStore.setState({ projects: [], activeProjectId: null })
    renderProjectView()

    expect(screen.getByRole('heading', { name: 'Give focused work a permanent home' })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Create first project' }))

    expect(useProjectStore.getState().projects).toHaveLength(1)
    expect(screen.getByLabelText('Project name')).toHaveValue('Project 1')
  })
})
