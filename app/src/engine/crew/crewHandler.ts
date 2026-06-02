// crewHandler.ts
// Handles crew task messages from the engine store.

import { listen } from '@tauri-apps/api/event'
import { useWorkTasksStore } from '../../stores/workTasksStore'
import { resolveCrewAgentsWithProfiles, useCrewStore, type CrewPersonalityProfile } from '../../stores/crewStore'
import { useConfigStore } from '../../stores/configStore'
import { useChatStore, type CrewLiveState, type CrewLiveStatus } from '../../stores/chatStore'
import { usePersonalityStore } from '../../stores/personalityStore'
import { safeInvoke } from '../../utils/safeInvoke'
import type { EngineUserInput, ConversationHistorySeed } from '../../stores/engineStore'
import type { EngineEvent } from '../core/queryEngine'
import type { ChatProviderSelection } from '../../utils/chatProvider'
import type { PermissionMode } from '../types/tool'
import {
  appendCrewLiveEntry,
  applyCrewDefaultModel,
  buildCrewLiveMessageContent,
  buildCrewRuntimeTasks,
  buildWorkTaskCrewGuidelines,
  createCrewLiveEntry,
  resolveCrewRuntimeConfig,
  resolveExternalProviderConfig,
  type CrewExecutionLog,
  type CrewExecutionLogEvent,
  type CrewExecutionResponse,
} from '../../components/TasksView'

export interface CrewTaskMessageParams {
  userInput: EngineUserInput
  cwd: string
  onEvent?: (event: EngineEvent) => void
  historySeed?: ConversationHistorySeed
  providerSelection?: ChatProviderSelection
  permissionConfig?: { mode: PermissionMode; allowedDirectories: string[] }
  crewId: string | null
  threadId: string
  runId: string
}

