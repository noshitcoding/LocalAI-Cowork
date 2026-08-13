import { create } from 'zustand'
import type { ChatAttachment, ChatAttachmentKind } from '../utils/chatAttachments'
import { hasTauriRuntime, safeInvoke, safeInvokeVoid } from '../utils/safeInvoke'

const LEGACY_STORAGE_KEY = 'open-cowork-projects'
const SQLITE_MIGRATION_FLAG = 'open-cowork-projects-sqlite-migrated'

export type ProjectResourceKind = ChatAttachmentKind | 'link'
export type ProjectResourceAccess = 'read_only' | 'read_write'

export type ProjectResource = {
  id: string
  path: string
  kind: ProjectResourceKind
  label?: string
  enabled: boolean
  access: ProjectResourceAccess
  isPrimary: boolean
  addedAt: number
}

export type Project = {
  id: string
  title: string
  instructions: string
  resources: ProjectResource[]
  threadIds: string[]
  createdAt: number
  updatedAt: number
}

type AddProjectResourceInput = {
  path: string
  kind: ProjectResourceKind
  label?: string
  enabled?: boolean
  access?: ProjectResourceAccess
  isPrimary?: boolean
}

type DeleteProjectOptions = {
  deleteThreads?: boolean
}

type DbProjectResource = {
  id: string
  projectId?: string
  project_id?: string
  kind: string
  path: string
  label?: string | null
  enabled?: boolean
  access?: string
  isPrimary?: boolean
  is_primary?: boolean
  addedAt?: string
  added_at?: string
}

type DbProject = {
  id: string
  title: string
  instructions?: string
  resources?: DbProjectResource[]
  threadIds?: string[]
  thread_ids?: string[]
  createdAt?: string
  created_at?: string
  updatedAt?: string
  updated_at?: string
}

type RawProjectResource = {
  id?: unknown
  projectId?: unknown
  project_id?: unknown
  kind?: unknown
  path?: unknown
  label?: unknown
  enabled?: unknown
  access?: unknown
  isPrimary?: unknown
  is_primary?: unknown
  addedAt?: string | number
  added_at?: string | number
}

type RawProject = {
  id?: unknown
  title?: unknown
  instructions?: unknown
  resources?: unknown
  threadIds?: unknown
  thread_ids?: unknown
  createdAt?: string | number
  created_at?: string | number
  updatedAt?: string | number
  updated_at?: string | number
}

type ProjectState = {
  projects: Project[]
  activeProjectId: string | null
  loadFromDb: () => Promise<void>
  addProject: (title?: string, instructions?: string) => string
  renameProject: (projectId: string, title: string) => void
  updateProjectInstructions: (projectId: string, instructions: string) => void
  deleteProject: (projectId: string, options?: DeleteProjectOptions) => string[]
  setActiveProject: (projectId: string | null) => void
  addResources: (projectId: string, resources: AddProjectResourceInput[]) => void
  removeResource: (projectId: string, resourceId: string) => void
  setResourceEnabled: (projectId: string, resourceId: string, enabled: boolean) => void
  setResourceAccess: (projectId: string, resourceId: string, access: ProjectResourceAccess) => boolean
  setPrimaryResource: (projectId: string, resourceId: string) => void
  attachThread: (projectId: string, threadId: string) => void
  detachThread: (projectId: string, threadId: string) => void
  detachThreadFromAll: (threadId: string) => void
}

function generateId(prefix: string): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function parseDate(value: string | number | undefined, fallback = Date.now()): number {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string' && value.trim()) {
    const parsed = new Date(value).getTime()
    if (Number.isFinite(parsed)) return parsed
  }
  return fallback
}

function toIso(timestamp: number): string {
  return new Date(timestamp).toISOString()
}

function normalizeTitle(title: string | undefined, fallback: string): string {
  const trimmed = title?.trim()
  return trimmed || fallback
}

function normalizeResourceKind(kind: unknown): ProjectResourceKind {
  if (kind === 'folder') return 'folder'
  if (kind === 'link') return 'link'
  return 'file'
}

function normalizeResourceAccess(access: unknown, kind: ProjectResourceKind): ProjectResourceAccess {
  if (kind === 'link') return 'read_only'
  return access === 'read_only' ? 'read_only' : 'read_write'
}

function normalizeComparablePath(path: string): string {
  return path.trim().replace(/[\\/]+/g, '/').replace(/\/$/, '').toLowerCase()
}

