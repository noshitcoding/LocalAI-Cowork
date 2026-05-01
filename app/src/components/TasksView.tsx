import { open } from '@tauri-apps/plugin-dialog'
import { useEffect, useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useChatStore } from '../stores/chatStore'
import { useConfigStore } from '../stores/configStore'
import { useCoworkStore, type ScheduledTask } from '../stores/coworkStore'
import { useCrewStore, type Crew, type CrewProviderKind } from '../stores/crewStore'
import { useTaskTemplatesStore } from '../stores/taskTemplatesStore'
import { useUiStore } from '../stores/uiStore'
import { useWorkTasksStore, type WorkTask, type WorkTaskRunner } from '../stores/workTasksStore'
import { safeInvoke, safeInvokeVoid } from '../utils/safeInvoke'
import { streamChatTurn } from '../utils/ollamaStreaming'

type CrewExecutionLog = {
  id: string
  crewId: string
  agentId: string
  taskId: string
  action: string
  result: string
  timestamp: number
}

type CrewTaskExecutionResponse = {
  taskId: string
  agentId: string
  status: 'pending' | 'running' | 'completed' | 'failed' | 'canceled'
  output: string | null
}

type CrewExecutionResponse = {
  crewId: string
  status: 'idle' | 'running' | 'completed' | 'failed' | 'canceled'
  taskResults: CrewTaskExecutionResponse[]
  logs: CrewExecutionLog[]
  error: string | null
}

type CrewResolvedProviderConfigs = {
  openAICompatible: { baseUrl: string; model: string; apiKey: string; timeoutMs: number } | undefined
  openRouter: { baseUrl: string; model: string; apiKey: string; timeoutMs: number } | undefined
}

function resolveDefaultAgentId(crew: Crew): string | null {
  const enabled = crew.agents.find((agent) => agent.enabled)
  if (enabled) return enabled.id
  return crew.agents[0]?.id ?? null
}

function resolveCrewRuntimeConfig(crew: Crew, fallbackConfig: { baseUrl: string; model: string; timeoutMs: number }) {
  if (!crew.runtimeConfig.enabled) {
    return fallbackConfig
  }

  return {
    ...fallbackConfig,
    baseUrl: crew.runtimeConfig.baseUrl.trim() || fallbackConfig.baseUrl,
    model: crew.runtimeConfig.model.trim() || fallbackConfig.model,
    timeoutMs: Math.max(1000, crew.runtimeConfig.timeoutMs || fallbackConfig.timeoutMs),
  }
}

function resolveExternalProviderConfig(
  config: { enabled: boolean; baseUrl: string; model: string; apiKey: string; timeoutMs: number },
  fallbackConfig: { baseUrl?: string; model?: string; apiKey?: string } | undefined,
  fallbackBaseUrl: string,
) {
  if (!config.enabled) {
    return undefined
  }

  return {
    baseUrl: config.baseUrl.trim() || fallbackConfig?.baseUrl?.trim() || fallbackBaseUrl,
    model: config.model.trim() || fallbackConfig?.model?.trim() || '',
    apiKey: config.apiKey.trim() || fallbackConfig?.apiKey?.trim() || '',
    timeoutMs: Math.max(1000, config.timeoutMs || 600000),
  }
}

function applyCrewDefaultModel(
  crew: Crew,
  config: { baseUrl: string; model: string; timeoutMs: number },
  providerConfigs: CrewResolvedProviderConfigs,
) {
  const defaultProvider: CrewProviderKind = crew.defaultProvider ?? 'ollama'
  const defaultModel = crew.defaultModel?.trim()
  if (!defaultModel) {
    return { config, providerConfigs }
  }

  if (defaultProvider === 'ollama') {
    return {
      config: { ...config, model: defaultModel },
      providerConfigs,
    }
  }

  if (defaultProvider === 'openai-compatible' && providerConfigs.openAICompatible) {
    return {
      config,
      providerConfigs: {
        ...providerConfigs,
        openAICompatible: { ...providerConfigs.openAICompatible, model: defaultModel },
      },
    }
  }

  if (defaultProvider === 'openrouter' && providerConfigs.openRouter) {
    return {
      config,
      providerConfigs: {
        ...providerConfigs,
        openRouter: { ...providerConfigs.openRouter, model: defaultModel },
      },
    }
  }

  return { config, providerConfigs }
}

function formatTimestamp(ts: number | null | undefined): string {
  if (!ts) return '—'
  try {
    return new Date(ts).toLocaleString('de-DE')
  } catch {
    return String(ts)
  }
}

