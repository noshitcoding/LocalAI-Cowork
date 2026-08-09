import { open } from '@tauri-apps/plugin-dialog'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { useChatStore, type CrewLiveState, type CrewLiveStatus } from '../stores/chatStore'
import { useConfigStore } from '../stores/configStore'
import { useCoworkStore, type ScheduledTask } from '../stores/coworkStore'
import { resolveCrewAgentsWithProfiles, useCrewStore, type CrewPersonalityProfile } from '../stores/crewStore'
import { usePersonalityStore } from '../stores/personalityStore'
import { getProjectForThread, useProjectStore } from '../stores/projectStore'
import { useTaskTemplatesStore } from '../stores/taskTemplatesStore'
import { useUiStore } from '../stores/uiStore'
import { useWorkTasksStore, type WorkTask, type WorkTaskRunner } from '../stores/workTasksStore'
import { useEngineStore } from '../stores/engineStore'
import type { EngineEvent } from '../engine/core/queryEngine'
import { extractTextContent } from '../engine/types'
import i18n, { tr } from '../i18n'
import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'
import { getChatProviderState } from '../utils/chatProvider'
import {
  createDurableLocalRun,
  createDurableCrewRun,
  createDurableCodexRun,
  deleteDurableLocalSchedule,
  upsertDurableCrewSchedule,
  upsertDurableCodexSchedule,
  upsertDurableLocalSchedule,
} from '../runtime/localDaemonExecution'
import { attachDurableLocalRun, cancelLatestDurableRun } from '../runtime/localDaemonChat'
import { useCodexStore } from '../stores/codexStore'
import {
  appendCrewLiveEntry,
  applyCrewDefaultModel,
  augmentCrewToolsForTask,
  buildCrewLiveMessageContent,
  buildCrewRuntimeTasks,
  buildWorkTaskCrewGuidelines,
  createCrewLiveEntry,
  resolveEffectiveCrewProvider,
  resolveCrewRuntimeConfig,
  resolveExternalProviderConfig,
  type CrewExecutionLog,
  type CrewExecutionResponse,
  type CrewResolvedProviderConfigs,
} from '../engine/crew/workTaskCrewRuntime'
import {
  buildCrewRunOutput,
  buildCrewMissionDraft,
  buildCrewMissionId,
  buildCrewMissionTask,
  buildTaskPromptMessage,
  buildTaskThreadSummary,
  createCrewStreamId,
  deriveTaskName,
  isAbsolutePath,
  resolveWorkTaskChatProviderSettings,
} from '../engine/tasks/workTaskExecutionService'
import {
  findScheduledTask,
  readCrewScheduleSnapshotMetadata,
  resolveCrewScheduleSource,
} from '../engine/tasks/workTaskScheduleService'
import {
  appendTaskProjectPrompt,
  resolveTaskProjectRunContext,
  type TaskProjectRunContext,
} from '../utils/taskProjectContext'
import TaskCreatePanel from './tasks/TaskCreatePanel'
import TaskDetailPane from './tasks/TaskDetailPane'
import TaskListPane from './tasks/TaskListPane'

