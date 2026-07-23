import { beforeEach, describe, expect, it } from 'vitest'
import {
  getEnabledProjectAttachments,
  getEnabledProjectLinks,
  getProjectForThread,
  useProjectStore,
} from './projectStore'

describe('projectStore', () => {
  beforeEach(() => {
    window.localStorage.removeItem('open-cowork-projects')
    window.localStorage.removeItem('open-cowork-projects-sqlite-migrated')
    useProjectStore.setState({
      projects: [],
      activeProjectId: null,
    })
  })

  it('creates named projects and renames them', () => {
    const id = useProjectStore.getState().addProject('Customer analysis')
    useProjectStore.getState().renameProject(id, 'Customer analysis Q2')
    useProjectStore.getState().updateProjectInstructions(id, 'Use short answers.')

    const project = useProjectStore.getState().projects[0]
    expect(project.id).toBe(id)
    expect(project.title).toBe('Customer analysis Q2')
    expect(project.instructions).toBe('Use short answers.')
    expect(useProjectStore.getState().activeProjectId).toBe(id)
  })

  it('deduplicates resources and separates local attachments from links', () => {
    const id = useProjectStore.getState().addProject('Data')
    useProjectStore.getState().addResources(id, [
      { path: 'C:/docs/spec.md', kind: 'file' },
      { path: 'C:/docs/spec.md', kind: 'file' },
      { path: 'C:/repo', kind: 'folder', enabled: false },
      { path: 'https://example.com/spec', kind: 'link', label: 'Spec Link' },
    ])

    const project = useProjectStore.getState().projects[0]
    expect(project.resources).toHaveLength(3)
    expect(getEnabledProjectAttachments(project)).toEqual([
      { path: 'C:/docs/spec.md', kind: 'file', label: undefined },
    ])
    expect(getEnabledProjectLinks(project).map((resource) => resource.path)).toEqual([
      'https://example.com/spec',
    ])
  })

  it('moves chats between projects exclusively', () => {
    const firstId = useProjectStore.getState().addProject('Alpha')
    const secondId = useProjectStore.getState().addProject('Beta')

    useProjectStore.getState().attachThread(firstId, 'thread-1')
    useProjectStore.getState().attachThread(secondId, 'thread-1')

    const state = useProjectStore.getState()
    const first = state.projects.find((project) => project.id === firstId)
    const second = state.projects.find((project) => project.id === secondId)

    expect(first?.threadIds).toEqual([])
    expect(second?.threadIds).toEqual(['thread-1'])
    expect(getProjectForThread(state.projects, 'thread-1')?.id).toBe(secondId)
  })

  it('can detach chats from all projects and report deleted project threads', () => {
    const id = useProjectStore.getState().addProject('Alpha')
    useProjectStore.getState().attachThread(id, 'thread-1')
    useProjectStore.getState().detachThreadFromAll('thread-1')
    expect(useProjectStore.getState().projects[0].threadIds).toEqual([])

    useProjectStore.getState().attachThread(id, 'thread-2')
    const deletedThreadIds = useProjectStore.getState().deleteProject(id, { deleteThreads: true })
    expect(deletedThreadIds).toEqual(['thread-2'])
    expect(useProjectStore.getState().projects).toEqual([])
  })
})