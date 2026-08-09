// crewHandler.ts
// Handles crew task messages from the engine store.

import { useWorkTasksStore, type WorkTask } from '../../stores/workTasksStore'
import { resolveCrewAgentsWithProfiles, useCrewStore, type CrewPersonalityProfile } from '../../stores/crewStore'
import { useConfigStore } from '../../stores/configStore'
import { useChatStore, type CrewLiveState, type CrewLiveStatus } from '../../stores/chatStore'
import { usePersonalityStore } from '../../stores/personalityStore'
import { createDurableCrewRun } from '../../runtime/localDaemonExecution'
import { attachDurableLocalRun } from '../../runtime/localDaemonChat'
import type { EngineUserInput, ConversationHistorySeed } from '../../stores/engineStore'
import type { EngineEvent } from '../core/queryEngine'
import type { ChatProviderSelection } from '../../utils/chatProvider'
import type { PermissionMode } from '../types/tool'
import type { AuthorizedTaskPath } from '../../utils/taskProjectContext'
import i18n from '../../i18n'
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
  type CrewExecutionResponse,
  type CrewResolvedProviderConfigs,
} from './workTaskCrewRuntime'

export interface CrewTaskMessageParams {
  userInput: EngineUserInput
  cwd: string
  onEvent?: (event: EngineEvent) => void
  historySeed?: ConversationHistorySeed
  providerSelection?: ChatProviderSelection
  permissionConfig?: {
    mode: PermissionMode
    allowedDirectories: string[]
    authorizedPaths?: AuthorizedTaskPath[]
  }
  crewId: string | null
  threadId: string
  runId: string
  securityMode: 'windows_native_elevated' | 'host_read_only_broker'
}

const READ_ONLY_BLOCKED_CREW_TOOL = /(bash|shell|terminal|powershell|cmd|write|edit|delete|remove|rename|move|copy|create.?directory|office|python|save.?skill|desktop)/i

function secureCrewTools(
  tools: string[],
  mode: CrewTaskMessageParams['securityMode'],
): string[] {
  if (mode === 'windows_native_elevated') return tools
  return tools.filter((tool) => !READ_ONLY_BLOCKED_CREW_TOOL.test(tool))
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

  return response.error || 'Crew-Execution abclosed.'
}

function extractCrewChatPrompt(userInput: EngineUserInput): string {
  if (typeof userInput === 'string') return userInput.trim()
  return userInput
    .filter((block): block is Extract<(typeof userInput)[number], { type: 'text' }> => block.type === 'text')
    .map((block) => block.text.trim())
    .filter(Boolean)
    .join('\n\n')
}

export function createChatCrewWorkTask(params: Pick<CrewTaskMessageParams, 'userInput' | 'cwd' | 'threadId' | 'crewId' | 'runId'>): WorkTask {
  const prompt = extractCrewChatPrompt(params.userInput) || 'Complete the user request from this chat.'
  const title = prompt.replace(/\s+/g, ' ').trim().slice(0, 80) || 'Crew chat request'
  const now = Date.now()
  return {
    id: `chat-${params.runId}`,
    title,
    prompt,
    expectedOutput: 'Return the complete final result to the chat.',
    workDir: params.cwd,
    threadId: params.threadId,
    runner: 'crew',
    crewId: params.crewId,
    model: '',
    scheduleExpr: '',
    scheduleEnabled: false,
    status: 'idle',
    output: null,
    error: null,
    lastRunAt: null,
    createdAt: now,
    updatedAt: now,
  }
}