function createCrewStreamId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return `crew-${crypto.randomUUID()}`
  }
  return `crew-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
}

function buildCrewRunOutput(response: CrewExecutionResponse, fallbackTaskId: string): string {
  const directResult = response.taskResults.find((result) => result.taskId === fallbackTaskId)
  if (directResult?.output?.trim()) {
    return directResult.output
  }

  const renderedResults = response.taskResults
    .filter((result) => result.output?.trim())
    .map((result) => `Task ${result.taskId} (${result.status}):\n${result.output}`)
  if (renderedResults.length > 0) {
    return renderedResults.join('\n\n')
  }

  return response.error || 'Crew-Ausfuehrung abgeschlossen.'
}

export async function handleCrewTaskMessage(params: CrewTaskMessageParams): Promise<void> {
  const { crewId, threadId } = params
  const workTasksStore = useWorkTasksStore.getState()
  const crewStore = useCrewStore.getState()

  const task = workTasksStore.tasks.find((entry) => entry.threadId === threadId)
  if (!task) {
    throw new Error('Task fuer diesen Chat nicht gefunden.')
  }

  workTasksStore.updateTask(task.id, {
    status: 'running',
    output: '',
    error: null,
  })

  let crewLiveState: CrewLiveState | null = null
  let crewLiveMessageId: string | null = null
  let unlistenCrewLogs: (() => void) | null = null
  const streamedCrewLogIds = new Set<string>()

  const publishCrewLive = (persist = false) => {
    if (!crewLiveState || !crewLiveMessageId) return
    useChatStore.getState().updateMessage(threadId, crewLiveMessageId, {
      content: buildCrewLiveMessageContent(crewLiveState),
      streaming: crewLiveState.status === 'running',
      crewLive: crewLiveState,
    }, {
      persist,
    })
  }

  const appendCrewLogToMonitor = (log: CrewExecutionLog) => {
    if (!crewLiveState || !log.id || streamedCrewLogIds.has(log.id)) return
    const entry = createCrewLiveEntry(log)
    if (!entry) return
    streamedCrewLogIds.add(log.id)
    crewLiveState = appendCrewLiveEntry(crewLiveState, entry)
    publishCrewLive()
  }

  const finishCrewLive = (status: CrewLiveStatus, persist = true) => {
    if (!crewLiveState) return
    crewLiveState = {
      ...crewLiveState,
      status,
      updatedAt: Date.now(),
    }
    publishCrewLive(persist)
  }

  try {
    if (!crewId) {
      throw new Error('Bitte eine Crew auswaehlen.')
    }

    const crew = crewStore.crews.find((entry) => entry.id === crewId)
    if (!crew) {
      throw new Error('Crew nicht gefunden (evtl. geloescht).')
    }

    const personalityState = usePersonalityStore.getState()
    if (personalityState.personalities.length === 0) {
      await personalityState.loadPersonalities()
    }
    const personalityProfiles: CrewPersonalityProfile[] = usePersonalityStore.getState().personalities.map((personality) => ({
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
    const resolvedAgents = resolveCrewAgentsWithProfiles(crew.agents, personalityProfiles)
    const enabledAgents = resolvedAgents.filter((agent) => agent.enabled)
    if (enabledAgents.length === 0) {
      throw new Error('Keine aktiven Crew-Mitglieder vorhanden.')
    }

    const configState = useConfigStore.getState()
    const ollamaConfig = configState?.ollama || { baseUrl: 'http://localhost:11434', model: 'llama3', timeoutMs: 600000 }
    const enabledAgentIds = new Set(enabledAgents.map((agent) => agent.id))
    const runtimeTasks = buildCrewRuntimeTasks(crew, task, enabledAgentIds)
    const defaultOpenAICompatibleProfile = configState.llmProfiles.find((profile) => profile.id === configState.defaultLlmProfileIds['openai-compatible'] && profile.provider === 'openai-compatible')
      ?? configState.llmProfiles.find((profile) => profile.provider === 'openai-compatible')
    const defaultOpenRouterProfile = configState.llmProfiles.find((profile) => profile.id === configState.defaultLlmProfileIds.openrouter && profile.provider === 'openrouter')
      ?? configState.llmProfiles.find((profile) => profile.provider === 'openrouter')
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
    const crewStreamId = createCrewStreamId()

    crewLiveState = {
      streamId: crewStreamId,
      title: `${task.title || task.id} - Crew-Ausfuehrung`,
      status: 'running',
      entries: [],
      agentColors: {},
      updatedAt: Date.now(),
    }
    crewLiveMessageId = useChatStore.getState().addMessage(threadId, {
      role: 'assistant',
      content: buildCrewLiveMessageContent(crewLiveState),
      timestamp: Date.now(),
      streaming: true,
      crewLive: crewLiveState,
    })

    try {
      unlistenCrewLogs = await listen<CrewExecutionLogEvent>('crew-execution-log', (event) => {
        const payload = event.payload
        if (!payload || payload.streamId !== crewStreamId) return
        appendCrewLogToMonitor(payload.log)
      })
    } catch {
      // Browser-only tests and fallback environments do not expose the Tauri event bus.
    }

    const response = await safeInvoke<CrewExecutionResponse>('crew_execute', {
      request: {
        id: crew.id,
        streamId: crewStreamId,
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
        process: crew.process,
        managerAgentId: crew.managerAgentId,
        verbose: crew.verbose,
        maxRpm: crew.maxRpm,
        maxParallelTasks: crew.maxParallelTasks,
        providerConfigs,
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
          tools: agent.tools,
          mcpServerNames: agent.mcpServerNames,
          enabled: agent.enabled,
          allowDelegation: agent.allowDelegation,
          verbose: agent.verbose,
          maxIterations: agent.maxIterations,
        })),
        tasks: runtimeTasks,
        cwd: params.cwd || null,
        config,
      },
    })

    const mappedStatus = response.status === 'completed' ? 'completed' : 'failed'
    for (const log of response.logs) {
      appendCrewLogToMonitor(log)
    }
    finishCrewLive(mappedStatus === 'completed' ? 'completed' : 'failed')
    const output = buildCrewRunOutput(response, task.id)

    workTasksStore.updateTask(task.id, {
      status: mappedStatus,
      output,
      error: response.error ?? null,
      lastRunAt: Date.now(),
    })

    useChatStore.getState().addMessage(threadId, {
      role: 'assistant',
      content: output,
      timestamp: Date.now(),
    })
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    finishCrewLive('failed')

    workTasksStore.updateTask(task.id, {
      status: 'failed',
      error: message,
      output: message,
      lastRunAt: Date.now(),
    })

    useChatStore.getState().addMessage(threadId, {
      role: 'assistant',
      content: `Fehler: ${message}`,
      timestamp: Date.now(),
    })
  } finally {
    unlistenCrewLogs?.()
  }
}