function deriveTaskName(task: WorkTask): string {
  const title = task.title.trim()
  if (title) return title
  const prompt = task.prompt.trim()
  if (!prompt) return task.id
  const singleLine = prompt.replace(/\s+/g, ' ').trim()
  return singleLine.length > 48 ? `${singleLine.slice(0, 48)}…` : singleLine
}

function findScheduledTask(scheduledTasks: ScheduledTask[], taskId: string): ScheduledTask | null {
  return scheduledTasks.find((entry) => entry.id === taskId) ?? null
}

function isAbsolutePath(path: string): boolean {
  const trimmed = path.trim()
  return /^(?:[a-zA-Z]:[\\/]|\\\\|\/)/.test(trimmed)
}

function buildTaskThreadSummary(task: WorkTask): string {
  const lines = [
    `Task angelegt: ${deriveTaskName(task)}`,
    `Runner: ${task.runner === 'crew' ? 'Crew' : 'Modell'}`,
    task.expectedOutput.trim() ? `Expected Output: ${task.expectedOutput.trim()}` : '',
    task.workDir.trim() ? `Arbeitsordner: ${task.workDir.trim()}` : '',
  ].filter(Boolean)

  return lines.join('\n')
}

function buildTaskPromptMessage(task: WorkTask): string {
  const parts = [task.prompt.trim()]

  if (task.expectedOutput.trim()) {
    parts.push(`Erwartetes Ergebnis:\n${task.expectedOutput.trim()}`)
  }

  if (task.workDir.trim()) {
    parts.push(`Arbeitsordner:\n${task.workDir.trim()}`)
  }

  return parts.filter(Boolean).join('\n\n')
}

function formatCrewLogMessage(log: CrewExecutionLog): string {
  return [
    `Aktion: ${log.action}`,
    `Agent: ${log.agentId}`,
    log.result.trim(),
  ].filter(Boolean).join('\n\n')
}

