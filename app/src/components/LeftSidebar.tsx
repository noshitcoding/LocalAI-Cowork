import { useEffect, useMemo, useRef, useState } from 'react'
import type { DragEvent, PointerEvent as ReactPointerEvent } from 'react'
import { useNavigate } from 'react-router-dom'
import type { SessionSummary } from '../engine'
import { useChatStore } from '../stores/chatStore'
import { useUiStore } from '../stores/uiStore'
import { useConfigStore } from '../stores/configStore'
import { useEngineStore } from '../stores/engineStore'
import { useTaskStore } from '../stores/taskStore'
import { useWorkTasksStore, type WorkTask, type WorkTaskStatus } from '../stores/workTasksStore'
import { useProjectStore } from '../stores/projectStore'
import { useCrewStore } from '../stores/crewStore'
import { resolveWorkTaskChatProviderSettings } from '../engine/tasks/workTaskExecutionService'
import { createChatProviderSelection, getChatProviderState } from '../utils/chatProvider'
import { ContextPanel, DocumentWorkspacePanel, OutputsPanel, ProgressPanel, WorkingFolderPanel } from './RightSidebar'
import { tr } from '../i18n'
import { Activity, ChevronDown, ChevronRight, FolderPlus, MessageSquarePlus, Plus, Trash2 } from 'lucide-react'
import { getProductRouteById, type ProductRouteId } from '../product/routeRegistry'

const THREAD_DND_MIME = 'application/open-cowork-thread-id'
const POINTER_DRAG_THRESHOLD = 5

type PointerThreadDrag = {
  threadId: string
  title: string
  pointerId: number
  startX: number
  startY: number
  x: number
  y: number
  active: boolean
}

type ThreadDropTarget =
  | { type: 'project'; projectId: string }
  | { type: 'chats' }

function isAbsolutePath(path: string): boolean {
  const trimmed = path.trim()
  return /^(?:[a-zA-Z]:[\\/]|\\\\|\/)/.test(trimmed)
}

function getTaskSidebarTitle(task: WorkTask): string {
  const title = task.title.trim()
  if (title) return title

  const prompt = task.prompt.trim().replace(/\s+/g, ' ')
  if (!prompt) return task.id
  return prompt.length > 36 ? `${prompt.slice(0, 36)}...` : prompt
}

function formatWorkTaskStatus(status: WorkTaskStatus): string {
  switch (status) {
    case 'idle': return tr('Idle')
    case 'waiting_approval': return tr('Waiting for approval')
    case 'running': return tr('Running')
    case 'completed': return tr('Completed')
    case 'failed': return tr('Failed')
    case 'canceled': return tr('Canceled')
  }
}

function buildTaskSidebarSummary(task: WorkTask): string {
  return [
    `${tr('Task created')}: ${getTaskSidebarTitle(task)}`,
    `${tr('Runner')}: ${task.runner === 'crew' ? tr('Crew') : tr('Model')}`,
    task.expectedOutput.trim() ? `${tr('Expected output')}: ${task.expectedOutput.trim()}` : '',
    task.workDir.trim() ? `${tr('Working folder')}: ${task.workDir.trim()}` : '',
  ].filter(Boolean).join('\n')
}

function readDraggedThreadId(event: DragEvent): string {
  const typed = event.dataTransfer.getData(THREAD_DND_MIME).trim()
  if (typed) return typed

  const plain = event.dataTransfer.getData('text/plain').trim()
  return plain.startsWith('thread:') ? plain.slice('thread:'.length).trim() : plain
}

function getThreadDropTarget(clientX: number, clientY: number): ThreadDropTarget | null {
  const element = document.elementFromPoint(clientX, clientY)
  const dropElement = element?.closest('[data-sidebar-project-id], [data-sidebar-chats-drop-zone="true"]') as HTMLElement | null
  if (!dropElement) return null

  const projectId = dropElement.dataset.sidebarProjectId
  if (projectId) return { type: 'project', projectId }
  if (dropElement.dataset.sidebarChatsDropZone === 'true') return { type: 'chats' }
  return null
}

