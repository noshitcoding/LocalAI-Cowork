import { open } from '@tauri-apps/plugin-dialog'
import { listen } from '@tauri-apps/api/event'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { CalendarClock, ListTodo, PlayCircle } from 'lucide-react'
import { useChatStore, type CrewLiveState, type CrewLiveStatus } from '../stores/chatStore'
import { useConfigStore } from '../stores/configStore'
import { useCoworkStore, type ScheduledTask } from '../stores/coworkStore'
import { resolveCrewAgentsWithProfiles, useCrewStore, type CrewPersonalityProfile } from '../stores/crewStore'
import { usePersonalityStore } from '../stores/personalityStore'
import { useProjectStore } from '../stores/projectStore'
import { useTaskTemplatesStore } from '../stores/taskTemplatesStore'
import { useUiStore } from '../stores/uiStore'
import { useWorkTasksStore, type WorkTask, type WorkTaskRunner } from '../stores/workTasksStore'
import { tr } from '../i18n'
import { safeInvoke, safeInvokeVoid } from '../utils/safeInvoke'
import { streamChatTurn } from '../utils/ollamaStreaming'
import {
  appendCrewLiveEntry,
  applyCrewDefaultModel,
  augmentCrewToolsForTask,
  buildCrewLiveMessageContent,
  buildCrewRuntimeTasks,
  buildWorkTaskCrewGuidelines,
  createCrewLiveEntry,
  resolveCrewRuntimeConfig,
  resolveExternalProviderConfig,
  type CrewExecutionLog,
  type CrewExecutionLogEvent,
  type CrewExecutionResponse,
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
  const setActiveMode = useUiStore((s) => s.setActiveMode)
  const setWorkingFolder = useUiStore((s) => s.setWorkingFolder)

  const templates = useTaskTemplatesStore((s) => s.templates)
  const removeTemplate = useTaskTemplatesStore((s) => s.removeTemplate)

  const {
    scheduledTasks,
    loadScheduledTasks,
    upsertScheduledTask,
    toggleScheduledTask,
    removeScheduledTask,
  } = useCoworkStore()

  const ollamaConfig = useConfigStore((s) => s.ollama)
  const defaultLlmProfileIds = useConfigStore((s) => s.defaultLlmProfileIds)
  const llmProfiles = useConfigStore((s) => s.llmProfiles)

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
    if (importCrewId && crews.some((crew) => crew.id === importCrewId)) return
    setImportCrewId(crews[0]?.id ?? '')
  }, [crews, importCrewId])

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
    const project = projects.find((item) => item.threadIds.includes(selectedTask.threadId as string))
    return project ? { title: project.title } : null
  }, [projects, selectedTask?.threadId])

  const ensureAllowedTaskFolder = async (workDir: string) => {
    const normalized = workDir.trim()
    if (!normalized || !isAbsolutePath(normalized)) return
    await safeInvokeVoid('fs_add_allowed_folder', { path: normalized })
  }

  const createTaskThread = (task: WorkTask, preserveCurrentThread = true): string => {
    const existingThreadId = task.threadId && useChatStore.getState().threads.some((thread) => thread.id === task.threadId)
      ? task.threadId
      : null

    if (existingThreadId) {
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
      updateTask(task.id, { threadId: ensuredThread.id })
    }

    if (preserveCurrentThread) {
      setActiveThread(previousActiveThreadId)
    }

    return ensuredThread.id
  }

  const applyTaskWorkingFolder = async (task: WorkTask) => {
    const normalizedWorkDir = task.workDir.trim()
    if (normalizedWorkDir && isAbsolutePath(normalizedWorkDir)) {
      await ensureAllowedTaskFolder(normalizedWorkDir)
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
      await ensureAllowedTaskFolder(selected)
    }
  }

  const handlePickTaskWorkDir = async (task: WorkTask) => {
    const selected = await pickWorkDir()
    if (selected === null) return

    updateTask(task.id, { workDir: selected })
    if (isAbsolutePath(selected)) {
      await ensureAllowedTaskFolder(selected)
    }
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
      })
      createdTask = useWorkTasksStore.getState().tasks.find((task) => task.id === id)
    }

    if (createdTask) {
      void ensureAllowedTaskFolder(createdTask.workDir)
      createTaskThread(createdTask, true)
      setSelectedTaskId(createdTask.id)
    }

    setPendingCrewMissionId(null)
    setNewTitle('')
    setNewPrompt('')
    setNewExpectedOutput('')
    setNewWorkDir('')
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

    const taskForRun = normalizedWorkDir ? { ...task, workDir: normalizedWorkDir } : task
    await loadChatFromDb()
    const threadId = createTaskThread(taskForRun, true)
    const startedAt = Date.now()

    updateTask(task.id, {
      status: 'running',
      output: '',
      error: null,
    })
    canceledTaskIdsRef.current.delete(task.id)
    const abortController = new AbortController()
    runningTaskControllersRef.current.set(task.id, abortController)

    await ensureAllowedTaskFolder(normalizedWorkDir)
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
    addChatMessage(threadId, {
      role: 'user',
      content: buildTaskPromptMessage(taskForRun),
      timestamp: startedAt,
    })

    if (task.runner === 'model') {
      const model = task.model.trim() || ollamaConfig.model
      const config = {
        ...ollamaConfig,
        model,
      }
      const assistantMessageId = addChatMessage(threadId, {
        role: 'assistant',
        content: '',
        timestamp: Date.now(),
        streaming: true,
      })

      try {
        let buffered = ''
        const response = await streamChatTurn(
          {
            prompt: task.prompt,
            history: normalizedWorkDir ? [{ role: 'system', content: `Working directory: ${normalizedWorkDir}` }] : [],
            config,
          },
          (chunk) => {
            if (abortController.signal.aborted) return
            buffered += chunk
            updateTask(task.id, { output: buffered })
            updateChatMessage(threadId, assistantMessageId, { content: buffered })
          },
          { signal: abortController.signal },
        )

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
          output: response.assistantMessage,
          lastRunAt: Date.now(),
        })
        updateChatMessage(threadId, assistantMessageId, {
          content: response.assistantMessage,
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
    let unlistenCrewLogs: (() => void) | null = null
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

      const defaultOpenAICompatibleProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds['openai-compatible'] && profile.provider === 'openai-compatible')
        ?? llmProfiles.find((profile) => profile.provider === 'openai-compatible')
      const defaultOpenRouterProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds.openrouter && profile.provider === 'openrouter')
        ?? llmProfiles.find((profile) => profile.provider === 'openrouter')

      let providerConfigs = {
        openAICompatible: resolveExternalProviderConfig(
          crew.providerProfiles.openAICompatible,
          defaultOpenAICompatibleProfile,
          defaultOpenAICompatibleProfile?.baseUrl || crew.providerProfiles.openAICompatible.baseUrl || 'https://api.openai.com/v1',
        ),
        openRouter: resolveExternalProviderConfig(
          crew.providerProfiles.openRouter,
          defaultOpenRouterProfile,
          defaultOpenRouterProfile?.baseUrl || crew.providerProfiles.openRouter.baseUrl || 'https://openrouter.ai/api/v1',
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
      const crewDefaultProvider = crew.defaultProvider ?? 'ollama'
      runningCrewTaskIdsRef.current.set(task.id, crew.id)

      try {
        unlistenCrewLogs = await listen<CrewExecutionLogEvent>('crew-execution-log', (event) => {
          const payload = event.payload
          if (!payload || payload.streamId !== crewStreamId) return
          appendCrewLogToMonitor(payload.log)
        })
      } catch {
        // In browser-only tests or fallback environments the Tauri event bus is unavailable.
      }

      const response = await safeInvoke<CrewExecutionResponse>('crew_execute', {
        request: {
          id: crew.id,
          streamId: crewStreamId,
          name: crew.name,
          description: crew.description,
          executionSubject: crew.executionSubject,
          executionGuidelines: buildWorkTaskCrewGuidelines(crew, taskForRun),
          knowledgeFocus: crew.knowledgeFocus,
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
          cwd: normalizedWorkDir || null,
          config,
        },
      })

      const mappedStatus = response.status === 'completed' ? 'completed' : 'failed'
      if (canceledTaskIdsRef.current.has(task.id) || response.status === 'canceled') {
        finishCrewLive('canceled')
        updateTask(task.id, {
          status: 'canceled',
          output: tr('Task canceled.'),
          error: null,
          lastRunAt: Date.now(),
        })
        addChatMessage(threadId, {
          role: 'assistant',
          content: tr('Task canceled.'),
          timestamp: Date.now(),
        })
        return
      }
      const output = buildCrewRunOutput(response, task.id)

      for (const log of response.logs) {
        appendCrewLogToMonitor(log)
      }
      finishCrewLive(mappedStatus === 'completed' ? 'completed' : 'failed')

      addChatMessage(threadId, {
        role: 'assistant',
        content: output,
        timestamp: Date.now(),
      })

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
      unlistenCrewLogs?.()
      runningTaskControllersRef.current.delete(task.id)
      runningCrewTaskIdsRef.current.delete(task.id)
      canceledTaskIdsRef.current.delete(task.id)
    }
  }

  const handleCancelTask = async (task: WorkTask) => {
    canceledTaskIdsRef.current.add(task.id)
    runningTaskControllersRef.current.get(task.id)?.abort()
    const crewId = runningCrewTaskIdsRef.current.get(task.id)
    if (crewId) {
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
    if (normalizedWorkDir) {
      await ensureAllowedTaskFolder(normalizedWorkDir)
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

      const defaultOpenAICompatibleProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds['openai-compatible'] && profile.provider === 'openai-compatible')
        ?? llmProfiles.find((profile) => profile.provider === 'openai-compatible')
      const defaultOpenRouterProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds.openrouter && profile.provider === 'openrouter')
        ?? llmProfiles.find((profile) => profile.provider === 'openrouter')

      let providerConfigs = {
        openAICompatible: resolveExternalProviderConfig(
          crew.providerProfiles.openAICompatible,
          defaultOpenAICompatibleProfile,
          defaultOpenAICompatibleProfile?.baseUrl || crew.providerProfiles.openAICompatible.baseUrl || 'https://api.openai.com/v1',
        ),
        openRouter: resolveExternalProviderConfig(
          crew.providerProfiles.openRouter,
          defaultOpenRouterProfile,
          defaultOpenRouterProfile?.baseUrl || crew.providerProfiles.openRouter.baseUrl || 'https://openrouter.ai/api/v1',
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
      const crewDefaultProvider = crew.defaultProvider ?? 'ollama'

      const crewSnapshotJson = JSON.stringify({
        id: crew.id,
        name: crew.name,
        description: crew.description,
        executionSubject: crew.executionSubject,
        executionGuidelines: buildWorkTaskCrewGuidelines(crew, task),
        knowledgeFocus: crew.knowledgeFocus,
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

      scheduled = {
        id: task.id,
        name: deriveTaskName(task),
        prompt: task.prompt,
        cronLike: scheduleExpr,
        taskKind: 'crew',
        crewId: crew.id,
        crewSnapshotJson,
        modelConfigJson: null,
        priority: scheduled?.priority ?? 100,
        dependsOnTaskIds: scheduled?.dependsOnTaskIds ?? [],
        active: Boolean(active),
        lastRunAt: scheduled?.lastRunAt ?? null,
        nextRunAt: scheduled?.nextRunAt ?? null,
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
          ...ollamaConfig,
          model: task.model.trim() || ollamaConfig.model,
          cwd: normalizedWorkDir || null,
        }),
        priority: scheduled?.priority ?? 100,
        dependsOnTaskIds: scheduled?.dependsOnTaskIds ?? [],
        active: Boolean(active),
        lastRunAt: scheduled?.lastRunAt ?? null,
        nextRunAt: scheduled?.nextRunAt ?? null,
      }
    }

    await upsertScheduledTask(scheduled)
  }

  const handleToggleSchedule = async (task: WorkTask, enabled: boolean) => {
    updateTask(task.id, { scheduleEnabled: enabled })
    const scheduled = findScheduledTask(scheduledTasks, task.id)
    if (scheduled) {
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
  }

  const handleDeleteTask = async (task: WorkTask) => {
    const scheduled = findScheduledTask(scheduledTasks, task.id)
    if (scheduled) {
      await removeScheduledTask(task.id)
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
  const activeTaskCount = tasks.filter((task) => task.status === 'running' || task.status === 'waiting_approval').length
  const scheduledTaskCount = tasks.filter((task) => Boolean(findScheduledTask(scheduledTasks, task.id))).length

  return (
    <div className="task-view" data-doc-id="view:/tasks">
      <header className="task-view-header">
        <div className="task-view-heading">
          <span className="task-view-kicker">{tr('Task command center')}</span>
          <h1>{tr('Tasks')}</h1>
          <p>{tr('Create tasks, assign a crew or model, start them, and schedule each task.')}</p>
        </div>
        <div className="task-view-metrics" aria-label={tr('Task overview')}>
          <span><ListTodo size={16} aria-hidden="true" /><strong>{tasks.length}</strong>{tr('Total')}</span>
          <span><PlayCircle size={16} aria-hidden="true" /><strong>{activeTaskCount}</strong>{tr('Active')}</span>
          <span><CalendarClock size={16} aria-hidden="true" /><strong>{scheduledTaskCount}</strong>{tr('Scheduled')}</span>
        </div>
      </header>

      <TaskCreatePanel
        crews={crews}
        defaultModel={ollamaConfig.model}
        title={newTitle}
        prompt={newPrompt}
        expectedOutput={newExpectedOutput}
        workDir={newWorkDir}
        runner={newRunner}
        crewId={newCrewId}
        model={newModel}
        canCreateTask={canCreateTask}
        onTitleChange={setNewTitle}
        onPromptChange={setNewPrompt}
        onExpectedOutputChange={setNewExpectedOutput}
        onWorkDirChange={setNewWorkDir}
        onRunnerChange={setNewRunner}
        onCrewIdChange={setNewCrewId}
        onModelChange={setNewModel}
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
          defaultModel={ollamaConfig.model}
          scheduled={selectedScheduledTask}
          crewScheduleMetadata={selectedCrewScheduleMetadata}
          projectContext={selectedProjectContext}
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