function writableResourcesOverlap(left: ProjectResource, right: ProjectResource): boolean {
  if (left.kind === 'link' || right.kind === 'link') return false
  const leftPath = normalizeComparablePath(left.path)
  const rightPath = normalizeComparablePath(right.path)
  if (leftPath === rightPath) return true
  if (left.kind === 'file' || right.kind === 'file') return false
  return leftPath.startsWith(`${rightPath}/`) || rightPath.startsWith(`${leftPath}/`)
}

function ensurePrimaryResource(resources: ProjectResource[]): ProjectResource[] {
  const eligible = resources.filter((resource) => (
    resource.enabled && resource.kind !== 'link' && resource.access === 'read_write'
  ))
  const selectedId = eligible.find((resource) => resource.isPrimary)?.id ?? eligible[0]?.id ?? null
  return resources.map((resource) => ({
    ...resource,
    isPrimary: selectedId !== null && resource.id === selectedId,
  }))
}

function resourceKey(resource: Pick<ProjectResource, 'kind' | 'path'>): string {
  return `${resource.kind}::${resource.path.trim().toLowerCase()}`
}

function normalizeResource(raw: RawProjectResource): ProjectResource | null {
  if (typeof raw.path !== 'string' || !raw.path.trim()) return null
  const addedAt = parseDate(raw.addedAt ?? raw.added_at, Date.now())
  const label = typeof raw.label === 'string' && raw.label.trim() ? raw.label.trim() : undefined
  const kind = normalizeResourceKind(raw.kind)
  return {
    id: typeof raw.id === 'string' && raw.id.trim() ? raw.id : generateId('resource'),
    path: raw.path.trim(),
    kind,
    label,
    enabled: raw.enabled !== false,
    access: normalizeResourceAccess(raw.access, kind),
    isPrimary: raw.isPrimary === true || raw.is_primary === true,
    addedAt,
  }
}

function normalizeProject(raw: RawProject): Project | null {
  if (typeof raw.id !== 'string' || !raw.id.trim()) return null
  const createdAt = parseDate(raw.createdAt ?? raw.created_at, Date.now())
  const resources = ensurePrimaryResource(Array.isArray(raw.resources)
    ? raw.resources
        .map((resource) => normalizeResource(resource as RawProjectResource))
        .filter((resource): resource is ProjectResource => Boolean(resource))
    : [])
  const rawThreadIds = Array.isArray(raw.threadIds)
    ? raw.threadIds
    : Array.isArray(raw.thread_ids)
      ? raw.thread_ids
      : []
  const threadIds = Array.from(new Set(
    rawThreadIds.filter((id): id is string => typeof id === 'string' && id.trim().length > 0),
  ))

  return {
    id: raw.id,
    title: normalizeTitle(typeof raw.title === 'string' ? raw.title : undefined, 'Untitled project'),
    instructions: typeof raw.instructions === 'string' ? raw.instructions : '',
    resources,
    threadIds,
    createdAt,
    updatedAt: parseDate(raw.updatedAt ?? raw.updated_at, createdAt),
  }
}

function addUniqueResources(existing: ProjectResource[], incoming: AddProjectResourceInput[]): ProjectResource[] {
  const seen = new Set(existing.map(resourceKey))
  const now = Date.now()
  const accepted = [...existing]
  const additions = incoming
    .map((item): ProjectResource | null => {
      const normalized = normalizeResource({
        id: generateId('resource'),
        path: item.path,
        kind: item.kind,
        label: item.label,
        enabled: item.enabled,
        access: item.access,
        isPrimary: item.isPrimary,
        addedAt: now,
      })
      if (!normalized) return null
      const key = resourceKey(normalized)
      if (seen.has(key)) return null
      if (normalized.access === 'read_write' && accepted.some((resource) => (
        resource.access === 'read_write' && writableResourcesOverlap(resource, normalized)
      ))) return null
      seen.add(key)
      accepted.push(normalized)
      return normalized
    })
    .filter((item): item is ProjectResource => Boolean(item))

  return additions.length > 0 ? ensurePrimaryResource([...existing, ...additions]) : existing
}

function persistProject(project: Project): void {
  void safeInvokeVoid('project_upsert', {
    request: {
      id: project.id,
      title: project.title,
      instructions: project.instructions,
      createdAt: toIso(project.createdAt),
      updatedAt: toIso(project.updatedAt),
    },
  })
  mirrorProjectToDaemon(project)
}

export function projectMetadataForDaemon(project: Project): Record<string, unknown> {
  return {
    title: project.title,
    instructions: project.instructions,
    thread_ids: [...project.threadIds],
    project_kind: 'private',
    files_location: 'personal_device',
    created_at: toIso(project.createdAt),
    updated_at: toIso(project.updatedAt),
    source: 'desktop',
  }
}