export default function TasksView() {
  const navigate = useNavigate()
  const crews = useCrewStore((s) => s.crews)
  const { tasks, addTask, updateTask, removeTask, upsertMany } = useWorkTasksStore()
  const addThread = useChatStore((s) => s.addThread)
  const activeThreadId = useChatStore((s) => s.activeThreadId)
  const threads = useChatStore((s) => s.threads)
  const setActiveThread = useChatStore((s) => s.setActiveThread)
  const addChatMessage = useChatStore((s) => s.addMessage)
  const updateChatMessage = useChatStore((s) => s.updateMessage)
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

  const [newTitle, setNewTitle] = useState('')
  const [newPrompt, setNewPrompt] = useState('')
  const [newExpectedOutput, setNewExpectedOutput] = useState('')
  const [newWorkDir, setNewWorkDir] = useState('')
  const [newRunner, setNewRunner] = useState<WorkTaskRunner>('crew')
  const [newCrewId, setNewCrewId] = useState<string>('')
  const [newModel, setNewModel] = useState<string>('')

  const normalizedNewWorkDir = newWorkDir.trim()
  const canCreateTask = newPrompt.trim().length > 0
    && (newRunner !== 'crew' || Boolean(newCrewId))
    && (!normalizedNewWorkDir || isAbsolutePath(normalizedNewWorkDir))

  useEffect(() => {
    void loadScheduledTasks()
  }, [loadScheduledTasks])

  useEffect(() => {
    if (newRunner !== 'crew') return
    if (newCrewId && crews.some((crew) => crew.id === newCrewId)) return
    setNewCrewId(crews[0]?.id ?? '')
  }, [crews, newCrewId, newRunner])

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

  useEffect(() => {
    // One-way migration helper: import Crew-attached tasks into Work Tasks.
    const existingIds = new Set(tasks.map((task) => task.id))
    const legacyCrewTasks: WorkTask[] = []
    for (const crew of crews) {
      for (const crewTask of crew.tasks ?? []) {
        if (existingIds.has(crewTask.id)) continue
        legacyCrewTasks.push({
          id: crewTask.id,
          title: '',
          prompt: crewTask.description,
          expectedOutput: crewTask.expectedOutput,
          workDir: '',
          threadId: null,
          runner: 'crew',
          crewId: crew.id,
          model: '',
          scheduleExpr: '',
          scheduleEnabled: false,
          status: 'idle',
          output: crewTask.output ?? null,
          error: null,
          lastRunAt: null,
          createdAt: crew.updatedAt || Date.now(),
          updatedAt: crew.updatedAt || Date.now(),
        })
      }
    }

    if (legacyCrewTasks.length === 0) return
    upsertMany(legacyCrewTasks)
  }, [crews, tasks, upsertMany])

  const crewsById = useMemo(() => new Map(crews.map((crew) => [crew.id, crew])), [crews])

  const ensureAllowedTaskFolder = async (workDir: string) => {
    const normalized = workDir.trim()
    if (!normalized || !isAbsolutePath(normalized)) return
    await safeInvokeVoid('fs_add_allowed_folder', { path: normalized })
  }

  const createTaskThread = (task: WorkTask, preserveCurrentThread = true): string => {
    const existingThreadId = task.threadId && threads.some((thread) => thread.id === task.threadId)
      ? task.threadId
      : null

    if (existingThreadId) {
      return existingThreadId
    }

    const previousActiveThreadId = activeThreadId
    const threadId = addThread(deriveTaskName(task))
    addChatMessage(threadId, {
      role: 'system',
      content: buildTaskThreadSummary(task),
      visibleInChat: true,
      timestamp: Date.now(),
    })
    updateTask(task.id, { threadId })

    if (preserveCurrentThread) {
      setActiveThread(previousActiveThreadId)
    }

    return threadId
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
      const path = window.prompt('Absoluten Ordnerpfad eingeben:')
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
    const threadId = createTaskThread(task, false)
    await applyTaskWorkingFolder(task)
    setActiveMode('work')
    setActiveThread(threadId)
    navigate('/')
  }

  const handleCreateTask = () => {
    if (!canCreateTask) return

    const id = addTask({
      title: newTitle,
      prompt: newPrompt,
      expectedOutput: newExpectedOutput,
      workDir: normalizedNewWorkDir,
      runner: newRunner,
      crewId: newRunner === 'crew' ? newCrewId : null,
      model: newRunner === 'model' ? newModel : '',
    })

    const createdTask = useWorkTasksStore.getState().tasks.find((task) => task.id === id)
    if (createdTask) {
      void ensureAllowedTaskFolder(createdTask.workDir)
      createTaskThread(createdTask, true)
    }

    setNewTitle('')
    setNewPrompt('')
    setNewExpectedOutput('')
    setNewWorkDir('')
  }

  const handleRunTask = async (task: WorkTask) => {
    const normalizedWorkDir = task.workDir.trim()
    if (normalizedWorkDir && !isAbsolutePath(normalizedWorkDir)) {
      const message = 'Arbeitsordner muss absolut sein.'
      updateTask(task.id, {
        status: 'failed',
        error: message,
        output: message,
        lastRunAt: Date.now(),
      })
      return
    }

    const taskForRun = normalizedWorkDir ? { ...task, workDir: normalizedWorkDir } : task
    const threadId = createTaskThread(taskForRun, true)
    const startedAt = Date.now()

    updateTask(task.id, {
      status: 'running',
      output: '',
      error: null,
    })

    await ensureAllowedTaskFolder(normalizedWorkDir)
    addChatMessage(threadId, {
      role: 'system',
      content: [
        'Task-Lauf gestartet',
        `Runner: ${task.runner === 'crew' ? 'Crew' : 'Modell'}`,
        normalizedWorkDir ? `Arbeitsordner: ${normalizedWorkDir}` : '',
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
            history: normalizedWorkDir ? [{ role: 'system', content: `Arbeitsverzeichnis: ${normalizedWorkDir}` }] : [],
            config,
          },
          (chunk) => {
            buffered += chunk
            updateTask(task.id, { output: buffered })
            updateChatMessage(threadId, assistantMessageId, { content: buffered })
          },
        )

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
        updateTask(task.id, {
          status: 'failed',
          error: message,
          output: message,
          lastRunAt: Date.now(),
        })
        updateChatMessage(threadId, assistantMessageId, {
          content: message,
          streaming: false,
        }, {
          persist: true,
        })
      }

      return
    }

    try {
      if (!task.crewId) {
        throw new Error('Bitte eine Crew auswaehlen.')
      }

      const crew = crewsById.get(task.crewId)
      if (!crew) {
        throw new Error('Crew nicht gefunden (evtl. geloescht).')
      }

      const enabledAgents = crew.agents.filter((agent) => agent.enabled)
      if (enabledAgents.length === 0) {
        throw new Error('Keine aktiven Crew-Mitglieder vorhanden.')
      }

      const agentId = resolveDefaultAgentId(crew)
      if (!agentId) {
        throw new Error('Crew hat keinen Agenten.')
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

      const response = await safeInvoke<CrewExecutionResponse>('crew_execute', {
        request: {
          id: crew.id,
          name: crew.name,
          description: crew.description,
          executionGuidelines: crew.executionGuidelines,
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
            providerKind: agent.providerKind || crewDefaultProvider,
            tools: agent.tools,
            mcpServerNames: agent.mcpServerNames,
            enabled: agent.enabled,
            allowDelegation: agent.allowDelegation,
            verbose: agent.verbose,
            maxIterations: agent.maxIterations,
          })),
          tasks: [
            {
              id: task.id,
              description: task.prompt,
              expectedOutput: task.expectedOutput,
              agentId,
              context: [],
              dependencies: [],
              asyncExecution: crew.process === 'parallel',
            },
          ],
          cwd: normalizedWorkDir || null,
          config,
        },
      })

      const taskResult = response.taskResults.find((result) => result.taskId === task.id) ?? response.taskResults[0]
      const mappedStatus = response.status === 'completed' ? 'completed' : 'failed'
      const output = taskResult?.output ?? response.error ?? 'Crew-Lauf abgeschlossen ohne Textausgabe.'

      for (const log of response.logs) {
        addChatMessage(threadId, {
          role: 'system',
          content: formatCrewLogMessage(log),
          visibleInChat: true,
          timestamp: log.timestamp,
        })
      }

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
      addChatMessage(threadId, {
        role: 'assistant',
        content: message,
        timestamp: Date.now(),
      })
      updateTask(task.id, {
        status: 'failed',
        error: message,
        output: message,
        lastRunAt: Date.now(),
      })
    }
  }

  const handleUpsertSchedule = async (task: WorkTask) => {
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

    if (task.runner === 'crew') {
      if (!task.crewId) {
        updateTask(task.id, { scheduleEnabled: false })
        return
      }

      const crew = crewsById.get(task.crewId)
      if (!crew) {
        updateTask(task.id, { scheduleEnabled: false })
        return
      }

      const enabledAgents = crew.agents.filter((agent) => agent.enabled)
      const agentId = resolveDefaultAgentId(crew)
      if (!agentId || enabledAgents.length === 0) {
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
        executionGuidelines: crew.executionGuidelines,
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
          modelOverride: agent.modelOverride?.trim() ? agent.modelOverride : null,
          providerKind: agent.providerKind || crewDefaultProvider,
        })),
        tasks: [
          {
            id: task.id,
            description: task.prompt,
            expectedOutput: task.expectedOutput,
            agentId,
            context: [],
            dependencies: [],
            asyncExecution: crew.process === 'parallel',
          },
        ],
        config,
        cwd: normalizedWorkDir || null,
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
        active: Boolean(task.scheduleEnabled),
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
        active: Boolean(task.scheduleEnabled),
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
    }
  }

  const handleRemoveSchedule = async (task: WorkTask) => {
    updateTask(task.id, { scheduleEnabled: false, scheduleExpr: '' })
    const scheduled = findScheduledTask(scheduledTasks, task.id)
    if (scheduled) {
      await removeScheduledTask(task.id)
    }
  }

  const handleRemoveLegacyTemplate = (templateId: string) => {
    // Templates are legacy; keep deletion available so users can clean up old storage.
    removeTemplate(templateId)
  }

  return (
    <div className="settings-view">
      <h1>Tasks</h1>
      <p className="hint-text">Tasks erstellen, Crew oder Modell zuordnen, starten und pro Task schedulen.</p>

      <div className="panel">
        <h2>➕ Neuer Task</h2>
        <div className="grid">
          <label>
            Titel (optional)
            <input value={newTitle} onChange={(e) => setNewTitle(e.target.value)} placeholder="z.B. Weekly Report" />
          </label>
          <label>
            Ausfuehrung
            <select value={newRunner} onChange={(e) => setNewRunner(e.target.value as WorkTaskRunner)}>
              <option value="crew">Crew</option>
              <option value="model">Modell</option>
            </select>
          </label>
          {newRunner === 'crew' ? (
            <label>
              Crew
              <select value={newCrewId} onChange={(e) => setNewCrewId(e.target.value)}>
                {crews.length === 0 && (
                  <option value="">Keine Crews vorhanden</option>
                )}
                {crews.map((crew) => (
                  <option key={crew.id} value={crew.id}>{crew.name}</option>
                ))}
              </select>
            </label>
          ) : (
            <label>
              Modell (optional)
              <input value={newModel} onChange={(e) => setNewModel(e.target.value)} placeholder={`Default: ${ollamaConfig.model || '—'}`} />
            </label>
          )}
          <label>
            Expected Output (optional)
            <input value={newExpectedOutput} onChange={(e) => setNewExpectedOutput(e.target.value)} placeholder="z.B. Bullet-Report" />
          </label>
          <label style={{ gridColumn: '1 / -1' }}>
            Arbeitsordner (optional, absolut)
            <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <input value={newWorkDir} onChange={(e) => setNewWorkDir(e.target.value)} placeholder="C:\\Projekte\\mein-task" />
              <button type="button" className="btn-secondary" onClick={() => void handlePickNewWorkDir()}>
                Ordner waehlen
              </button>
            </div>
            {normalizedNewWorkDir && !isAbsolutePath(normalizedNewWorkDir) ? (
              <div className="hint-text">Der Arbeitsordner muss absolut sein.</div>
            ) : null}
          </label>
          <label style={{ gridColumn: '1 / -1' }}>
            Aufgabe
            <textarea value={newPrompt} onChange={(e) => setNewPrompt(e.target.value)} rows={4} placeholder="Was soll der Task tun?" />
          </label>
        </div>
        <div className="actions">
          <button type="button" onClick={handleCreateTask} disabled={!canCreateTask}>
            Task erstellen
          </button>
        </div>
        {newRunner === 'crew' && crews.length === 0 && (
          <p className="hint-text">Erstelle zuerst eine Crew in den Einstellungen, um Crew-Tasks auszufuehren.</p>
        )}
      </div>

      <div className="panel">
        <div className="panel-heading-row">
          <h2>🧩 Deine Tasks</h2>
          <span className="hint-text">{tasks.length} Task(s)</span>
        </div>

        {tasks.length === 0 ? (
          <p className="hint-text">Noch keine Tasks. Erstelle oben deinen ersten Task.</p>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
            {tasks.map((task) => {
              const scheduled = findScheduledTask(scheduledTasks, task.id)
              const crewName = task.crewId ? crewsById.get(task.crewId)?.name : null

              return (
                <div key={task.id} className="card">
                  <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
                      <strong>{deriveTaskName(task)}</strong>
                      <span style={{ fontSize: 11, padding: '1px 6px', borderRadius: 8, background: 'var(--accent)', color: '#fff' }}>
                        {task.runner === 'crew' ? 'Crew' : 'Modell'}
                      </span>
                      <span style={{ fontSize: 11, padding: '1px 6px', borderRadius: 8, background: task.status === 'completed' ? 'var(--success)' : task.status === 'failed' ? 'var(--danger)' : task.status === 'running' ? 'var(--accent)' : 'var(--border-color)', color: task.status === 'idle' ? 'var(--text-secondary)' : '#fff' }}>
                        {task.status}
                      </span>
                    </div>
                    <div className="actions" style={{ marginTop: 0 }}>
                      <button type="button" onClick={() => void handleOpenTaskChat(task)}>
                        Chat
                      </button>
                      <button type="button" onClick={() => void handleRunTask(task)} disabled={task.status === 'running' || !task.prompt.trim() || (task.runner === 'crew' && !task.crewId) || Boolean(task.workDir.trim() && !isAbsolutePath(task.workDir))}>
                        Start
                      </button>
                      <button type="button" className="btn-secondary" onClick={() => removeTask(task.id)} disabled={task.status === 'running'}>
                        Loeschen
                      </button>
                    </div>
                  </div>

                  <div className="grid" style={{ marginTop: 10 }}>
                    <label>
                      Titel
                      <input value={task.title} onChange={(e) => updateTask(task.id, { title: e.target.value })} />
                    </label>
                    <label>
                      Ausfuehrung
                      <select value={task.runner} onChange={(e) => updateTask(task.id, { runner: e.target.value as WorkTaskRunner })}>
                        <option value="crew">Crew</option>
                        <option value="model">Modell</option>
                      </select>
                    </label>
                    {task.runner === 'crew' ? (
                      <label>
                        Crew
                        <select value={task.crewId ?? ''} onChange={(e) => updateTask(task.id, { crewId: e.target.value || null })}>
                          <option value="">Crew waehlen</option>
                          {crews.map((crew) => (
                            <option key={crew.id} value={crew.id}>{crew.name}</option>
                          ))}
                        </select>
                        {task.crewId && !crewName ? (
                          <div className="hint-text">Zugeordnete Crew existiert nicht mehr.</div>
                        ) : null}
                      </label>
                    ) : (
                      <label>
                        Modell (optional)
                        <input
                          value={task.model}
                          onChange={(e) => updateTask(task.id, { model: e.target.value })}
                          placeholder={`Default: ${ollamaConfig.model || '—'}`}
                        />
                      </label>
                    )}
                    <label>
                      Expected Output
                      <input value={task.expectedOutput} onChange={(e) => updateTask(task.id, { expectedOutput: e.target.value })} />
                    </label>
                    <label style={{ gridColumn: '1 / -1' }}>
                      Arbeitsordner (absolut)
                      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                        <input value={task.workDir} onChange={(e) => updateTask(task.id, { workDir: e.target.value })} placeholder="C:\\Projekte\\mein-task" />
                        <button type="button" className="btn-secondary" onClick={() => void handlePickTaskWorkDir(task)}>
                          Ordner waehlen
                        </button>
                      </div>
                      {task.workDir.trim() && !isAbsolutePath(task.workDir) ? (
                        <div className="hint-text">Der Arbeitsordner muss absolut sein.</div>
                      ) : null}
                    </label>
                    <label style={{ gridColumn: '1 / -1' }}>
                      Aufgabe
                      <textarea value={task.prompt} onChange={(e) => updateTask(task.id, { prompt: e.target.value })} rows={3} />
                    </label>
                  </div>

                  <div className="card" style={{ marginTop: 10 }}>
                    <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
                      <strong>⏰ Scheduler</strong>
                      <div style={{ fontSize: 11, color: 'var(--text-muted)' }}>
                        Letzter Lauf: {formatTimestamp(scheduled?.lastRunAt ?? null)} · Naechster Lauf: {formatTimestamp(scheduled?.nextRunAt ?? null)}
                      </div>
                    </div>

                    <div className="grid" style={{ marginTop: 8 }}>
                      <label>
                        Ausdruck
                        <input
                          value={task.scheduleExpr}
                          onChange={(e) => updateTask(task.id, { scheduleExpr: e.target.value })}
                          placeholder="z.B. daily 09:00"
                        />
                      </label>
                      <label style={{ display: 'flex', flexDirection: 'column' }}>
                        Aktiv
                        <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 6 }}>
                          <input
                            type="checkbox"
                            checked={task.scheduleEnabled}
                            onChange={(e) => void handleToggleSchedule(task, e.target.checked)}
                          />
                          <span className="hint-text">Job {task.scheduleEnabled ? 'aktiv' : 'pausiert'}</span>
                        </div>
                      </label>
                    </div>
                    <div className="actions" style={{ marginTop: 10 }}>
                      <button type="button" className="btn-sm" onClick={() => void handleUpsertSchedule(task)} disabled={!task.scheduleExpr.trim()}>
                        Speichern
                      </button>
                      <button type="button" className="btn-sm" onClick={() => void handleRemoveSchedule(task)} disabled={!scheduled && !task.scheduleExpr.trim()}>
                        Entfernen
                      </button>
                      {task.runner === 'crew' && !task.crewId ? (
                        <span className="hint-text">(Crew erforderlich fuer Crew-Schedule)</span>
                      ) : null}
                    </div>
                  </div>

                  {(task.output || task.error) && (
                    <pre style={{ whiteSpace: 'pre-wrap', marginTop: 10, fontSize: 12 }}>
                      {(task.error ?? task.output ?? '').slice(0, 6000)}
                    </pre>
                  )}
                </div>
              )
            })}
          </div>
        )}
      </div>

      {templates.length > 0 && (
        <div className="panel">
          <div className="panel-heading-row">
            <h2>📦 Legacy: Templates</h2>
            <span className="hint-text">{templates.length} Template(s) im alten Speicher</span>
          </div>
          <p className="hint-text">Diese Templates werden nicht mehr aktiv genutzt. Du kannst sie hier bei Bedarf aufraeumen.</p>

          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {templates.map((template) => (
              <div key={template.id} className="card">
                <strong>{template.title?.trim() ? template.title : template.id}</strong>
                <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 4 }}>{template.description}</div>
                <div className="actions" style={{ marginTop: 10 }}>
                  <button type="button" className="btn-secondary" onClick={() => handleRemoveLegacyTemplate(template.id)}>
                    Loeschen
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