export default function LeftSidebar() {
  const navigate = useNavigate()
  const {
    threads,
    activeThreadId,
    addThread,
    ensureThread,
    loadFromDb: loadChatFromDb,
    addMessage,
    setActiveThread,
    deleteThread,
  } = useChatStore()
  const setActiveMode = useUiStore((s) => s.setActiveMode)
  const setWorkingFolder = useUiStore((s) => s.setWorkingFolder)
  const ollama = useConfigStore((s) => s.ollama)
  const availableModels = useConfigStore((s) => s.availableModels)
  const llmProfiles = useConfigStore((s) => s.llmProfiles)
  const defaultLlmProfileIds = useConfigStore((s) => s.defaultLlmProfileIds)
  const llmProfileModels = useConfigStore((s) => s.llmProfileModels)
  const workTasks = useWorkTasksStore((s) => s.tasks)
  const updateWorkTask = useWorkTasksStore((s) => s.updateTask)
  const crews = useCrewStore((s) => s.crews)
  const tasks = useTaskStore((s) => s.tasks)
  const activeTaskId = useTaskStore((s) => s.activeTaskId)
  const activeProvider = useEngineStore((s) => s.activeProvider)
  const getSessions = useEngineStore((s) => s.getSessions)
  const loadSessionById = useEngineStore((s) => s.loadSessionById)
  const currentSessionId = useEngineStore((s) => s.currentSessionId)
  const deleteSessionById = useEngineStore((s) => s.deleteSessionById)
  const {
    projects,
    activeProjectId,
    addProject,
    setActiveProject,
    attachThread,
    detachThreadFromAll,
  } = useProjectStore()
  const [persistedSessions, setPersistedSessions] = useState<SessionSummary[]>([])
  const [loadingSessions, setLoadingSessions] = useState(false)
  const [collapsedProjectIds, setCollapsedProjectIds] = useState<Set<string>>(new Set())
  const [chatsCollapsed, setChatsCollapsed] = useState(false)
  const [tasksCollapsed, setTasksCollapsed] = useState(false)
  const [sessionsCollapsed, setSessionsCollapsed] = useState(false)
  const [workspaceStatusOpen, setWorkspaceStatusOpen] = useState(Boolean(activeTaskId))
  const [dropProjectId, setDropProjectId] = useState<string | null>(null)
  const [chatsDropActive, setChatsDropActive] = useState(false)
  const [pointerDrag, setPointerDrag] = useState<PointerThreadDrag | null>(null)
  const pointerDragRef = useRef<PointerThreadDrag | null>(null)
  const dragPreviewRef = useRef<HTMLDivElement | null>(null)
  const suppressThreadClickRef = useRef<string | null>(null)

  const activeThread = useMemo(
    () => threads.find((thread) => thread.id === activeThreadId),
    [activeThreadId, threads],
  )
  const providerState = useMemo(
    () => getChatProviderState({
      ollama,
      availableModels,
      llmProfiles,
      defaultLlmProfileIds,
      llmProfileModels,
    }, activeProvider, activeThread?.providerSettings),
    [activeProvider, activeThread?.providerSettings, availableModels, defaultLlmProfileIds, llmProfileModels, llmProfiles, ollama],
  )
  const activeTask = tasks.find((task) => task.id === activeTaskId)

  const navigateToProductRoute = (routeId: ProductRouteId) => {
    const route = getProductRouteById(routeId)
    if (route.activeMode) {
      setActiveMode(route.activeMode)
    }
    navigate(route.path)
  }

  useEffect(() => {
    let cancelled = false
    void (async () => {
      setLoadingSessions(true)
      try {
        const sessions = await getSessions()
        if (!cancelled) setPersistedSessions(sessions)
      } catch {
        if (!cancelled) setPersistedSessions([])
      } finally {
        if (!cancelled) setLoadingSessions(false)
      }
    })()
    return () => { cancelled = true }
  }, [getSessions, currentSessionId])

  const threadIds = useMemo(() => new Set(threads.map((thread) => thread.id)), [threads])
  const threadById = useMemo(() => new Map(threads.map((thread) => [thread.id, thread])), [threads])
  const taskThreadIds = useMemo(
    () => new Set(workTasks.map((task) => task.threadId).filter((threadId): threadId is string => typeof threadId === 'string' && threadId.trim().length > 0)),
    [workTasks],
  )
  const projectThreadIds = useMemo(
    () => new Set(projects.flatMap((project) => project.threadIds)),
    [projects],
  )
  const unassignedThreads = useMemo(
    () => threads.filter((thread) => !taskThreadIds.has(thread.id) && !projectThreadIds.has(thread.id)),
    [projectThreadIds, taskThreadIds, threads],
  )

  useEffect(() => {
    const preview = dragPreviewRef.current
    if (!preview || !pointerDrag?.active) return
    preview.style.transform = `translate3d(${pointerDrag.x + 12}px, ${pointerDrag.y + 10}px, 0)`
  }, [pointerDrag?.active, pointerDrag?.x, pointerDrag?.y])

  useEffect(() => {
    if (!pointerDrag) return undefined

    const handlePointerMove = (event: PointerEvent) => {
      const current = pointerDragRef.current
      if (!current || event.pointerId !== current.pointerId) return

      const distance = Math.hypot(event.clientX - current.startX, event.clientY - current.startY)
      const active = current.active || distance >= POINTER_DRAG_THRESHOLD
      const next = {
        ...current,
        x: event.clientX,
        y: event.clientY,
        active,
      }
      pointerDragRef.current = next
      setPointerDrag(next)

      if (!active) return
      event.preventDefault()
      const target = getThreadDropTarget(event.clientX, event.clientY)
      setDropProjectId(target?.type === 'project' ? target.projectId : null)
      setChatsDropActive(target?.type === 'chats')
    }

    const finishPointerDrag = (event: PointerEvent) => {
      const current = pointerDragRef.current
      if (!current || event.pointerId !== current.pointerId) return

      if (current.active && threadIds.has(current.threadId)) {
        suppressThreadClickRef.current = current.threadId
        const target = getThreadDropTarget(event.clientX, event.clientY)
        if (target?.type === 'project') {
          moveThreadToProject(target.projectId, current.threadId)
        } else if (target?.type === 'chats') {
          detachThreadFromAll(current.threadId)
        }
      }

      pointerDragRef.current = null
      setPointerDrag(null)
      clearThreadDropState()
    }

    const cancelPointerDrag = () => {
      pointerDragRef.current = null
      setPointerDrag(null)
      clearThreadDropState()
    }

    window.addEventListener('pointermove', handlePointerMove, { passive: false })
    window.addEventListener('pointerup', finishPointerDrag)
    window.addEventListener('pointercancel', cancelPointerDrag)
    return () => {
      window.removeEventListener('pointermove', handlePointerMove)
      window.removeEventListener('pointerup', finishPointerDrag)
      window.removeEventListener('pointercancel', cancelPointerDrag)
    }
    // Drag handlers close over the active store actions for the current gesture.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detachThreadFromAll, pointerDrag, threadIds])

  const handleNewChat = (projectId?: string) => {
    const threadId = addThread('New chat', createChatProviderSelection(providerState))
    if (projectId) {
      attachThread(projectId, threadId)
      setActiveProject(projectId)
    }
    setActiveThread(threadId)
    navigateToProductRoute('cowork')
  }

  const handleNewProject = () => {
    const projectId = addProject(`Project ${projects.length + 1}`)
    setActiveProject(projectId)
    navigateToProductRoute('projects')
  }

  const handleOpenProjects = () => {
    if (!activeProjectId && projects[0]) {
      setActiveProject(projects[0].id)
    }
    navigateToProductRoute('projects')
  }

  const handleOpenProject = (projectId: string) => {
    setActiveProject(projectId)
    navigateToProductRoute('projects')
  }

  const handleOpenThread = (threadId: string) => {
    if (suppressThreadClickRef.current === threadId) {
      suppressThreadClickRef.current = null
      return
    }
    setActiveThread(threadId)
    navigateToProductRoute('cowork')
  }

  const handleOpenTaskThread = async (task: WorkTask) => {
    await loadChatFromDb()
    const loadedThreadIds = new Set(useChatStore.getState().threads.map((thread) => thread.id))
    const existingThreadId = task.threadId && loadedThreadIds.has(task.threadId)
      ? task.threadId
      : null
    const taskProviderSettings = resolveWorkTaskChatProviderSettings(task, {
      crews,
      ollamaModel: ollama.model,
      defaultLlmProfileIds,
      llmProfiles,
      fallbackProviderSettings: createChatProviderSelection(providerState),
    })

    const ensuredThread = existingThreadId
      ? { id: existingThreadId, created: false }
      : task.threadId
        ? ensureThread(task.threadId, getTaskSidebarTitle(task), taskProviderSettings)
        : { id: addThread(getTaskSidebarTitle(task), taskProviderSettings), created: true }
    const threadId = ensuredThread.id

    if (ensuredThread.created) {
      addMessage(threadId, {
        role: 'system',
        content: buildTaskSidebarSummary(task),
        visibleInChat: true,
        timestamp: Date.now(),
      })
      if (!task.threadId) {
        updateWorkTask(task.id, { threadId })
      }
    }

    const workDir = task.workDir.trim()
    setWorkingFolder(workDir && isAbsolutePath(workDir) ? workDir : null)
    setActiveThread(threadId)
    navigateToProductRoute('cowork')
  }

  const handleOpenSession = async (sessionId: string) => {
    const session = await loadSessionById(sessionId)
    if (!session) return
    if (session.threadId && threadIds.has(session.threadId)) {
      setActiveThread(session.threadId)
    }
    navigateToProductRoute('cowork')
  }

  const toggleProjectCollapsed = (projectId: string) => {
    setCollapsedProjectIds((current) => {
      const next = new Set(current)
      if (next.has(projectId)) {
        next.delete(projectId)
      } else {
        next.add(projectId)
      }
      return next
    })
  }

  const clearThreadDropState = () => {
    setDropProjectId(null)
    setChatsDropActive(false)
  }

  const moveThreadToProject = (projectId: string, threadId: string) => {
    attachThread(projectId, threadId)
    setActiveProject(projectId)
    setCollapsedProjectIds((current) => {
      if (!current.has(projectId)) return current
      const next = new Set(current)
      next.delete(projectId)
      return next
    })
  }

  const handleThreadDragStart = (event: DragEvent, threadId: string) => {
    event.stopPropagation()
    event.dataTransfer.setData(THREAD_DND_MIME, threadId)
    event.dataTransfer.setData('text/plain', `thread:${threadId}`)
    event.dataTransfer.effectAllowed = 'move'
  }

  const handleThreadPointerDown = (
    event: ReactPointerEvent<HTMLElement>,
    threadId: string,
    title: string,
  ) => {
    if (event.button !== 0) return
    const target = event.target
    if (target instanceof HTMLElement && target.closest('.sidebar-row-action')) return

    const next = {
      threadId,
      title,
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      x: event.clientX,
      y: event.clientY,
      active: false,
    }
    pointerDragRef.current = next
    setPointerDrag(next)
  }

  const handleThreadDrop = (event: DragEvent, onDropThread: (threadId: string) => void) => {
    event.preventDefault()
    event.stopPropagation()
    const threadId = readDraggedThreadId(event)
    if (threadId && threadIds.has(threadId)) {
      onDropThread(threadId)
    }
    clearThreadDropState()
  }

  const handleProjectDragOver = (event: DragEvent, projectId: string) => {
    event.preventDefault()
    event.stopPropagation()
    event.dataTransfer.dropEffect = 'move'
    setDropProjectId(projectId)
    setChatsDropActive(false)
  }

  const handleProjectDragLeave = (event: DragEvent, projectId: string) => {
    const relatedTarget = event.relatedTarget
    if (relatedTarget instanceof Node && event.currentTarget.contains(relatedTarget)) return
    setDropProjectId((current) => (current === projectId ? null : current))
  }

  const handleChatsDragOver = (event: DragEvent) => {
    event.preventDefault()
    event.stopPropagation()
    event.dataTransfer.dropEffect = 'move'
    setChatsDropActive(true)
    setDropProjectId(null)
  }

  const handleChatsDragLeave = (event: DragEvent) => {
    const relatedTarget = event.relatedTarget
    if (relatedTarget instanceof Node && event.currentTarget.contains(relatedTarget)) return
    setChatsDropActive(false)
  }

  return (
    <aside className="left-sidebar" data-doc-id="element:/app/left-sidebar" aria-label={tr("Workspace sidebar")}>
      <div className="sidebar-primary-actions">
        <button type="button" className="sidebar-primary-action" aria-label={tr("+ New chat")} data-doc-id="button:/app/left-sidebar/new-chat" onClick={() => handleNewChat()}>
          <MessageSquarePlus size={17} aria-hidden="true" />
          <span>{tr("New chat")}</span>
        </button>
        <button type="button" className="sidebar-primary-action" aria-label={tr("+ New project")} data-doc-id="button:/app/left-sidebar/new-project" onClick={handleNewProject}>
          <FolderPlus size={17} aria-hidden="true" />
          <span>{tr("New project")}</span>
        </button>
      </div>
      <div className="sidebar-utility-actions">
        <button type="button" className="sidebar-utility-link" data-doc-id="button:/app/left-sidebar/manage-projects" onClick={handleOpenProjects}>{tr("Manage projects")}</button>
        <button type="button" className="sidebar-utility-link" data-doc-id="button:/app/left-sidebar/open-crew" onClick={() => navigateToProductRoute('crew')}>{tr("Crew Studio")}</button>
      </div>

      <div className={`sidebar-status-group${workspaceStatusOpen ? ' open' : ''}`}>
        <button
          type="button"
          className="sidebar-status-toggle"
          aria-expanded={workspaceStatusOpen}
          onClick={() => setWorkspaceStatusOpen((open) => !open)}
        >
          <span><Activity size={15} aria-hidden="true" />{tr("Workspace status")}</span>
          {workspaceStatusOpen ? <ChevronDown size={15} aria-hidden="true" /> : <ChevronRight size={15} aria-hidden="true" />}
        </button>
        {workspaceStatusOpen && (
          <div className="sidebar-status-panels">
            <ProgressPanel task={activeTask} />
            <DocumentWorkspacePanel />
            <WorkingFolderPanel />
            <OutputsPanel task={activeTask} />
            <ContextPanel />
          </div>
        )}
      </div>

      <div className="sidebar-section">
        <div className="sidebar-group">
          <div className="sidebar-group-title">
            <span>{tr("Projects")}</span>
            <button type="button" onClick={handleNewProject} title={tr("New project")} aria-label={tr("New project")}>
              <Plus size={14} aria-hidden="true" />
            </button>
          </div>
          {projects.map((project) => {
            const projectThreads = project.threadIds
              .map((threadId) => threadById.get(threadId))
              .filter((thread): thread is NonNullable<typeof thread> => Boolean(thread))
            const collapsed = collapsedProjectIds.has(project.id)
            const active = activeProjectId === project.id
            return (
              <div
                key={project.id}
                className={`sidebar-project${active ? ' active' : ''}${dropProjectId === project.id ? ' drop-active' : ''}`}
                data-sidebar-project-id={project.id}
                onDragOver={(event) => handleProjectDragOver(event, project.id)}
                onDragLeave={(event) => handleProjectDragLeave(event, project.id)}
                onDrop={(event) => handleThreadDrop(event, (threadId) => moveThreadToProject(project.id, threadId))}
              >
                <div className="sidebar-project-header">
                  <button
                    type="button"
                    className="sidebar-project-caret"
                    aria-label={collapsed ? tr("Expand project") : tr("Collapse project")}
                    aria-expanded={!collapsed}
                    onClick={() => toggleProjectCollapsed(project.id)}
                  >
                    {collapsed ? <ChevronRight size={14} aria-hidden="true" /> : <ChevronDown size={14} aria-hidden="true" />}
                  </button>
                  <button type="button" className="sidebar-project-main" onClick={() => handleOpenProject(project.id)}>
                    <span className="sidebar-project-name">{project.title}</span>
                    <span className="sidebar-project-count">{projectThreads.length}</span>
                  </button>
                  <button type="button" className="sidebar-row-action" onClick={() => handleNewChat(project.id)} title={tr("Start project chat")} aria-label={tr("Start project chat")}>
                    <Plus size={14} aria-hidden="true" />
                  </button>
                </div>
                {!collapsed && (
                  <div className="sidebar-thread-list">
                    {projectThreads.map((thread) => (
                      <div
                        key={thread.id}
                        className={`sidebar-thread-row thread-draggable${thread.id === activeThreadId ? ' active' : ''}`}
                        onDragStart={(event) => handleThreadDragStart(event, thread.id)}
                        onDragEnd={clearThreadDropState}
                        onPointerDown={(event) => handleThreadPointerDown(event, thread.id, thread.title)}
                        title={tr("Move chat to another project")}
                      >
                        <button
                          type="button"
                          className="sidebar-row-main"
                          onDragStart={(event) => handleThreadDragStart(event, thread.id)}
                          onDragEnd={clearThreadDropState}
                          onClick={() => handleOpenThread(thread.id)}
                        >
                          {thread.title}
                        </button>
                      </div>
                    ))}
                    {projectThreads.length === 0 && <p className="hint-text">{tr("No chats in this project")}</p>}
                  </div>
                )}
              </div>
            )
          })}
          {projects.length === 0 && <p className="hint-text">{tr("No projects")}</p>}
        </div>

        <div
          className={`sidebar-group${chatsDropActive ? ' drop-active' : ''}`}
          data-sidebar-chats-drop-zone="true"
          onDragOver={handleChatsDragOver}
          onDragLeave={handleChatsDragLeave}
          onDrop={(event) => handleThreadDrop(event, detachThreadFromAll)}
        >
          <button
            type="button"
            className="sidebar-group-toggle"
            aria-expanded={!chatsCollapsed}
            onClick={() => setChatsCollapsed((value) => !value)}
          >
            <span>{chatsCollapsed ? <ChevronRight size={12} aria-hidden="true" /> : <ChevronDown size={12} aria-hidden="true" />}{tr("Chats")}</span>
            <span>{unassignedThreads.length}</span>
          </button>
          {!chatsCollapsed && (
            <div className="sidebar-thread-list">
              {unassignedThreads.map((thread) => (
                <div
                  key={thread.id}
                  className={`sidebar-thread-row thread-draggable${thread.id === activeThreadId ? ' active' : ''}`}
                  onDragStart={(event) => handleThreadDragStart(event, thread.id)}
                  onDragEnd={clearThreadDropState}
                  onPointerDown={(event) => handleThreadPointerDown(event, thread.id, thread.title)}
                  title={tr("Move chat to a project")}
                >
                  <button
                    type="button"
                    className="sidebar-row-main"
                    onDragStart={(event) => handleThreadDragStart(event, thread.id)}
                    onDragEnd={clearThreadDropState}
                    onClick={() => handleOpenThread(thread.id)}
                  >
                    {thread.title}
                  </button>
                  <button
                    type="button"
                    className="sidebar-row-action"
                    draggable={false}
                    onClick={(event) => { event.stopPropagation(); deleteThread(thread.id) }}
                    title={tr("Delete")}
                    aria-label={tr("Delete chat")}
                  ><Trash2 size={14} aria-hidden="true" /></button>
                </div>
              ))}
              {unassignedThreads.length === 0 && <p className="hint-text">{tr("No unassigned chats")}</p>}
            </div>
          )}
        </div>

        <div className="sidebar-group">
          <button type="button" className="sidebar-group-toggle" aria-expanded={!tasksCollapsed} onClick={() => setTasksCollapsed((value) => !value)}>
            <span>{tasksCollapsed ? <ChevronRight size={12} aria-hidden="true" /> : <ChevronDown size={12} aria-hidden="true" />}{tr("Tasks")}</span>
            <span>{workTasks.length}</span>
          </button>
          {!tasksCollapsed && (
            <div className="sidebar-thread-list">
              {workTasks.map((task) => (
                <div key={task.id} className={`sidebar-thread-row${task.threadId === activeThreadId ? ' active' : ''}`}>
                  <button type="button" className="sidebar-row-main sidebar-task-row-main" data-doc-id="button:/app/left-sidebar/open-task-chat" onClick={() => void handleOpenTaskThread(task)}>
                    <span className="sidebar-task-title">{getTaskSidebarTitle(task)}</span>
                    <span className={`task-pill task-status task-status-${task.status}`}>{formatWorkTaskStatus(task.status)}</span>
                  </button>
                </div>
              ))}
              {workTasks.length === 0 && <p className="hint-text">{tr("No tasks")}</p>}
            </div>
          )}
        </div>

        <div className="sidebar-group">
          <button type="button" className="sidebar-group-toggle" aria-expanded={!sessionsCollapsed} onClick={() => setSessionsCollapsed((value) => !value)}>
            <span>{sessionsCollapsed ? <ChevronRight size={12} aria-hidden="true" /> : <ChevronDown size={12} aria-hidden="true" />}{tr("Sessions")}</span>
            <span>{persistedSessions.length}</span>
          </button>
          {!sessionsCollapsed && (
            <div className="sidebar-thread-list">
              {persistedSessions.map((session) => (
                <div key={session.id} className={`sidebar-thread-row${session.id === currentSessionId ? ' active' : ''}`}>
                  <button type="button" className="sidebar-row-main" onClick={() => void handleOpenSession(session.id)}>
                    {session.title}
                  </button>
                  <button
                    type="button"
                    className="sidebar-row-action"
                    onClick={(event) => { event.stopPropagation(); void deleteSessionById(session.id) }}
                    title={tr("Delete")}
                    aria-label={tr("Delete session")}
                  ><Trash2 size={14} aria-hidden="true" /></button>
                </div>
              ))}
              {loadingSessions && <p className="hint-text">{tr("Loading sessions...")}</p>}
              {!loadingSessions && persistedSessions.length === 0 && <p className="hint-text">{tr("No persisted sessions")}</p>}
            </div>
          )}
        </div>
      </div>
      {pointerDrag?.active && (
        <div
          ref={dragPreviewRef}
          className="sidebar-drag-preview"
        >
          {pointerDrag.title}
        </div>
      )}
    </aside>
  )
}