export default function TasksView() {
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const crews = useCrewStore((s) => s.crews)
  const personalities = usePersonalityStore((s) => s.personalities)
  const loadPersonalities = usePersonalityStore((s) => s.loadPersonalities)
  const { tasks, addTask, updateTask, removeTask, upsertMany } = useWorkTasksStore()
  const addThread = useChatStore((s) => s.addThread)
  const ensureThread = useChatStore((s) => s.ensureThread)
  const loadChatFromDb = useChatStore((s) => s.loadFromDb)
  const activeThreadId = useChatStore((s) => s.activeThreadId)
  const threads = useChatStore((s) => s.threads)
  const setActiveThread = useChatStore((s) => s.setActiveThread)
  const addChatMessage = useChatStore((s) => s.addMessage)
  const updateChatMessage = useChatStore((s) => s.updateMessage)
  const projects = useProjectStore((s) => s.projects)
  const activeProjectId = useProjectStore((s) => s.activeProjectId)
  const attachProjectThread = useProjectStore((s) => s.attachThread)
  const detachProjectThreadFromAll = useProjectStore((s) => s.detachThreadFromAll)
  const setActiveMode = useUiStore((s) => s.setActiveMode)
  const setWorkingFolder = useUiStore((s) => s.setWorkingFolder)
  const sendEngineMessage = useEngineStore((s) => s.sendMessage)
  const abortEngine = useEngineStore((s) => s.abort)

  const templates = useTaskTemplatesStore((s) => s.templates)
  const removeTemplate = useTaskTemplatesStore((s) => s.removeTemplate)

  const {
    scheduledTasks,
    policyFlags,
    loadScheduledTasks,
    upsertScheduledTask,
    toggleScheduledTask,
    removeScheduledTask,
  } = useCoworkStore()

  const ollamaConfig = useConfigStore((s) => s.ollama)
  const defaultLlmProfileIds = useConfigStore((s) => s.defaultLlmProfileIds)
  const llmProfileModels = useConfigStore((s) => s.llmProfileModels)
  const llmProfiles = useConfigStore((s) => s.llmProfiles)
  const availableModels = useConfigStore((s) => s.availableModels)
  const mcpServer = useConfigStore((s) => s.mcpServer)
  const mcpServers = useConfigStore((s) => s.mcpServers)

  const personalityProfiles = useMemo<CrewPersonalityProfile[]>(() => (
    personalities.map((personality) => ({
      id: personality.id,
      name: personality.name,
      description: personality.description,
      role: personality.role,
      goal: personality.goal || personality.description,
      systemPrompt: personality.system_prompt,
      skillsMarkdown: personality.skills_markdown,
      modelOverride: personality.model_override,
      temperature: personality.temperature,
      icon: personality.icon,
      isDefault: personality.is_default,
    }))
  ), [personalities])

  const [newTitle, setNewTitle] = useState('')
  const [newPrompt, setNewPrompt] = useState('')
  const [newExpectedOutput, setNewExpectedOutput] = useState('')
  const [newWorkDir, setNewWorkDir] = useState('')
  const [newRunner, setNewRunner] = useState<WorkTaskRunner>('crew')
  const [newCrewId, setNewCrewId] = useState<string>('')
  const [newModel, setNewModel] = useState<string>('')
  const [newUseProjectContext, setNewUseProjectContext] = useState(false)
  const [newProjectId, setNewProjectId] = useState<string>('')
  const [createPanelOpen, setCreatePanelOpen] = useState(tasks.length === 0)
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null)
  const [importCrewId, setImportCrewId] = useState<string>('')
  const [pendingCrewMissionId, setPendingCrewMissionId] = useState<string | null>(null)
  const runningTaskControllersRef = useRef(new Map<string, AbortController>())
  const runningCrewTaskIdsRef = useRef(new Map<string, string>())
  const canceledTaskIdsRef = useRef(new Set<string>())
  const handledCrewHandoffRef = useRef<string | null>(null)

  const normalizedNewWorkDir = newWorkDir.trim()
  const canCreateTask = newPrompt.trim().length > 0
    && (newRunner !== 'crew' || Boolean(newCrewId))
    && (!normalizedNewWorkDir || isAbsolutePath(normalizedNewWorkDir))

  useEffect(() => {
    void loadScheduledTasks()
  }, [loadScheduledTasks])

  useEffect(() => {
    void loadPersonalities()
  }, [loadPersonalities])

  useEffect(() => {
    if (newRunner !== 'crew') return
    if (newCrewId && crews.some((crew) => crew.id === newCrewId)) return
    setNewCrewId(crews[0]?.id ?? '')
  }, [crews, newCrewId, newRunner])

  useEffect(() => {
    if (newProjectId && projects.some((project) => project.id === newProjectId)) return
    const preferredProjectId = activeProjectId && projects.some((project) => project.id === activeProjectId)
      ? activeProjectId
      : projects[0]?.id ?? ''
    setNewProjectId(preferredProjectId)
    if (!preferredProjectId) setNewUseProjectContext(false)
  }, [activeProjectId, newProjectId, projects])

  useEffect(() => {
    if (importCrewId && crews.some((crew) => crew.id === importCrewId)) return
    setImportCrewId(crews[0]?.id ?? '')
  }, [crews, importCrewId])

  useEffect(() => {
    if (tasks.length === 0) setCreatePanelOpen(true)
  }, [tasks.length])

  useEffect(() => {
    const handoffCrewId = searchParams.get('crew')?.trim() ?? ''
    if (!handoffCrewId || handledCrewHandoffRef.current === handoffCrewId) return

    const crew = crews.find((entry) => entry.id === handoffCrewId)
    if (!crew) return

    handledCrewHandoffRef.current = handoffCrewId
    setImportCrewId(crew.id)
    const existingMission = tasks.find((task) => task.id === buildCrewMissionId(crew.id))
    if (existingMission) {
      setSelectedTaskId(existingMission.id)
    } else {
      const mission = buildCrewMissionDraft(crew)
      setPendingCrewMissionId(crew.id)
      setCreatePanelOpen(true)
      setNewRunner('crew')
      setNewCrewId(crew.id)
      setNewTitle(mission.title)
      setNewPrompt(mission.prompt)
      setNewExpectedOutput(mission.expectedOutput)
    }

    const nextSearchParams = new URLSearchParams(searchParams)
    nextSearchParams.delete('crew')
    setSearchParams(nextSearchParams, { replace: true })
  }, [crews, searchParams, setSearchParams, tasks])

  useEffect(() => {
    if (searchParams.has('crew')) return
    const linkedTaskId = searchParams.get('task')?.trim() ?? ''
    if (!linkedTaskId) return

    const linkedTask = tasks.find((task) => task.id === linkedTaskId)
    if (!linkedTask) {
      if (tasks.length === 0) return
      const nextSearchParams = new URLSearchParams(searchParams)
      nextSearchParams.delete('task')
      setSearchParams(nextSearchParams, { replace: true })
      return
    }

    setSelectedTaskId(linkedTask.id)
    const nextSearchParams = new URLSearchParams(searchParams)
    nextSearchParams.delete('task')
    setSearchParams(nextSearchParams, { replace: true })
  }, [searchParams, setSearchParams, tasks])

  useEffect(() => {
    if (searchParams.has('task')) return
    if (selectedTaskId && tasks.some((task) => task.id === selectedTaskId)) return
    setSelectedTaskId(tasks[0]?.id ?? null)
  }, [searchParams, selectedTaskId, tasks])

  useEffect(() => {
    if (tasks.length === 0) return
    for (const task of tasks) {
      const scheduled = findScheduledTask(scheduledTasks, task.id)
      if (!scheduled) {
        if (task.scheduleEnabled) {
          updateTask(task.id, { scheduleEnabled: false })
        }
        continue
      }

      const nextPatch: Partial<Omit<WorkTask, 'id' | 'createdAt'>> = {}
      const scheduledExpr = scheduled.cronLike.trim()
      if (scheduledExpr && task.scheduleExpr !== scheduledExpr) {
        nextPatch.scheduleExpr = scheduledExpr
      }
      if (task.scheduleEnabled !== scheduled.active) {
        nextPatch.scheduleEnabled = scheduled.active
      }

      const patchEntries = Object.keys(nextPatch)
      if (patchEntries.length > 0) {
        updateTask(task.id, nextPatch)
      }
    }
  }, [scheduledTasks, tasks, updateTask])

  useEffect(() => {
    // One-way migration helper: import legacy templates as runnable tasks.
    if (tasks.length > 0) return
    if (templates.length === 0) return

    const migrated: WorkTask[] = templates.map((template) => ({
      id: template.id,
      title: template.title ?? '',
      prompt: template.description ?? '',
      expectedOutput: template.expectedOutput ?? '',
      workDir: '',
      threadId: null,
      runner: 'model',
      crewId: null,
      model: '',
      scheduleExpr: '',
      scheduleEnabled: false,
      status: 'idle',
      output: null,
      error: null,
      lastRunAt: null,
      createdAt: Date.now(),
      updatedAt: Date.now(),
    }))

    upsertMany(migrated)
  }, [tasks.length, templates, upsertMany])

  const crewsById = useMemo(() => new Map(crews.map((crew) => [crew.id, crew])), [crews])
  const selectedTask = selectedTaskId ? tasks.find((task) => task.id === selectedTaskId) ?? null : tasks[0] ?? null
  const selectedScheduledTask = selectedTask ? findScheduledTask(scheduledTasks, selectedTask.id) : null
  const selectedProjectContext = useMemo(() => {
    if (!selectedTask?.threadId) return null
    const project = getProjectForThread(projects, selectedTask.threadId)
    return project ? { id: project.id, title: project.title } : null
  }, [projects, selectedTask?.threadId])

  const createTaskThread = (task: WorkTask, preserveCurrentThread = true): string => {
    const existingThreadId = task.threadId && useChatStore.getState().threads.some((thread) => thread.id === task.threadId)
      ? task.threadId
      : null

    if (existingThreadId) {
      const existingThread = useChatStore.getState().threads.find((thread) => thread.id === existingThreadId)
      useChatStore.getState().setThreadRunner(
        existingThreadId,
        task.runner,
        task.runner === 'crew' ? task.crewId : null,
      )
      if (task.runner === 'model') {
        if (task.backendSelection) {
          useChatStore.getState().setThreadProviderSettings(existingThreadId, task.backendSelection)
        } else if (existingThread?.providerSettings) {
          updateTask(task.id, { backendSelection: existingThread.providerSettings })
        }
      }
      return existingThreadId
    }

    const currentThread = threads.find((thread) => thread.id === activeThreadId)
    const providerSettings = resolveWorkTaskChatProviderSettings(task, {
      crews,
      ollamaModel: ollamaConfig.model,
      defaultLlmProfileIds,
      llmProfiles,
      fallbackProviderSettings: currentThread?.providerSettings,
    })
    const previousActiveThreadId = activeThreadId
    const ensuredThread = task.threadId
      ? ensureThread(
          task.threadId,
          deriveTaskName(task),
          providerSettings,
          undefined,
          task.runner,
          task.runner === 'crew' ? task.crewId : null,
        )
      : { id: addThread(
          deriveTaskName(task),
          providerSettings,
          undefined,
          task.runner,
          task.runner === 'crew' ? task.crewId : null,
        ), created: true }

    if (ensuredThread.created) {
      addChatMessage(ensuredThread.id, {
        role: 'system',
        content: buildTaskThreadSummary(task),
        visibleInChat: true,
        timestamp: Date.now(),
      })
    }
    if (!task.threadId) {
      updateTask(task.id, {
        threadId: ensuredThread.id,
        ...(task.runner === 'model' && providerSettings ? { backendSelection: providerSettings } : {}),
      })
    } else if (task.runner === 'model' && providerSettings && !task.backendSelection) {
      updateTask(task.id, { backendSelection: providerSettings })
    }

    if (preserveCurrentThread) {
      setActiveThread(previousActiveThreadId)
    }

    return ensuredThread.id
  }

  const applyTaskWorkingFolder = async (task: WorkTask) => {
    const normalizedWorkDir = task.workDir.trim()
    if (normalizedWorkDir && isAbsolutePath(normalizedWorkDir)) {
      setWorkingFolder(normalizedWorkDir)
      return
    }

    setWorkingFolder(null)
  }

  const pickWorkDir = async (): Promise<string | null> => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
      })
      return typeof selected === 'string' ? selected.trim() : null
    } catch {
      const path = window.prompt('Enter an absolute folder path:')
      return path ? path.trim() : null
    }
  }

  const handlePickNewWorkDir = async () => {
    const selected = await pickWorkDir()
    if (selected) {
      setNewWorkDir(selected)
    }
  }

  const handlePickTaskWorkDir = async (task: WorkTask) => {
    const selected = await pickWorkDir()
    if (selected === null) return

    updateTask(task.id, { workDir: selected })
  }

  const handleOpenTaskChat = async (task: WorkTask) => {
    await loadChatFromDb()
    const threadId = createTaskThread(task, false)
    await applyTaskWorkingFolder(task)
    setActiveMode('work')
    setActiveThread(threadId)
    navigate('/')
  }

  const handleCreateTask = () => {
    if (!canCreateTask) return

    let createdTask: WorkTask | undefined
    if (pendingCrewMissionId && newRunner === 'crew' && newCrewId === pendingCrewMissionId) {
      const crew = crewsById.get(pendingCrewMissionId)
      if (crew) {
        createdTask = {
          ...buildCrewMissionTask(crew),
          title: newTitle.trim(),
          prompt: newPrompt.trim(),
          expectedOutput: newExpectedOutput.trim(),
          workDir: normalizedNewWorkDir,
        }
        upsertMany([createdTask])
      }
    }

    if (!createdTask) {
      const id = addTask({
        title: newTitle,
        prompt: newPrompt,
        expectedOutput: newExpectedOutput,
        workDir: normalizedNewWorkDir,
        runner: newRunner,
        crewId: newRunner === 'crew' ? newCrewId : null,
        model: newRunner === 'model' ? newModel : '',
        backendSelection: newRunner === 'model'
          ? threads.find((thread) => thread.id === activeThreadId)?.providerSettings ?? {
              backend: 'openai-compatible',
              profileId: defaultLlmProfileIds.api ?? defaultLlmProfileIds.ollama,
            }
          : undefined,
      })
      createdTask = useWorkTasksStore.getState().tasks.find((task) => task.id === id)
    }

    if (createdTask) {
      const threadId = createTaskThread(createdTask, true)
      if (newUseProjectContext && newProjectId) {
        attachProjectThread(newProjectId, threadId)
      }
      setSelectedTaskId(createdTask.id)
      setCreatePanelOpen(false)
    }

    setPendingCrewMissionId(null)
    setNewTitle('')
    setNewPrompt('')
    setNewExpectedOutput('')
    setNewWorkDir('')
    setNewUseProjectContext(false)
  }

  const handleTaskProjectContextEnabledChange = (task: WorkTask, enabled: boolean) => {
    if (!enabled) {
      if (task.threadId) detachProjectThreadFromAll(task.threadId)
      return
    }

    const projectId = activeProjectId && projects.some((project) => project.id === activeProjectId)
      ? activeProjectId
      : projects[0]?.id
    if (!projectId) return
    const threadId = createTaskThread(task, true)
    attachProjectThread(projectId, threadId)
  }

  const handleTaskProjectChange = (task: WorkTask, projectId: string) => {
    if (!projects.some((project) => project.id === projectId)) return
    const threadId = createTaskThread(task, true)
    attachProjectThread(projectId, threadId)
  }

  const handleImportCrewTasks = () => {
    const crew = crewsById.get(importCrewId)
    if (!crew) return

    const missionId = buildCrewMissionId(crew.id)
    if (tasks.some((task) => task.id === missionId)) return

    const missionTask = buildCrewMissionTask(crew, crew.updatedAt || Date.now())
    upsertMany([missionTask])
    createTaskThread(missionTask, true)
    setSelectedTaskId(missionTask.id)
  }

  const handleRunTask = async (task: WorkTask) => {
    const normalizedWorkDir = task.workDir.trim()
    if (normalizedWorkDir && !isAbsolutePath(normalizedWorkDir)) {
      const message = tr('Working folder must be absolute.')
      updateTask(task.id, {
        status: 'failed',
        error: message,
        output: message,
        lastRunAt: Date.now(),
      })
      return
    }

    if (runningTaskControllersRef.current.has(task.id)) return

    const taskForRun = normalizedWorkDir ? { ...task, workDir: normalizedWorkDir } : task
    const startedAt = Date.now()
    const abortController = new AbortController()
    runningTaskControllersRef.current.set(task.id, abortController)

    updateTask(task.id, {
      status: 'running',
      output: '',
      error: null,
    })
    canceledTaskIdsRef.current.delete(task.id)

    let threadId: string
    let projectRunContext: TaskProjectRunContext
    try {
      await loadChatFromDb()
      threadId = createTaskThread(taskForRun, true)
      projectRunContext = await resolveTaskProjectRunContext({
        taskId: task.id,
        threadId,
        prompt: task.prompt,
        workDir: normalizedWorkDir,
      })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      updateTask(task.id, {
        status: 'failed',
        error: message,
        output: message,
        lastRunAt: Date.now(),
      })
      runningTaskControllersRef.current.delete(task.id)
      return
    }

    addChatMessage(threadId, {
      role: 'system',
      content: [
        tr('Task run started'),
        `${tr('Runner')}: ${task.runner === 'crew' ? tr('Crew') : tr('Model')}`,
        normalizedWorkDir ? `${tr('Working folder')}: ${normalizedWorkDir}` : '',
      ].filter(Boolean).join('\n'),
      visibleInChat: true,
      timestamp: startedAt,
    })
    if (projectRunContext.warnings.length > 0) {
      addChatMessage(threadId, {
        role: 'system',
        content: `${tr('Project context warnings')}:\n${projectRunContext.warnings.map((warning) => `- ${warning}`).join('\n')}`,
        visibleInChat: true,
        timestamp: Date.now(),
      })
    }
    const taskPromptMessageId = addChatMessage(threadId, {
      role: 'user',
      content: buildTaskPromptMessage(taskForRun),
      timestamp: startedAt,
    })

    if (task.runner === 'model') {
      const assistantMessageId = addChatMessage(threadId, {
        role: 'assistant',
        content: '',
        timestamp: Date.now(),
        streaming: true,
      })

      try {
        let buffered = ''
        let engineError = ''
        const promptWithProjectContext = appendTaskProjectPrompt(task.prompt, projectRunContext)
        const taskThread = useChatStore.getState().threads.find((thread) => thread.id === threadId)
        const providerSelection = task.backendSelection
          ?? taskThread?.providerSettings
          ?? resolveWorkTaskChatProviderSettings(task, {
            crews,
            ollamaModel: ollamaConfig.model,
            defaultLlmProfileIds,
            llmProfiles,
          })
        if (!providerSelection) throw new Error('No backend is configured for this task.')

        if (hasTauriRuntime() && (providerSelection.backend === 'openai-compatible' || providerSelection.backend === 'codex')) {
          const providerState = getChatProviderState({
            ollama: ollamaConfig,
            availableModels,
            llmProfiles,
            defaultLlmProfileIds,
            llmProfileModels,
          }, providerSelection.backend, providerSelection)
          const project = getProjectForThread(projects, threadId)
          const permissionMode = useEngineStore.getState().config.permissionMode
          const workspacePath = (projectRunContext.preferredCwd ?? normalizedWorkDir) || ''
          if (providerState.provider === 'codex') await useCodexStore.getState().load()
          const codexProfile = providerState.provider === 'codex'
            ? useCodexStore.getState().profiles.find((profile) => profile.id === providerState.authProfileId && profile.status === 'ready')
              ?? useCodexStore.getState().profiles.find((profile) => profile.status === 'ready')
            : null
          const durable = providerState.provider === 'codex'
            ? await createDurableCodexRun({
                clientThreadId: threadId,
                clientProjectId: project?.id ?? `standalone:${workspacePath || 'no-workspace'}`,
                clientTaskId: task.id,
                assistantMessageId,
                userMessageId: taskPromptMessageId,
                prompt: promptWithProjectContext,
                history: (taskThread?.messages ?? [])
                  .filter((message) => message.id !== taskPromptMessageId && message.id !== assistantMessageId && message.role !== 'system')
                  .map((message) => ({
                    role: message.role as 'user' | 'assistant',
                    content: message.content,
                  })),
                workspacePath,
                projectRevision: project?.updatedAt ?? 1,
                taskRevision: task.updatedAt,
                toolPolicy: permissionMode === 'strict' || permissionMode === 'plan' ? 'read_only' : 'autonomous',
                profileId: codexProfile?.id ?? '',
                model: providerState.model || undefined,
                reasoningEffort: providerState.reasoningEffort,
                timeoutMs: providerState.timeoutMs,
                source: 'task',
              })
            : await createDurableLocalRun({
            clientThreadId: threadId,
            clientProjectId: project?.id ?? `standalone:${(projectRunContext.preferredCwd ?? normalizedWorkDir) || 'no-workspace'}`,
            clientTaskId: task.id,
            assistantMessageId,
            userMessageId: taskPromptMessageId,
            prompt: promptWithProjectContext,
            history: (taskThread?.messages ?? [])
              .filter((message) => message.id !== taskPromptMessageId && message.id !== assistantMessageId && message.role !== 'system')
              .map((message) => ({
                role: message.role as 'user' | 'assistant',
                content: message.content,
              })),
            workspacePath: workspacePath || null,
            projectRevision: project?.updatedAt ?? 1,
            taskRevision: task.updatedAt,
            toolPolicy: permissionMode === 'strict' || permissionMode === 'plan' ? 'read_only' : 'autonomous',
            provider: providerState,
            mcpServers: policyFlags.allowMcpToolCalls
              ? (mcpServers.length > 0 ? mcpServers : [mcpServer])
              : [],
            source: 'task',
          })
          const { client, run } = durable
          await attachDurableLocalRun(client, run)
          return
        }

        const onEngineEvent = (event: EngineEvent) => {
          if (abortController.signal.aborted) return
          if (event.type === 'text_delta') {
            buffered += event.text
          } else if (event.type === 'assistant_message') {
            const finalText = extractTextContent(event.message)
            if (finalText.trim()) buffered = finalText
          } else if (event.type === 'error') {
            engineError = event.error
          } else {
            return
          }
          updateTask(task.id, { output: buffered || engineError })
          updateChatMessage(threadId, assistantMessageId, { content: buffered || engineError })
        }

        await sendEngineMessage(
          promptWithProjectContext,
          projectRunContext.preferredCwd ?? normalizedWorkDir,
          onEngineEvent,
          {
            threadId,
            ownerKind: 'task',
            ownerId: task.id,
            messages: (taskThread?.messages ?? [])
              .filter((message) => message.id !== taskPromptMessageId && message.id !== assistantMessageId)
              .map((message) => ({
                role: message.role,
                content: message.content,
                debugContent: message.debugContent,
              })),
          },
          providerSelection,
          {
            mode: useEngineStore.getState().config.permissionMode,
            allowedDirectories: projectRunContext.authorizedPaths
              .filter((entry) => entry.kind === 'folder')
              .map((entry) => entry.path),
            authorizedPaths: projectRunContext.authorizedPaths,
          },
        )
        if (engineError) throw new Error(engineError)

        if (abortController.signal.aborted || canceledTaskIdsRef.current.has(task.id)) {
          const message = tr('Task canceled.')
          updateTask(task.id, {
            status: 'canceled',
            error: null,
            output: buffered || message,
            lastRunAt: Date.now(),
          })
          updateChatMessage(threadId, assistantMessageId, {
            content: buffered ? `${buffered}\n\n${message}` : message,
            streaming: false,
          }, {
            persist: true,
          })
          return
        }

        updateTask(task.id, {
          status: 'completed',
          output: buffered,
          lastRunAt: Date.now(),
        })
        updateChatMessage(threadId, assistantMessageId, {
          content: buffered,
          streaming: false,
        }, {
          persist: true,
        })
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        const aborted = abortController.signal.aborted || canceledTaskIdsRef.current.has(task.id)
        updateTask(task.id, {
          status: aborted ? 'canceled' : 'failed',
          error: aborted ? null : message,
          output: aborted ? tr('Task canceled.') : message,
          lastRunAt: Date.now(),
        })
        updateChatMessage(threadId, assistantMessageId, {
          content: aborted ? tr('Task canceled.') : message,
          streaming: false,
        }, {
          persist: true,
        })
      } finally {
        runningTaskControllersRef.current.delete(task.id)
        canceledTaskIdsRef.current.delete(task.id)
      }

      return
    }

    const crewStreamId = createCrewStreamId()
    const streamedCrewLogIds = new Set<string>()
    let crewLiveState: CrewLiveState = {
      streamId: crewStreamId,
      title: `${deriveTaskName(taskForRun)} - Crew-Execution`,
      status: 'running',
      entries: [],
      agentColors: {},
      updatedAt: Date.now(),
    }
    const crewLiveMessageId = addChatMessage(threadId, {
      role: 'assistant',
      content: buildCrewLiveMessageContent(crewLiveState),
      timestamp: Date.now(),
      streaming: true,
      crewLive: crewLiveState,
    })
    const publishCrewLive = (persist = false) => {
      updateChatMessage(threadId, crewLiveMessageId, {
        content: buildCrewLiveMessageContent(crewLiveState),
        streaming: crewLiveState.status === 'running',
        crewLive: crewLiveState,
      }, {
        persist,
      })
    }
    const appendCrewLogToMonitor = (log: CrewExecutionLog) => {
      if (!log.id || streamedCrewLogIds.has(log.id)) return
      const entry = createCrewLiveEntry(log)
      if (!entry) return
      streamedCrewLogIds.add(log.id)
      crewLiveState = appendCrewLiveEntry(crewLiveState, entry)
      publishCrewLive()
    }
    const finishCrewLive = (status: CrewLiveStatus, persist = true) => {
      crewLiveState = {
        ...crewLiveState,
        status,
        updatedAt: Date.now(),
      }
      publishCrewLive(persist)
    }

    try {
      if (!task.crewId) {
        throw new Error('Please select a crew.')
      }

      const crew = crewsById.get(task.crewId)
      if (!crew) {
        throw new Error('Crew not found (possibly deleted).')
      }

      const resolvedCrewAgents = resolveCrewAgentsWithProfiles(crew.agents, personalityProfiles)
      const enabledAgents = resolvedCrewAgents.filter((agent) => agent.enabled)
      if (enabledAgents.length === 0) {
        throw new Error('No active crew members available.')
      }

      const enabledAgentIds = new Set(enabledAgents.map((agent) => agent.id))
      const runtimeTasks = buildCrewRuntimeTasks(crew, task, enabledAgentIds)

      const defaultOpenAICompatibleProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds['openai-compatible'] && profile.preset === 'openai')
        ?? llmProfiles.find((profile) => profile.preset === 'openai')
      const defaultOpenRouterProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds.openrouter && profile.preset === 'openrouter')
        ?? llmProfiles.find((profile) => profile.preset === 'openrouter')

      let providerConfigs: CrewResolvedProviderConfigs = {
        openAICompatible: resolveExternalProviderConfig(
          crew.providerProfiles.openAICompatible,
          defaultOpenAICompatibleProfile,
          defaultOpenAICompatibleProfile?.baseUrl || crew.providerProfiles.openAICompatible.baseUrl || 'https://api.openai.com/v1',
          defaultOpenAICompatibleProfile ? llmProfileModels[defaultOpenAICompatibleProfile.id] ?? [] : [],
        ),
        openRouter: resolveExternalProviderConfig(
          crew.providerProfiles.openRouter,
          defaultOpenRouterProfile,
          defaultOpenRouterProfile?.baseUrl || crew.providerProfiles.openRouter.baseUrl || 'https://openrouter.ai/api/v1',
          defaultOpenRouterProfile ? llmProfileModels[defaultOpenRouterProfile.id] ?? [] : [],
        ),
      }

      let config = resolveCrewRuntimeConfig(crew, {
        baseUrl: ollamaConfig.baseUrl,
        model: ollamaConfig.model,
        timeoutMs: ollamaConfig.timeoutMs,
      })
      const appliedCrewDefault = applyCrewDefaultModel(crew, config, providerConfigs)
      config = appliedCrewDefault.config
      providerConfigs = appliedCrewDefault.providerConfigs
      const crewDefaultProvider = resolveEffectiveCrewProvider(
        crew.defaultProvider ?? 'ollama',
        config,
        providerConfigs,
      )
      runningCrewTaskIdsRef.current.set(task.id, crew.id)

      const crewRequest = {
          id: crew.id,
          streamId: crewStreamId,
          name: crew.name,
          description: crew.description,
          executionSubject: crew.executionSubject,
          executionGuidelines: appendTaskProjectPrompt(
            buildWorkTaskCrewGuidelines(crew, taskForRun),
            projectRunContext,
          ),
          knowledgeFocus: crew.knowledgeFocus,
          responseLanguage: i18n.resolvedLanguage ?? i18n.language ?? 'en',
          governanceMode: crew.governanceMode,
          outputMode: crew.outputMode,
          stopOnFailure: crew.stopOnFailure,
          retryCount: crew.retryCount,
          managerReviewEnabled: crew.managerReviewEnabled,
          managerReviewGuidelines: crew.managerReviewGuidelines,
          shareAllTaskOutputs: crew.shareAllTaskOutputs,
          sharedOutputCharLimit: crew.sharedOutputCharLimit,
          providerConfigs,
          process: crew.process,
          managerAgentId: crew.managerAgentId,
          verbose: crew.verbose,
          maxRpm: crew.maxRpm,
          maxParallelTasks: crew.maxParallelTasks,
          agents: enabledAgents.map((agent) => ({
            id: agent.id,
            name: agent.name,
            role: agent.role,
            goal: agent.goal,
            backstory: agent.backstory,
            skillsMarkdown: agent.skillsMarkdown,
            personalityId: agent.personalityId,
            modelOverride: agent.modelOverride?.trim() ? agent.modelOverride : null,
            providerKind: crewDefaultProvider,
            tools: augmentCrewToolsForTask(agent.tools, taskForRun),
            mcpServerNames: agent.mcpServerNames,
            enabled: agent.enabled,
            allowDelegation: agent.allowDelegation,
            verbose: agent.verbose,
            maxIterations: agent.maxIterations,
          })),
          tasks: runtimeTasks,
          cwd: projectRunContext.preferredCwd,
          authorizedPaths: projectRunContext.authorizedPaths,
          config,
      }
      const assistantMessageId = addChatMessage(threadId, {
        role: 'assistant',
        content: '',
        timestamp: Date.now(),
        streaming: true,
      })
      const project = getProjectForThread(projects, threadId)
      const { client, run } = await createDurableCrewRun({
        clientThreadId: threadId,
        clientProjectId: project?.id ?? `standalone:${(projectRunContext.preferredCwd ?? normalizedWorkDir) || 'no-workspace'}`,
        clientTaskId: task.id,
        assistantMessageId,
        crewLiveMessageId,
        crewLiveTitle: crewLiveState.title,
        prompt: appendTaskProjectPrompt(task.prompt, projectRunContext),
        workspacePath: (projectRunContext.preferredCwd ?? normalizedWorkDir) || null,
        projectRevision: project?.updatedAt ?? 1,
        taskRevision: task.updatedAt,
        crewId: crew.id,
        crewRequest,
        source: 'task',
      })
      const finalRun = await attachDurableLocalRun(client, run)
      const response = ((finalRun.result as Record<string, unknown> | null)?.crew_response
        ?? null) as CrewExecutionResponse | null
      if (!response) {
        throw new Error(finalRun.error?.message || `Crew run ended in state ${finalRun.state}`)
      }

      const mappedStatus = response.status === 'completed' ? 'completed' : 'failed'
      if (canceledTaskIdsRef.current.has(task.id) || response.status === 'canceled') {
        finishCrewLive('canceled')
        updateTask(task.id, {
          status: 'canceled',
          output: tr('Task canceled.'),
          error: null,
          lastRunAt: Date.now(),
        })
        updateChatMessage(threadId, assistantMessageId, {
          content: tr('Task canceled.'),
          streaming: false,
        }, { persist: true })
        return
      }
      const output = buildCrewRunOutput(response, task.id)

      for (const log of response.logs) {
        appendCrewLogToMonitor(log)
      }
      finishCrewLive(mappedStatus === 'completed' ? 'completed' : 'failed')

      updateChatMessage(threadId, assistantMessageId, {
        content: output,
        streaming: false,
      }, { persist: true })

      updateTask(task.id, {
        status: mappedStatus,
        output,
        error: response.error ?? null,
        lastRunAt: Date.now(),
      })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      const aborted = canceledTaskIdsRef.current.has(task.id) || abortController.signal.aborted
      const waitingForApproval = message.trim().toLowerCase().startsWith('crew waiting for approval:')
      finishCrewLive(aborted ? 'canceled' : 'failed')
      addChatMessage(threadId, {
        role: 'assistant',
        content: aborted ? tr('Task canceled.') : message,
        timestamp: Date.now(),
      })
      updateTask(task.id, {
        status: aborted ? 'canceled' : waitingForApproval ? 'waiting_approval' : 'failed',
        error: aborted ? null : message,
        output: aborted ? tr('Task canceled.') : message,
        lastRunAt: Date.now(),
      })
    } finally {
      runningTaskControllersRef.current.delete(task.id)
      runningCrewTaskIdsRef.current.delete(task.id)
      canceledTaskIdsRef.current.delete(task.id)
    }
  }

  const handleCancelTask = async (task: WorkTask) => {
    canceledTaskIdsRef.current.add(task.id)
    runningTaskControllersRef.current.get(task.id)?.abort()
    const canceledDurable = task.threadId
      ? await cancelLatestDurableRun(task.threadId)
      : false
    if (task.runner === 'model' && !canceledDurable) abortEngine()
    const crewId = runningCrewTaskIdsRef.current.get(task.id)
    if (crewId && !canceledDurable) {
      await safeInvoke('crew_stop', { request: { crewId } }, null)
    }
    updateTask(task.id, {
      status: 'canceled',
      error: null,
      output: task.output?.trim() ? `${task.output}\n\n${tr('Task canceled.')}` : tr('Task canceled.'),
      lastRunAt: Date.now(),
    })
  }

  const handleUpsertSchedule = async (task: WorkTask, activeOverride?: boolean) => {
    const scheduleExpr = task.scheduleExpr.trim()
    if (!scheduleExpr) {
      updateTask(task.id, { scheduleEnabled: false })
      return
    }

    const normalizedWorkDir = task.workDir.trim()
    if (normalizedWorkDir && !isAbsolutePath(normalizedWorkDir)) {
      updateTask(task.id, { scheduleEnabled: false })
      return
    }
    let scheduled: ScheduledTask | null = findScheduledTask(scheduledTasks, task.id)
    const active = activeOverride ?? scheduled?.active ?? task.scheduleEnabled

    if (task.runner === 'crew') {
      if (!task.crewId) {
        updateTask(task.id, { scheduleEnabled: false })
        return
      }

      const currentCrew = crewsById.get(task.crewId)
      if (!currentCrew) {
        updateTask(task.id, { scheduleEnabled: false })
        return
      }

      const { crew, metadata } = await resolveCrewScheduleSource(currentCrew)

      const enabledAgents = crew.agents.filter((agent) => agent.enabled)
      if (enabledAgents.length === 0) {
        updateTask(task.id, { scheduleEnabled: false })
        return
      }
      const enabledAgentIds = new Set(enabledAgents.map((agent) => agent.id))
      let runtimeTasks
      try {
        runtimeTasks = buildCrewRuntimeTasks(crew, task, enabledAgentIds)
      } catch {
        updateTask(task.id, { scheduleEnabled: false })
        return
      }

      const defaultOpenAICompatibleProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds['openai-compatible'] && profile.preset === 'openai')
        ?? llmProfiles.find((profile) => profile.preset === 'openai')
      const defaultOpenRouterProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds.openrouter && profile.preset === 'openrouter')
        ?? llmProfiles.find((profile) => profile.preset === 'openrouter')

      let providerConfigs: CrewResolvedProviderConfigs = {
        openAICompatible: resolveExternalProviderConfig(
          crew.providerProfiles.openAICompatible,
          defaultOpenAICompatibleProfile,
          defaultOpenAICompatibleProfile?.baseUrl || crew.providerProfiles.openAICompatible.baseUrl || 'https://api.openai.com/v1',
          defaultOpenAICompatibleProfile ? llmProfileModels[defaultOpenAICompatibleProfile.id] ?? [] : [],
        ),
        openRouter: resolveExternalProviderConfig(
          crew.providerProfiles.openRouter,
          defaultOpenRouterProfile,
          defaultOpenRouterProfile?.baseUrl || crew.providerProfiles.openRouter.baseUrl || 'https://openrouter.ai/api/v1',
          defaultOpenRouterProfile ? llmProfileModels[defaultOpenRouterProfile.id] ?? [] : [],
        ),
      }

      let config = resolveCrewRuntimeConfig(crew, {
        baseUrl: ollamaConfig.baseUrl,
        model: ollamaConfig.model,
        timeoutMs: ollamaConfig.timeoutMs,
      })
      const appliedCrewDefault = applyCrewDefaultModel(crew, config, providerConfigs)
      config = appliedCrewDefault.config
      providerConfigs = appliedCrewDefault.providerConfigs
      const crewDefaultProvider = resolveEffectiveCrewProvider(
        crew.defaultProvider ?? 'ollama',
        config,
        providerConfigs,
      )

      const crewSnapshotJson = JSON.stringify({
        id: crew.id,
        name: crew.name,
        description: crew.description,
        executionSubject: crew.executionSubject,
        executionGuidelines: buildWorkTaskCrewGuidelines(crew, task),
        knowledgeFocus: crew.knowledgeFocus,
        responseLanguage: i18n.resolvedLanguage ?? i18n.language ?? 'en',
        governanceMode: crew.governanceMode,
        outputMode: crew.outputMode,
        stopOnFailure: crew.stopOnFailure,
        retryCount: crew.retryCount,
        managerReviewEnabled: crew.managerReviewEnabled,
        managerReviewGuidelines: crew.managerReviewGuidelines,
        shareAllTaskOutputs: crew.shareAllTaskOutputs,
        sharedOutputCharLimit: crew.sharedOutputCharLimit,
        providerConfigs,
        process: crew.process,
        managerAgentId: crew.managerAgentId,
        verbose: crew.verbose,
        maxRpm: crew.maxRpm,
        maxParallelTasks: crew.maxParallelTasks,
        agents: enabledAgents.map((agent) => ({
          ...agent,
          tools: augmentCrewToolsForTask(agent.tools, task),
          modelOverride: agent.modelOverride?.trim() ? agent.modelOverride : null,
          providerKind: crewDefaultProvider,
        })),
        tasks: runtimeTasks,
        config,
        cwd: normalizedWorkDir || null,
        snapshotSource: metadata.snapshotSource,
        definitionVersionId: metadata.definitionVersionId,
        definitionVersionNumber: metadata.definitionVersionNumber,
        definitionChangeSummary: metadata.definitionChangeSummary,
        definitionSavedAt: metadata.definitionSavedAt,
      })

      let daemonCrewSchedule: Awaited<ReturnType<typeof upsertDurableCrewSchedule>> | null = null
      if (hasTauriRuntime()) {
        await loadChatFromDb()
        const threadId = createTaskThread(task, true)
        const projectRunContext = await resolveTaskProjectRunContext({
          taskId: task.id,
          threadId,
          prompt: task.prompt,
          workDir: normalizedWorkDir,
        })
        const project = getProjectForThread(projects, threadId)
        const crewRequest = JSON.parse(crewSnapshotJson) as Record<string, unknown>
        crewRequest.executionGuidelines = appendTaskProjectPrompt(
          String(crewRequest.executionGuidelines ?? ''),
          projectRunContext,
        )
        crewRequest.cwd = projectRunContext.preferredCwd
        crewRequest.authorizedPaths = projectRunContext.authorizedPaths
        daemonCrewSchedule = await upsertDurableCrewSchedule({
          scheduleClientId: task.id,
          expression: scheduleExpr,
          timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC',
          enabled: Boolean(active),
          clientThreadId: threadId,
          clientProjectId: project?.id ?? `standalone:${(projectRunContext.preferredCwd ?? normalizedWorkDir) || 'no-workspace'}`,
          clientTaskId: task.id,
          crewLiveTitle: `${deriveTaskName(task)} - Crew-Execution`,
          prompt: appendTaskProjectPrompt(task.prompt, projectRunContext),
          workspacePath: (projectRunContext.preferredCwd ?? normalizedWorkDir) || null,
          projectRevision: project?.updatedAt ?? 1,
          taskRevision: task.updatedAt,
          crewId: crew.id,
          crewRequest,
        })
      }

      scheduled = {
        id: task.id,
        name: deriveTaskName(task),
        prompt: task.prompt,
        cronLike: scheduleExpr,
        taskKind: 'crew',
        crewId: crew.id,
        crewSnapshotJson,
        modelConfigJson: daemonCrewSchedule
          ? JSON.stringify({ executorTarget: 'personal_device_daemon' })
          : null,
        priority: scheduled?.priority ?? 100,
        dependsOnTaskIds: scheduled?.dependsOnTaskIds ?? [],
        active: Boolean(active),
        lastRunAt: scheduled?.lastRunAt ?? null,
        nextRunAt: daemonCrewSchedule?.next_run_at
          ? Date.parse(daemonCrewSchedule.next_run_at)
          : scheduled?.nextRunAt ?? null,
      }
    } else {
      scheduled = {
        id: task.id,
        name: deriveTaskName(task),
        prompt: task.prompt,
        cronLike: scheduleExpr,
        taskKind: 'prompt',
        crewId: null,
        crewSnapshotJson: null,
        modelConfigJson: JSON.stringify({
          executorTarget: 'personal_device_daemon',
          backendSelection: task.backendSelection ?? {
            backend: 'openai-compatible',
            profileId: defaultLlmProfileIds.api ?? defaultLlmProfileIds.ollama,
            ...(task.model.trim() ? { model: task.model.trim() } : {}),
          },
          cwd: normalizedWorkDir || null,
        }),
        priority: scheduled?.priority ?? 100,
        dependsOnTaskIds: scheduled?.dependsOnTaskIds ?? [],
        active: Boolean(active),
        lastRunAt: scheduled?.lastRunAt ?? null,
        nextRunAt: scheduled?.nextRunAt ?? null,
      }
    }

    if (task.runner === 'model' && hasTauriRuntime()) {
      await loadChatFromDb()
      const threadId = createTaskThread(task, true)
      const projectRunContext = await resolveTaskProjectRunContext({
        taskId: task.id,
        threadId,
        prompt: task.prompt,
        workDir: normalizedWorkDir,
      })
      const taskThread = useChatStore.getState().threads.find((thread) => thread.id === threadId)
      const project = getProjectForThread(projects, threadId)
      const providerSelection = task.backendSelection
        ?? taskThread?.providerSettings
        ?? resolveWorkTaskChatProviderSettings(task, {
          crews,
          ollamaModel: ollamaConfig.model,
          defaultLlmProfileIds,
          llmProfiles,
        })
      if (!providerSelection) throw new Error('Persistent local schedules require a model profile.')
      const providerState = getChatProviderState({
        ollama: ollamaConfig,
        availableModels,
        llmProfiles,
        defaultLlmProfileIds,
        llmProfileModels,
      }, providerSelection.backend, providerSelection)
      const permissionMode = useEngineStore.getState().config.permissionMode
      const workspacePath = (projectRunContext.preferredCwd ?? normalizedWorkDir) || ''
      if (providerState.provider === 'codex') await useCodexStore.getState().load()
      const codexProfile = providerState.provider === 'codex'
        ? useCodexStore.getState().profiles.find((profile) => profile.id === providerState.authProfileId && profile.status === 'ready')
          ?? useCodexStore.getState().profiles.find((profile) => profile.status === 'ready')
        : null
      const scheduleToolPolicy = permissionMode === 'strict' || permissionMode === 'plan'
        ? 'read_only' as const
        : 'autonomous' as const
      const scheduleInput = {
        scheduleClientId: task.id,
        expression: scheduleExpr,
        timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC',
        enabled: Boolean(active),
        clientThreadId: threadId,
        clientProjectId: project?.id ?? `standalone:${(projectRunContext.preferredCwd ?? normalizedWorkDir) || 'no-workspace'}`,
        clientTaskId: task.id,
        prompt: appendTaskProjectPrompt(task.prompt, projectRunContext),
        history: (taskThread?.messages ?? [])
          .filter((message) => message.role !== 'system')
          .map((message) => ({
            role: message.role as 'user' | 'assistant',
            content: message.content,
          })),
        workspacePath,
        projectRevision: project?.updatedAt ?? 1,
        taskRevision: task.updatedAt,
        toolPolicy: scheduleToolPolicy,
      }
      const daemonSchedule = providerState.provider === 'codex'
        ? await upsertDurableCodexSchedule({
            ...scheduleInput,
            profileId: codexProfile?.id ?? '',
            model: providerState.model || undefined,
            reasoningEffort: providerState.reasoningEffort,
            timeoutMs: providerState.timeoutMs,
          })
        : await upsertDurableLocalSchedule({
            ...scheduleInput,
            provider: providerState,
            mcpServers: policyFlags.allowMcpToolCalls
              ? (mcpServers.length > 0 ? mcpServers : [mcpServer])
              : [],
          })
      scheduled = {
        ...scheduled,
        nextRunAt: daemonSchedule.next_run_at ? Date.parse(daemonSchedule.next_run_at) : null,
        lastRunAt: daemonSchedule.last_triggered_at ? Date.parse(daemonSchedule.last_triggered_at) : scheduled.lastRunAt,
      }
    }
    await upsertScheduledTask(scheduled)
  }

  const handleToggleSchedule = async (task: WorkTask, enabled: boolean) => {
    updateTask(task.id, { scheduleEnabled: enabled })
    const scheduled = findScheduledTask(scheduledTasks, task.id)
    if (scheduled) {
      if (hasTauriRuntime()) {
        await handleUpsertSchedule({ ...task, scheduleEnabled: enabled }, enabled)
        return
      }
      await toggleScheduledTask(task.id, enabled)
      return
    }

    if (enabled) {
      await handleUpsertSchedule({ ...task, scheduleEnabled: true }, true)
    }
  }

  const handleRemoveSchedule = async (task: WorkTask) => {
    updateTask(task.id, { scheduleEnabled: false, scheduleExpr: '' })
    const scheduled = findScheduledTask(scheduledTasks, task.id)
    if (scheduled) {
      await removeScheduledTask(task.id)
    }
    if (hasTauriRuntime()) {
      await deleteDurableLocalSchedule(task.id)
    }
  }

  const handleDeleteTask = async (task: WorkTask) => {
    const scheduled = findScheduledTask(scheduledTasks, task.id)
    if (scheduled) {
      await removeScheduledTask(task.id)
    }
    if (hasTauriRuntime()) {
      await deleteDurableLocalSchedule(task.id)
    }
    removeTask(task.id)
  }

  const handleRemoveLegacyTemplate = (templateId: string) => {
    // Templates are legacy; keep deletion available so users can clean up old storage.
    removeTemplate(templateId)
  }

  const selectedCrewScheduleMetadata = selectedTask?.runner === 'crew'
    ? readCrewScheduleSnapshotMetadata(selectedScheduledTask?.crewSnapshotJson)
    : null
  return (
    <div className="task-view" data-doc-id="view:/tasks">
      <TaskCreatePanel
        crews={crews}
        projects={projects}
        defaultModel={ollamaConfig.model}
        open={createPanelOpen}
        title={newTitle}
        prompt={newPrompt}
        expectedOutput={newExpectedOutput}
        workDir={newWorkDir}
        runner={newRunner}
        crewId={newCrewId}
        model={newModel}
        useProjectContext={newUseProjectContext}
        projectId={newProjectId}
        canCreateTask={canCreateTask}
        onOpenChange={setCreatePanelOpen}
        onTitleChange={setNewTitle}
        onPromptChange={setNewPrompt}
        onExpectedOutputChange={setNewExpectedOutput}
        onWorkDirChange={setNewWorkDir}
        onRunnerChange={setNewRunner}
        onCrewIdChange={setNewCrewId}
        onModelChange={setNewModel}
        onUseProjectContextChange={setNewUseProjectContext}
        onProjectIdChange={setNewProjectId}
        onPickWorkDir={() => void handlePickNewWorkDir()}
        onCreateTask={handleCreateTask}
      />

      <div className="tasks-layout">
        <TaskListPane
          tasks={tasks}
          crews={crews}
          selectedTaskId={selectedTask?.id ?? null}
          importCrewId={importCrewId}
          onSelectTask={setSelectedTaskId}
          onImportCrewIdChange={setImportCrewId}
          onImportCrewTasks={handleImportCrewTasks}
          scheduledTasks={scheduledTasks}
        />

        <TaskDetailPane
          task={selectedTask}
          crews={crews}
          projects={projects}
          defaultModel={ollamaConfig.model}
          scheduled={selectedScheduledTask}
          crewScheduleMetadata={selectedCrewScheduleMetadata}
          projectContext={selectedProjectContext}
          onProjectContextEnabledChange={handleTaskProjectContextEnabledChange}
          onProjectContextProjectChange={handleTaskProjectChange}
          onUpdateTask={updateTask}
          onPickWorkDir={(task) => void handlePickTaskWorkDir(task)}
          onOpenChat={(task) => void handleOpenTaskChat(task)}
          onRunTask={(task) => void handleRunTask(task)}
          onCancelTask={(task) => void handleCancelTask(task)}
          onDeleteTask={(task) => void handleDeleteTask(task)}
          onToggleSchedule={(task, enabled) => void handleToggleSchedule(task, enabled)}
          onSaveSchedule={(task) => void handleUpsertSchedule(task)}
          onRemoveSchedule={(task) => void handleRemoveSchedule(task)}
        />
      </div>

      {templates.length > 0 && (
        <div className="panel">
          <div className="panel-heading-row">
            <h2>{tr("Legacy: Templates")}</h2>
            <span className="hint-text">{templates.length} {tr("template(s) in legacy storage")}</span>
          </div>
          <p className="hint-text">{tr("These templates are no longer used actively. You can clean them up here if needed.")}</p>

          <div className="task-list">
            {templates.map((template) => (
              <div key={template.id} className="work-task-card">
                <strong>{template.title?.trim() ? template.title : template.id}</strong>
                <div className="task-template-description">{template.description}</div>
                <div className="actions work-task-card-actions">
                  <button type="button" className="ui-button ui-button--danger" onClick={() => handleRemoveLegacyTemplate(template.id)}>
                    {tr("Delete")}
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