function mirrorProjectToDaemon(project: Project): void {
  if (!hasTauriRuntime()) return
  void import('../runtime/localDaemonEntities')
    .then(({ mirrorDurableLocalEntity }) => (
      mirrorDurableLocalEntity('project', project.id, projectMetadataForDaemon(project))
    ))
    .catch((error) => console.warn('[projectStore] Daemon project mirror failed', error))
}

function tombstoneProjectInDaemon(projectId: string): void {
  if (!hasTauriRuntime()) return
  void import('../runtime/localDaemonEntities')
    .then(({ tombstoneDurableLocalEntity }) => tombstoneDurableLocalEntity('project', projectId))
    .catch((error) => console.warn('[projectStore] Daemon project tombstone failed', error))
}

function persistResource(projectId: string, resource: ProjectResource): void {
  void safeInvokeVoid('project_resource_upsert', {
    request: {
      id: resource.id,
      projectId,
      kind: resource.kind,
      path: resource.path,
      label: resource.label ?? null,
      enabled: resource.enabled,
      access: resource.access,
      isPrimary: resource.isPrimary,
      addedAt: toIso(resource.addedAt),
    },
  })
}

function parseLegacyStorage(): { projects: Project[]; activeProjectId: string | null } | null {
  if (typeof window === 'undefined') return null
  const raw = window.localStorage.getItem(LEGACY_STORAGE_KEY)
  if (!raw) return null

  try {
    const parsed = JSON.parse(raw) as { state?: unknown; projects?: unknown; activeProjectId?: unknown }
    const state = (parsed.state && typeof parsed.state === 'object' ? parsed.state : parsed) as {
      projects?: unknown
      activeProjectId?: unknown
    }
    const projects = Array.isArray(state.projects)
      ? state.projects
          .map((project) => normalizeProject(project as Partial<Project>))
          .filter((project): project is Project => Boolean(project))
      : []
    const activeProjectId = typeof state.activeProjectId === 'string'
      && projects.some((project) => project.id === state.activeProjectId)
      ? state.activeProjectId
      : projects[0]?.id ?? null
    return { projects, activeProjectId }
  } catch {
    return null
  }
}

async function migrateLegacyStorageToSqlite(): Promise<void> {
  if (!hasTauriRuntime() || typeof window === 'undefined') return
  if (window.localStorage.getItem(SQLITE_MIGRATION_FLAG) === 'true') return

  const legacy = parseLegacyStorage()
  if (!legacy || legacy.projects.length === 0) {
    window.localStorage.setItem(SQLITE_MIGRATION_FLAG, 'true')
    return
  }

  for (const project of legacy.projects) {
    await safeInvoke('project_upsert', {
      request: {
        id: project.id,
        title: project.title,
        instructions: project.instructions,
        createdAt: toIso(project.createdAt),
        updatedAt: toIso(project.updatedAt),
      },
    })

    for (const resource of project.resources) {
      await safeInvoke('project_resource_upsert', {
        request: {
          id: resource.id,
          projectId: project.id,
          kind: resource.kind,
          path: resource.path,
          label: resource.label ?? null,
          enabled: resource.enabled,
          access: resource.access,
          isPrimary: resource.isPrimary,
          addedAt: toIso(resource.addedAt),
        },
      })
    }

    for (const threadId of project.threadIds) {
      await safeInvoke('project_attach_thread', { projectId: project.id, threadId })
    }
  }

  window.localStorage.setItem(SQLITE_MIGRATION_FLAG, 'true')
}

function getLegacyFallbackProjects(): { projects: Project[]; activeProjectId: string | null } {
  return parseLegacyStorage() ?? { projects: [], activeProjectId: null }
}