export async function handleCrewTaskMessage(params: CrewTaskMessageParams): Promise<void> {
  const { crewId, threadId } = params
  const workTasksStore = useWorkTasksStore.getState()
  const crewStore = useCrewStore.getState()

  const persistedTask = workTasksStore.tasks.find((entry) => entry.threadId === threadId) ?? null
  const task = persistedTask ?? createChatCrewWorkTask(params)

  if (persistedTask) {
    workTasksStore.updateTask(task.id, {
      status: 'running',
      output: '',
      error: null,
    })
  }

  let crewLiveState: CrewLiveState | null = null
  let crewLiveMessageId: string | null = null
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
      throw new Error('Please select a crew.')
    }

    const crew = crewStore.crews.find((entry) => entry.id === crewId)
    if (!crew) {
      throw new Error('Crew not found (possibly deleted).')
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
      throw new Error('No active Crew-members available.')
    }

    const configState = useConfigStore.getState()
    const ollamaConfig = configState?.ollama || { baseUrl: 'http://localhost:11434', model: 'llama3', timeoutMs: 600000 }
    const enabledAgentIds = new Set(enabledAgents.map((agent) => agent.id))
    const runtimeTasks = buildCrewRuntimeTasks(crew, task, enabledAgentIds)
    const defaultOpenAICompatibleProfile = configState.llmProfiles.find((profile) => profile.id === configState.defaultLlmProfileIds['openai-compatible'] && profile.preset === 'openai')
      ?? configState.llmProfiles.find((profile) => profile.preset === 'openai')
    const defaultOpenRouterProfile = configState.llmProfiles.find((profile) => profile.id === configState.defaultLlmProfileIds.openrouter && profile.preset === 'openrouter')
      ?? configState.llmProfiles.find((profile) => profile.preset === 'openrouter')
    let providerConfigs: CrewResolvedProviderConfigs = {
      openAICompatible: resolveExternalProviderConfig(
        crew.providerProfiles.openAICompatible,
        defaultOpenAICompatibleProfile,
        defaultOpenAICompatibleProfile?.baseUrl || crew.providerProfiles.openAICompatible.baseUrl || 'https://api.openai.com/v1',
        defaultOpenAICompatibleProfile ? configState.llmProfileModels[defaultOpenAICompatibleProfile.id] ?? [] : [],
      ),
      openRouter: resolveExternalProviderConfig(
        crew.providerProfiles.openRouter,
        defaultOpenRouterProfile,
        defaultOpenRouterProfile?.baseUrl || crew.providerProfiles.openRouter.baseUrl || 'https://openrouter.ai/api/v1',
        defaultOpenRouterProfile ? configState.llmProfileModels[defaultOpenRouterProfile.id] ?? [] : [],
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
      title: `${task.title || task.id} - Crew-Execution`,
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

    const crewRequest = {
        id: crew.id,
        streamId: crewStreamId,
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
          tools: secureCrewTools(augmentCrewToolsForTask(agent.tools, task), params.securityMode),
          mcpServerNames: agent.mcpServerNames,
          enabled: agent.enabled,
          allowDelegation: agent.allowDelegation,
          verbose: agent.verbose,
          maxIterations: agent.maxIterations,
        })),
        tasks: runtimeTasks,
        cwd: params.cwd || null,
        ...(params.permissionConfig?.authorizedPaths !== undefined
          ? { authorizedPaths: params.permissionConfig.authorizedPaths }
          : {}),
        config,
    }
    const resultMessageId = useChatStore.getState().addMessage(threadId, {
      role: 'assistant',
      content: '',
      timestamp: Date.now(),
      streaming: true,
    })
    const { client, run } = await createDurableCrewRun({
      clientThreadId: threadId,
      clientProjectId: params.cwd || `chat:${threadId}`,
      clientTaskId: persistedTask?.id ?? null,
      assistantMessageId: resultMessageId,
      crewLiveMessageId,
      crewLiveTitle: crewLiveState.title,
      prompt: task.prompt,
      workspacePath: params.cwd || null,
      crewId: crew.id,
      crewRequest,
      source: 'chat',
    })
    const finalRun = await attachDurableLocalRun(client, run)
    const response = ((finalRun.result as Record<string, unknown> | null)?.crew_response
      ?? null) as CrewExecutionResponse | null
    if (!response) {
      throw new Error(finalRun.error?.message || `Crew run ended in state ${finalRun.state}`)
    }

    const mappedStatus = response.status === 'completed' ? 'completed' : 'failed'
    for (const log of response.logs) {
      appendCrewLogToMonitor(log)
    }
    finishCrewLive(mappedStatus === 'completed' ? 'completed' : 'failed')
    const output = buildCrewRunOutput(response, task.id)

    if (persistedTask) {
      workTasksStore.updateTask(task.id, {
        status: mappedStatus,
        output,
        error: response.error ?? null,
        lastRunAt: Date.now(),
      })
    }

    useChatStore.getState().updateMessage(threadId, resultMessageId, {
      content: output,
      streaming: false,
    }, { persist: true })
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    finishCrewLive('failed')

    if (persistedTask) {
      workTasksStore.updateTask(task.id, {
        status: 'failed',
        error: message,
        output: message,
        lastRunAt: Date.now(),
      })
    }

    useChatStore.getState().addMessage(threadId, {
      role: 'assistant',
      content: `Error: ${message}`,
      timestamp: Date.now(),
    })
  } finally {
    // The per-user daemon owns the Crew subprocess and its lifecycle.
  }
}