export const useProjectStore = create<ProjectState>()((set, get) => ({
  projects: [],
  activeProjectId: null,

  loadFromDb: async () => {
    if (!hasTauriRuntime()) {
      const legacy = getLegacyFallbackProjects()
      set({
        projects: legacy.projects,
        activeProjectId: legacy.activeProjectId,
      })
      return
    }

    try {
      await migrateLegacyStorageToSqlite()
      const dbProjects = await safeInvoke<DbProject[]>('project_list', undefined, [])
      const projects = Array.isArray(dbProjects)
        ? dbProjects
            .map((project) => normalizeProject(project))
            .filter((project): project is Project => Boolean(project))
        : []
      set((state) => ({
        projects,
        activeProjectId: state.activeProjectId && projects.some((project) => project.id === state.activeProjectId)
          ? state.activeProjectId
          : projects[0]?.id ?? null,
      }))
    } catch (error) {
      console.warn('[projectStore] loadFromDb failed', error)
    }
  },

  addProject: (title, instructions = '') => {
    const now = Date.now()
    const id = generateId('project')
    const project: Project = {
      id,
      title: normalizeTitle(title, 'New project'),
      instructions,
      resources: [],
      threadIds: [],
      createdAt: now,
      updatedAt: now,
    }

    set((state) => ({
      projects: [project, ...state.projects],
      activeProjectId: id,
    }))
    persistProject(project)

    return id
  },

  renameProject: (projectId, title) => {
    let updatedProject: Project | null = null
    set((state) => ({
      projects: state.projects.map((project) => {
        if (project.id !== projectId) return project
        updatedProject = { ...project, title: normalizeTitle(title, project.title), updatedAt: Date.now() }
        return updatedProject
      }),
    }))
    if (updatedProject) persistProject(updatedProject)
  },

  updateProjectInstructions: (projectId, instructions) => {
    let updatedProject: Project | null = null
    set((state) => ({
      projects: state.projects.map((project) => {
        if (project.id !== projectId) return project
        updatedProject = { ...project, instructions, updatedAt: Date.now() }
        return updatedProject
      }),
    }))
    if (updatedProject) persistProject(updatedProject)
  },

  deleteProject: (projectId, options) => {
    let deletedThreadIds: string[] = []
    set((state) => {
      const project = state.projects.find((item) => item.id === projectId)
      deletedThreadIds = options?.deleteThreads ? project?.threadIds ?? [] : []
      const projects = state.projects.filter((item) => item.id !== projectId)
      return {
        projects,
        activeProjectId: state.activeProjectId === projectId ? projects[0]?.id ?? null : state.activeProjectId,
      }
    })
    void safeInvokeVoid('project_delete', {
      projectId,
      deleteThreads: Boolean(options?.deleteThreads),
    })
    tombstoneProjectInDaemon(projectId)
    return deletedThreadIds
  },

  setActiveProject: (projectId) =>
    set((state) => ({
      activeProjectId: projectId && state.projects.some((project) => project.id === projectId)
        ? projectId
        : null,
    })),

  addResources: (projectId, resources) => {
    let persistedResources: ProjectResource[] = []
    set((state) => ({
      projects: state.projects.map((project) => {
        if (project.id !== projectId) return project
        const nextResources = addUniqueResources(project.resources, resources)
        if (nextResources === project.resources) return project
        persistedResources = nextResources
        return {
          ...project,
          resources: nextResources,
          updatedAt: Date.now(),
        }
      }),
    }))
    persistedResources.forEach((resource) => persistResource(projectId, resource))
  },

  removeResource: (projectId, resourceId) => {
    let remainingResources: ProjectResource[] = []
    set((state) => {
      let removed = false
      const projects = state.projects.map((project) => {
        if (project.id !== projectId) return project
        const nextResources = ensurePrimaryResource(project.resources.filter((resource) => resource.id !== resourceId))
        removed = nextResources.length !== project.resources.length
        if (removed) remainingResources = nextResources
        return removed
          ? { ...project, resources: nextResources, updatedAt: Date.now() }
          : project
      })
      if (removed) void safeInvokeVoid('project_resource_delete', { resourceId })
      return { projects }
    })
    remainingResources.forEach((resource) => persistResource(projectId, resource))
  },

  setResourceEnabled: (projectId, resourceId, enabled) => {
    let persistedResources: ProjectResource[] = []
    set((state) => {
      let changed = false
      const projects = state.projects.map((project) => {
        if (project.id !== projectId) return project
        const resources = ensurePrimaryResource(project.resources.map((resource) => {
          if (resource.id !== resourceId || resource.enabled === enabled) return resource
          changed = true
          return { ...resource, enabled }
        }))
        if (changed) persistedResources = resources
        return {
          ...project,
          resources,
          updatedAt: Date.now(),
        }
      })
      return { projects }
    })
    persistedResources.forEach((resource) => persistResource(projectId, resource))
  },

  setResourceAccess: (projectId, resourceId, access) => {
    let accepted = true
    let persistedResources: ProjectResource[] = []
    set((state) => ({
      projects: state.projects.map((project) => {
        if (project.id !== projectId) return project
        const target = project.resources.find((resource) => resource.id === resourceId)
        if (!target || target.kind === 'link') return project
        const candidate = { ...target, access }
        if (access === 'read_write' && project.resources.some((resource) => (
          resource.id !== resourceId
          && resource.access === 'read_write'
          && writableResourcesOverlap(resource, candidate)
        ))) {
          accepted = false
          return project
        }
        const resources = ensurePrimaryResource(project.resources.map((resource) => (
          resource.id === resourceId ? candidate : resource
        )))
        persistedResources = resources
        return { ...project, resources, updatedAt: Date.now() }
      }),
    }))
    persistedResources.forEach((resource) => persistResource(projectId, resource))
    return accepted
  },

  setPrimaryResource: (projectId, resourceId) => {
    let persistedResources: ProjectResource[] = []
    set((state) => ({
      projects: state.projects.map((project) => {
        if (project.id !== projectId) return project
        const target = project.resources.find((resource) => resource.id === resourceId)
        if (!target || !target.enabled || target.kind === 'link' || target.access !== 'read_write') return project
        const resources = project.resources.map((resource) => ({
          ...resource,
          isPrimary: resource.id === resourceId,
        }))
        persistedResources = resources
        return { ...project, resources, updatedAt: Date.now() }
      }),
    }))
    persistedResources.forEach((resource) => persistResource(projectId, resource))
  },

  attachThread: (projectId, threadId) => {
    const normalizedThreadId = threadId.trim()
    if (!normalizedThreadId) return

    const changedProjectIds: string[] = []
    set((state) => ({
      projects: state.projects.map((project) => {
        const threadIds = project.threadIds.filter((id) => id !== normalizedThreadId)
        if (project.id !== projectId) {
          if (threadIds.length === project.threadIds.length) return project
          changedProjectIds.push(project.id)
          return { ...project, threadIds, updatedAt: Date.now() }
        }

        changedProjectIds.push(project.id)
        return {
          ...project,
          threadIds: [normalizedThreadId, ...threadIds],
          updatedAt: Date.now(),
        }
      }),
      activeProjectId: projectId,
    }))
    void safeInvokeVoid('project_attach_thread', { projectId, threadId: normalizedThreadId })
    changedProjectIds.forEach((id) => {
      const project = get().projects.find((entry) => entry.id === id)
      if (project) mirrorProjectToDaemon(project)
    })
  },

  detachThread: (projectId, threadId) => {
    let detached = false
    set((state) => {
      const projects = state.projects.map((project) => {
        if (project.id !== projectId) return project
        const threadIds = project.threadIds.filter((id) => id !== threadId)
        detached = threadIds.length !== project.threadIds.length
        return detached ? { ...project, threadIds, updatedAt: Date.now() } : project
      })
      if (detached) void safeInvokeVoid('project_detach_thread', { projectId, threadId })
      return { projects }
    })
    if (detached) {
      const project = get().projects.find((entry) => entry.id === projectId)
      if (project) mirrorProjectToDaemon(project)
    }
  },

  detachThreadFromAll: (threadId) => {
    const affectedProjectIds: string[] = []
    set((state) => {
      const projects = state.projects.map((project) => {
        if (!project.threadIds.includes(threadId)) return project
        affectedProjectIds.push(project.id)
        return {
          ...project,
          threadIds: project.threadIds.filter((id) => id !== threadId),
          updatedAt: Date.now(),
        }
      })
      affectedProjectIds.forEach((projectId) => {
        void safeInvokeVoid('project_detach_thread', { projectId, threadId })
      })
      return { projects }
    })
    affectedProjectIds.forEach((projectId) => {
      const project = get().projects.find((entry) => entry.id === projectId)
      if (project) mirrorProjectToDaemon(project)
    })
  },
}))

export function getProjectForThread(projects: Project[], threadId: string | null | undefined): Project | null {
  if (!threadId) return null
  return projects.find((project) => project.threadIds.includes(threadId)) ?? null
}

export function projectResourceToAttachment(resource: ProjectResource): ChatAttachment | null {
  if (resource.kind === 'link') return null
  return {
    path: resource.path,
    kind: resource.kind,
    label: resource.label,
    access: resource.access,
    isPrimary: resource.isPrimary,
  }
}

export function getEnabledProjectAttachments(project: Project | null | undefined): ChatAttachment[] {
  if (!project) return []
  return project.resources
    .filter((resource) => resource.enabled)
    .map(projectResourceToAttachment)
    .filter((attachment): attachment is ChatAttachment => Boolean(attachment))
}

export function getEnabledProjectLinks(project: Project | null | undefined): ProjectResource[] {
  if (!project) return []
  return project.resources.filter((resource) => resource.enabled && resource.kind === 'link')
}
