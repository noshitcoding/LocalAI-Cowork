// ── Engine Store (Zustand) ──────────────────────────────────────────────────
// Main Zustand binding for the integrated Ollama-first engine.
// Wraps QueryEngine in a reactive Zustand store for UI binding.

import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { invoke } from '@tauri-apps/api/core'
import type { EngineBackend, EngineConfig, QueryEngine } from '../engine/core/queryEngine'
import type { EngineEvent } from '../engine/core/queryEngine'
import { getAllCommands, registerBuiltinCommands } from '../engine/commands/registry'
import { listOllamaModels, checkOllamaConnection } from '../engine/api/ollamaClient'
import { buildSystemPromptWithMemory } from '../engine/memory/memorySystem'
import type { ContextSnapshot } from '../engine/services/contextManager'
import {
  createAssistantMessage,
  createInitialAppState,
  createSystemMessage,
  createUserMessage,
  EMPTY_USAGE,
  extractTextContent,
  type AgentDefinition,
  type ApprovalResult,
  type AppState,
  type ContentBlock,
  type Message,
  type TokenUsage,
  type ToolUIRequest,
} from '../engine/types'
import {
  autoSaveSession,
  createSession,
  deleteSession,
  generateSessionTitle,
  loadSession,
  listSessions,
  type SessionRecord,
  type SessionSummary,
} from '../engine/services/sessionPersistence'
import { useConfigStore } from './configStore'
import { useChatStore } from './chatStore'
import { parsePersistedSessionMessage } from '../utils/sessionThreads'
import { getChatProviderState, normalizeChatProvider, type ChatProviderKind, type ChatProviderSelection } from '../utils/chatProvider'
import type { PermissionMode } from '../engine/types/tool'

const DEFAULT_SYSTEM_PROMPT = `Du bist ein hilfreicher KI-Assistent in einer Desktop-Anwendung (Open Cowork). Du hast Zugriff auf verschiedene Tools um Dateien zu lesen, zu schreiben, zu suchen, Shell-Befehle auszufuehren, und mehr.

Wichtige Regeln:
1. Fuehre Aenderungen direkt aus statt nur Vorschlaege zu machen, es sei denn der Plan-Modus ist aktiv.
2. Lies Dateien bevor du sie aenderst, um den Kontext zu verstehen.
3. Nutze Tools haeufig — lese, suche, und verifiziere.
4. Gib klare, praezise Antworten.
5. Bei Unsicherheit: frage den Benutzer mit dem AskUser-Tool.
  Wenn das Ziel jedoch klar ist (z. B. Dateien sortieren/strukturieren), fuehre es selbststaendig aus und frage nur bei fehlenden kritischen Angaben oder bei destruktiven Schritten.
6. Erstelle keine Dateien, die nicht benoetigt werden.
7. Teste Aenderungen wenn moeglich (Build, Tests etc.).
8. Fuer Dateiorganisation und Strukturarbeit nutze dedizierte Datei-Tools wie ListDir, CreateDirectory, MovePath und CopyPath statt nur Shell-Befehle zu beschreiben.

Du arbeitest in einem Windows-Umfeld mit PowerShell.`

const DEFAULT_AGENTS: AgentDefinition[] = [
  {
    id: 'coder',
    name: 'Coder',
    description: 'Spezialisiert auf Code-Implementierung und Debugging.',
    type: 'coding',
    tools: ['Read', 'Write', 'Edit', 'Bash', 'Glob', 'Grep'],
    maxTurns: 15,
  },
  {
    id: 'researcher',
    name: 'Researcher',
    description: 'Spezialisiert auf Recherche und Analyse.',
    type: 'research',
    tools: ['Read', 'Glob', 'Grep', 'WebFetch', 'WebSearch', 'MemoryRead'],
    maxTurns: 10,
  },
  {
    id: 'reviewer',
    name: 'Reviewer',
    description: 'Spezialisiert auf Code-Review und Qualitaetssicherung.',
    type: 'review',
    tools: ['Read', 'Glob', 'Grep', 'Bash'],
    maxTurns: 8,
  },
  {
    id: 'planner',
    name: 'Planner',
    description: 'Spezialisiert auf Projektplanung und Aufgabenzerlegung.',
    type: 'planning',
    tools: ['Read', 'Glob', 'TaskCreate', 'TaskList', 'MemoryRead', 'MemoryWrite'],
    maxTurns: 5,
  },
]

// ── Types ──────────────────────────────────────────────────────────────────

export type EngineStatus = 'idle' | 'streaming' | 'tool_running' | 'waiting_approval' | 'error'
export type EngineProvider = ChatProviderKind

export type ToolExecution = {
  id: string
  toolName: string
  input: Record<string, unknown>
  status: 'running' | 'completed' | 'failed'
  result?: string
  startedAt: number
}

export type EngineStoreConfig = {
  apiKey: string
  model: string
  systemPrompt: string
  maxTurns: number
  maxBudgetUsd: number
  permissionMode: 'default' | 'plan' | 'bypass' | 'strict'
  thinkingEnabled: boolean
  thinkingBudget: number
  autoCompact: boolean
  appendSystemPrompt: string
  // Ollama-specific (persisted alongside configStore)
  ollamaBaseUrl: string
  ollamaModel: string
  sessionPersistence: boolean
}

export type ContextWarning = {
  level: 'none' | 'low' | 'medium' | 'high' | 'critical'
  estimatedTokens: number
}

export type ChatHistorySeedMessage = {
  role: 'user' | 'assistant' | 'system'
  content: string | ContentBlock[]
  debugContent?: string
}

export type ConversationHistorySeed = {
  threadId: string | null
  messages: ChatHistorySeedMessage[]
}

export type EngineUserInput = string | ContentBlock[]

function extractUserInputText(userInput: EngineUserInput): string {
  if (typeof userInput === 'string') {
    return userInput.trim()
  }

  const text = userInput
    .filter((block): block is Extract<ContentBlock, { type: 'text' }> => block.type === 'text')
    .map((block) => block.text.trim())
    .filter(Boolean)
    .join('\n\n')

  const imageCount = userInput.filter((block) => block.type === 'image').length
  if (!text && imageCount > 0) {
    return imageCount === 1 ? '[1 Bild-Anhang]' : `[${imageCount} Bild-Anhaenge]`
  }

  if (text && imageCount > 0) {
    const suffix = imageCount === 1 ? '[1 Bild-Anhang]' : `[${imageCount} Bild-Anhaenge]`
    return `${text}\n\n${suffix}`
  }

  return text
}

function extractSeedMessageText(message: ChatHistorySeedMessage): string {
  if (typeof message.content === 'string') {
    return message.content.trim()
  }

  return extractTextContent({
    type: message.role,
    uuid: 'seed-message',
    content: message.content,
    timestamp: 0,
    ...(message.role === 'assistant'
      ? { model: 'seed', usage: { ...EMPTY_USAGE }, stopReason: 'end_turn' as const }
      : {}),
  } as Message).trim()
}

export type EngineStoreState = {
  // ── Engine State ───────────────────────────────────────────────────────
  status: EngineStatus
  streamingText: string
  thinkingText: string
  messages: Message[]
  appState: AppState
  totalUsage: TokenUsage
  totalCostUsd: number
  currentToolUI: ToolUIRequest | null
  activeTools: ToolExecution[]
  error: string | null
  activeProvider: EngineProvider
  setActiveProvider: (provider: EngineProvider) => void

  // ── Context & Compaction State ─────────────────────────────────────────
  contextWarning: ContextWarning
  compactionCount: number
  contextSnapshot: ContextSnapshot | null

  // ── Session State ──────────────────────────────────────────────────────
  currentSessionId: string | null
  currentRunId: string | null
  conversationThreadId: string | null

  // ── Configuration ──────────────────────────────────────────────────────
  config: EngineStoreConfig
  setConfig: (patch: Partial<EngineStoreConfig>) => void
  setApiKey: (apiKey: string) => void

  // ── Engine Actions ─────────────────────────────────────────────────────
  sendMessage: (
    userInput: EngineUserInput,
    cwd: string,
    onEvent?: (event: EngineEvent) => void,
    historySeed?: ConversationHistorySeed,
    providerSelection?: ChatProviderSelection,
    permissionConfig?: { mode: PermissionMode; allowedDirectories: string[] },
  ) => Promise<void>
  abort: () => void
  resolveApproval: (result: ApprovalResult) => void
  clearCurrentToolUI: () => void
  clearMessages: () => void
  clearError: () => void
  // ── Crew Task Message Handler ─────────────────────────────────────
  crewTaskMessageHandler: ((
    params: {
      userInput: EngineUserInput
      cwd: string
      onEvent?: (event: EngineEvent) => void
      historySeed?: ConversationHistorySeed
      providerSelection?: ChatProviderSelection
      permissionConfig?: { mode: PermissionMode; allowedDirectories: string[] }
      crewId: string | null
      threadId: string
      runId: string
    },
  ) => Promise<void>) | null
  setCrewTaskMessageHandler: (handler: ((
    params: {
      userInput: EngineUserInput
      cwd: string
      onEvent?: (event: EngineEvent) => void
      historySeed?: ConversationHistorySeed
      providerSelection?: ChatProviderSelection
      permissionConfig?: { mode: PermissionMode; allowedDirectories: string[] }
      crewId: string | null
      threadId: string
      runId: string
    },
  ) => Promise<void>) | null) => void
  // ── New Actions (CC features) ──────────────────────────────────────────
  forceCompact: () => Promise<void>
  getContextSnapshot: () => ContextSnapshot | null
  loadSessionById: (sessionId: string) => Promise<SessionRecord | null>
  getSessions: () => Promise<SessionSummary[]>
  deleteSessionById: (sessionId: string) => Promise<void>
  fetchOllamaModels: () => Promise<Array<{ id: string; name: string; size: number }>>
  checkOllamaStatus: () => Promise<boolean>

  // ── Internal ───────────────────────────────────────────────────────────
  _engine: QueryEngine | null
  _initEngine: (cwd: string, providerSelection?: ChatProviderSelection, permissionConfig?: { mode: PermissionMode; allowedDirectories: string[] }) => Promise<QueryEngine>
}

// ── Store ──────────────────────────────────────────────────────────────────

// Register commands once at module load
let commandsRegistered = false
function ensureCommandsRegistered() {
  if (!commandsRegistered) {
    registerBuiltinCommands()
    commandsRegistered = true
  }
}

let sendMessageQueue: Promise<void> = Promise.resolve()
const MIN_THINKING_TIMEOUT_MS = 600000

function mapChatHistorySeedToEngineMessages(
  seedMessages: ChatHistorySeedMessage[],
  model: string,
): Message[] {
  return seedMessages.reduce<Message[]>((acc, message) => {
    // Try to parse structured content from debugContent or content
    const textContent = extractSeedMessageText(message)
    const rawContent = message.debugContent?.trim() || textContent
    const structuredMessage = parsePersistedSessionMessage(rawContent)

    if (structuredMessage) {
      acc.push(structuredMessage)
      return acc
    }

    // Try to parse content as JSON array (structured content blocks)
    if (typeof message.content === 'string' && message.content.trim().startsWith('[')) {
      try {
        const parsedContent = JSON.parse(message.content.trim())
        if (Array.isArray(parsedContent) && parsedContent.length > 0) {
          const isValidContentBlock = parsedContent.every(
            (block: unknown) => typeof block === 'object' && block !== null && 'type' in block,
          )
          if (isValidContentBlock) {
            const assistantMsg = createAssistantMessage(
              parsedContent as ContentBlock[],
              model,
              { ...EMPTY_USAGE },
              'end_turn',
            )
            acc.push(assistantMsg)
            return acc
          }
        }
      } catch {
        // Not valid JSON array, fall through to text handling
      }
    }

    const preferredContent = message.role === 'user'
      ? (message.debugContent?.trim() || textContent)
      : textContent

    if (!preferredContent) return acc

    switch (message.role) {
      case 'user':
        acc.push(createUserMessage(preferredContent))
        return acc
      case 'assistant':
        acc.push(createAssistantMessage([{ type: 'text', text: preferredContent }], model, { ...EMPTY_USAGE }, 'end_turn'))
        return acc
      case 'system':
        acc.push(createSystemMessage(preferredContent))
        return acc
      default:
        return acc
    }
  }, [])
}

function getResolvedProvider(provider: unknown): EngineBackend {
  return normalizeChatProvider(provider)
}

function buildChatEngineConfig(
  provider: EngineBackend,
  config: EngineStoreConfig,
  cwd: string,
  runId?: string,
  sessionId?: string,
  providerSelection?: ChatProviderSelection,
  permissionConfig?: { mode: PermissionMode; allowedDirectories: string[] },
): EngineConfig {
  const configState = useConfigStore.getState()
  const providerState = getChatProviderState(configState, provider, providerSelection)
  const ollamaConfig = configState.ollama
  const effectiveThinkingEnabled = true
  const effectiveOllamaTimeoutMs = Math.max(providerState.timeoutMs, MIN_THINKING_TIMEOUT_MS)

  return {
    backend: provider,
    anthropic: {
      apiKey: config.apiKey,
      model: config.model,
      thinking: effectiveThinkingEnabled
        ? { type: 'enabled', budgetTokens: config.thinkingBudget }
        : { type: 'disabled' },
    },
    ollama: {
      baseUrl: providerState.provider === 'ollama' ? providerState.endpoint : ollamaConfig.baseUrl,
      model: providerState.provider === 'ollama' ? providerState.model : ollamaConfig.model,
      temperature: ollamaConfig.temperature,
      contextWindow: ollamaConfig.contextWindow,
      timeoutMs: effectiveOllamaTimeoutMs,
      thinkingEnabled: effectiveThinkingEnabled,
    },
    openAiCompatible: provider === 'openai-compatible' || provider === 'openrouter'
      ? {
          provider,
          apiKey: providerState.apiKey,
          baseUrl: providerState.endpoint,
          model: providerState.model,
          timeoutMs: providerState.timeoutMs,
          verifyTlsCertificates: providerState.verifyTlsCertificates,
        }
      : undefined,
    cwd,
    systemPrompt: config.systemPrompt || DEFAULT_SYSTEM_PROMPT,
    maxTurns: config.maxTurns,
    maxBudgetUsd: config.maxBudgetUsd,
    permissionMode: permissionConfig?.mode ?? config.permissionMode,
    allowedDirectories: permissionConfig?.allowedDirectories ?? [],
    commands: getAllCommands(),
    agentDefinitions: DEFAULT_AGENTS,
    appendSystemPrompt: config.appendSystemPrompt,
    runId,
    sessionId,
  }
}

export const useEngineStore = create<EngineStoreState>()(
  persist(
    (set, get) => ({
      // ── Default State ────────────────────────────────────────────────────
      status: 'idle',
      streamingText: '',
      thinkingText: '',
      messages: [],
      appState: createInitialAppState(''),
      totalUsage: { ...EMPTY_USAGE },
      totalCostUsd: 0,
      currentToolUI: null,
      activeTools: [],
      error: null,
      activeProvider: 'ollama',

      // Context & Compaction
      contextWarning: { level: 'none', estimatedTokens: 0 },
      compactionCount: 0,
      contextSnapshot: null,

      // Session
      currentSessionId: null,
      currentRunId: null,
      conversationThreadId: null,

      config: {
        apiKey: '',
        model: 'claude-sonnet-4-20250514',
        systemPrompt: DEFAULT_SYSTEM_PROMPT,
        maxTurns: 25,
        maxBudgetUsd: 0,
        permissionMode: 'default' as const,
        thinkingEnabled: true,
        thinkingBudget: 10000,
        autoCompact: true,
        appendSystemPrompt: '',
        ollamaBaseUrl: 'http://localhost:11434',
        ollamaModel: 'llama3.1:8b',
        sessionPersistence: true,
      },

      _engine: null,
      // ── Crew Task Message Handler ──────────────────────────────────
      crewTaskMessageHandler: null,
      setCrewTaskMessageHandler: (handler) => set({ crewTaskMessageHandler: handler }),
      // ── Config ───────────────────────────────────────────────────────────
      setActiveProvider: (provider) => set({ activeProvider: normalizeChatProvider(provider) }),
      setConfig: (patch) => set((s) => ({ config: { ...s.config, ...patch } })),
      setApiKey: (apiKey) => set((s) => ({ config: { ...s.config, apiKey } })),

      // ── Init Engine ──────────────────────────────────────────────────────
      _initEngine: async (cwd: string, providerSelection?: ChatProviderSelection, permissionConfig?: { mode: PermissionMode; allowedDirectories: string[] }): Promise<QueryEngine> => {
        ensureCommandsRegistered()

        const { QueryEngine } = await import('../engine/core/queryEngine')

        const { config, activeProvider, currentRunId, currentSessionId } = get()
        const providerState = getChatProviderState(useConfigStore.getState(), activeProvider, providerSelection)
        const engineConfig = buildChatEngineConfig(
          getResolvedProvider(providerState.provider),
          config,
          cwd,
          currentRunId ?? undefined,
          currentSessionId ?? undefined,
          providerSelection,
          permissionConfig,
        )

        const engine = new QueryEngine(engineConfig)

        // Wire tool UI callback
        engine.setToolUICallback((ui) => {
          set({ currentToolUI: ui })
        })

        set({ _engine: engine })
        return engine
      },

      // ── Send Message ─────────────────────────────────────────────────────
      sendMessage: async (userInput, cwd, onEvent, historySeed, providerSelection, permissionConfig) => {
        const queuedRun = sendMessageQueue
          .catch(() => undefined)
          .then(async () => {
            let state = get()
            if (state.status !== 'idle') {
              if (!state.currentRunId) {
                set({ status: 'idle', error: null })
                state = get()
              } else {
                throw new Error('Die Engine verarbeitet bereits eine andere Anfrage.')
              }
            }

            // Check if active thread is a crew task
            const chatState = useChatStore.getState()
            const activeThread = chatState.threads.find(t => t.id === chatState.activeThreadId)
            const isCrewTask = activeThread?.runner === 'crew' && activeThread?.crewId

            // If this is a crew task, delegate to crew handler
            if (isCrewTask && get().crewTaskMessageHandler) {
              const runId = crypto.randomUUID()
              set({
                status: 'streaming',
                streamingText: '',
                thinkingText: '',
                error: null,
                activeTools: [],
                currentToolUI: null,
                currentRunId: runId,
              })
              
              return get().crewTaskMessageHandler!({
                userInput,
                cwd,
                onEvent,
                historySeed,
                providerSelection,
                permissionConfig,
                crewId: activeThread!.crewId!,
                threadId: activeThread!.id,
                runId,
              })
            }

            const runId = crypto.randomUUID()
            set({
              status: 'streaming',
              streamingText: '',
              thinkingText: '',
              error: null,
              activeTools: [],
              currentToolUI: null,
              currentRunId: runId,
            })

            // Get or create engine
            const latestStore = get()
            const userInputText = extractUserInputText(userInput)
            const providerState = getChatProviderState(useConfigStore.getState(), latestStore.activeProvider, providerSelection)
            const provider = getResolvedProvider(providerState.provider)
            let engine = state._engine
            if (!engine) {
              engine = await state._initEngine(cwd, providerSelection, permissionConfig)
            } else {
              engine.updateConfig(buildChatEngineConfig(
                provider,
                latestStore.config,
                cwd,
                runId,
                latestStore.currentSessionId ?? undefined,
                providerSelection,
                permissionConfig,
              ))
            }

            const shouldHydrateHistory = Boolean(
              historySeed &&
              Array.isArray(historySeed.messages) &&
              historySeed.messages.length > 0 &&
              (
                latestStore.messages.length === 0 ||
                historySeed.threadId !== latestStore.conversationThreadId
              ),
            )

            if (shouldHydrateHistory) {
              const hydratedMessages = mapChatHistorySeedToEngineMessages(historySeed!.messages, providerState.model)
              set({
                messages: hydratedMessages,
                conversationThreadId: historySeed?.threadId ?? null,
                currentSessionId: null,
                contextSnapshot: engine.getContextSnapshot(hydratedMessages),
              })
            } else if (historySeed?.threadId && historySeed.threadId !== latestStore.conversationThreadId) {
              set({ conversationThreadId: historySeed.threadId })
            }

            // Load project memory and build enhanced system prompt
            try {
              const { systemPrompt, memoryContent } = await buildSystemPromptWithMemory(
                cwd,
                latestStore.config.systemPrompt || DEFAULT_SYSTEM_PROMPT,
                { userInput: userInputText },
              )
              engine.updateConfig({
                systemPrompt,
                memoryContent,
              })
            } catch {
              // Memory loading is optional — fall back to default prompt
            }

            void invoke('engine_run_create', {
              request: {
                id: runId,
                sessionId: get().currentSessionId,
                title: userInputText.slice(0, 120) || 'Engine Run',
                inputSummary: userInputText.slice(0, 1000),
                status: 'running',
                phase: 'llm_turn',
                cwd,
                model: providerState.model,
                provider,
                metadataJson: JSON.stringify({
                  permissionMode: latestStore.config.permissionMode,
                  maxTurns: latestStore.config.maxTurns,
                }),
              },
            }).catch(() => {})

            void invoke('memory_upsert', {
              id: crypto.randomUUID(),
              scope: 'session',
              category: 'run_input',
              key: runId,
              content: userInputText,
              sourceSessionId: runId,
              confidence: 1,
            }).catch(() => {})

            try {
              const currentMessages = get().messages
              const query = engine.query(currentMessages, userInput)

              for await (const event of query) {
                // Forward to external listener
                onEvent?.(event)

                switch (event.type) {
                  case 'text_delta':
                    set((s) => ({ streamingText: s.streamingText + event.text }))
                    break

                  case 'thinking_delta':
                    set((s) => ({ thinkingText: s.thinkingText + event.thinking }))
                    break

                  case 'tool_use_start':
                    void invoke('engine_run_update', {
                      request: {
                        id: runId,
                        phase: `tool:${event.toolName}`,
                        metadataJson: JSON.stringify({ activeTool: event.toolName, input: event.input }),
                      },
                    }).catch(() => {})
                    set((s) => ({
                      status: 'tool_running',
                      activeTools: [...s.activeTools, {
                        id: event.toolUseId,
                        toolName: event.toolName,
                        input: event.input,
                        status: 'running',
                        startedAt: Date.now(),
                      }],
                    }))
                    break

                  case 'tool_use_complete':
                    set((s) => ({
                      activeTools: s.activeTools.map(t =>
                        t.id === event.toolUseId
                          ? { ...t, status: 'completed' as const, result: event.result }
                          : t,
                      ),
                    }))
                    break

                  case 'approval_required':
                    set({ status: 'waiting_approval' })
                    break

                  case 'usage_update':
                    set({
                      totalUsage: event.usage,
                      totalCostUsd: event.totalCostUsd,
                    })
                    break

                  case 'assistant_message':
                    void invoke('engine_run_checkpoint_add', {
                      request: {
                        runId,
                        label: `assistant-turn-${Date.now()}`,
                        snapshotJson: JSON.stringify({
                          turnCount: engine!.getAppState().turnCount,
                          lastAssistant: extractTextContent(event.message).slice(0, 4000),
                        }),
                      },
                    }).catch(() => {})
                    set((s) => ({
                      messages: [...s.messages, event.message],
                      streamingText: '',
                      thinkingText: '',
                      status: 'streaming',
                    }))
                    break

                  case 'turn_complete':
                    if (event.stopReason === 'tool_use') {
                      set({ status: 'streaming' })
                    } else if (event.stopReason === 'await_user') {
                      set({ status: 'idle', currentToolUI: null })
                    }
                    break

                  case 'error':
                    // Treat user-initiated abort as a clean stop, not an error
                    if (event.error === 'Abgebrochen.') {
                      set({ status: 'idle', currentRunId: null })
                      break
                    }
                    void invoke('engine_run_update', {
                      request: {
                        id: runId,
                        status: 'failed',
                        phase: 'error',
                        error: event.error,
                      },
                    }).catch(() => {})
                    set({ error: event.error, status: 'error', currentRunId: null })
                    break

                  case 'done':
                    {
                      const lastAssistant = [...event.messages]
                        .reverse()
                        .find((message) => message.type === 'assistant')
                      const summary = lastAssistant ? extractTextContent(lastAssistant).slice(0, 2000) : ''
                      const checkpointJson = JSON.stringify({
                        turnCount: engine!.getAppState().turnCount,
                        totalCostUsd: event.totalCostUsd,
                        totalUsage: event.totalUsage,
                        messageCount: event.messages.length,
                      })
                      void invoke('engine_run_update', {
                        request: {
                          id: runId,
                          status: 'completed',
                          phase: 'completed',
                          checkpointJson,
                          resultSummary: summary,
                        },
                      }).catch(() => {})
                      void invoke('memory_upsert', {
                        id: crypto.randomUUID(),
                        scope: 'session',
                        category: 'run_output',
                        key: runId,
                        content: summary || 'Run completed.',
                        sourceSessionId: runId,
                        confidence: 0.9,
                      }).catch(() => {})
                    }
                    set({
                      messages: event.messages,
                      totalUsage: event.totalUsage,
                      totalCostUsd: event.totalCostUsd,
                      status: 'idle',
                      streamingText: '',
                      thinkingText: '',
                      activeTools: [],
                      appState: engine!.getAppState(),
                      currentRunId: null,
                      conversationThreadId: historySeed?.threadId ?? get().conversationThreadId,
                    })

                    // Auto-save session after completion
                    if (get().config.sessionPersistence) {
                      const doneMessages = event.messages
                      const sessionId = get().currentSessionId ?? crypto.randomUUID()
                      const title = generateSessionTitle(doneMessages)
                      const threadId = get().conversationThreadId
                      void createSession({
                        id: sessionId,
                        threadId: threadId ?? undefined,
                        title,
                        model: providerState.model,
                        provider: providerState.provider,
                      }).catch(() => { /* optional */ })
                      void autoSaveSession(
                        sessionId,
                        title,
                        cwd,
                        doneMessages,
                        event.totalUsage,
                        event.totalCostUsd,
                        engine!.getAppState(),
                        threadId ?? undefined,
                      )
                        .then(() => set({ currentSessionId: sessionId }))
                        .catch(() => { /* session save optional */ })
                    }

                    // Update context snapshot
                    try {
                      const snap = engine!.getContextSnapshot(event.messages)
                      set({ contextSnapshot: snap })
                    } catch { /* optional */ }
                    break

                  case 'compaction':
                    set((s) => ({ compactionCount: s.compactionCount + 1 }))
                    break

                  case 'context_warning':
                    set({
                      contextWarning: {
                        level: event.level === 'warning' ? 'high' : event.level,
                        estimatedTokens: event.estimatedTokens,
                      },
                    })
                    break

                  case 'retry':
                    // Retry events are informational — forwarded to onEvent
                    break
                }
              }
            } catch (err) {
              const msg = err instanceof Error ? err.message : String(err)
              void invoke('engine_run_update', {
                request: {
                  id: runId,
                  status: 'failed',
                  phase: 'error',
                  error: msg,
                },
              }).catch(() => {})
              set({ error: msg, status: 'error', currentRunId: null })
            } finally {
              if (get().status !== 'idle') {
                set({ status: 'idle' })
              }
            }
          })

        sendMessageQueue = queuedRun.then(() => undefined, () => undefined)
        return queuedRun
      },

      // ── Abort ────────────────────────────────────────────────────────────
      abort: () => {
        const { _engine: engine, currentRunId } = get()
        if (engine) engine.abort()
        if (currentRunId) {
          void invoke('engine_run_cancel', { id: currentRunId }).catch(() => {})
        }
        set({ status: 'idle', streamingText: '', thinkingText: '', currentRunId: null })
      },

      // ── Approval ─────────────────────────────────────────────────────────
      resolveApproval: (result) => {
        const engine = get()._engine
        if (engine) {
          engine.resolveApproval(result)
          set({ status: 'streaming', currentToolUI: null })
        }
      },

      clearCurrentToolUI: () => set({ currentToolUI: null }),

      // ── Clear ────────────────────────────────────────────────────────────
      clearMessages: () => set({
        messages: [],
        streamingText: '',
        thinkingText: '',
        totalUsage: { ...EMPTY_USAGE },
        totalCostUsd: 0,
        activeTools: [],
        currentToolUI: null,
        appState: createInitialAppState(''),
        contextWarning: { level: 'none', estimatedTokens: 0 },
        compactionCount: 0,
        contextSnapshot: null,
        currentSessionId: null,
        currentRunId: null,
        conversationThreadId: null,
      }),

      clearError: () => set({ error: null, status: 'idle' }),

      // ── New Actions (CC features) ────────────────────────────────────────
      forceCompact: async () => {
        const { _engine: engine, messages } = get()
        if (!engine || messages.length === 0) return
        const compacted = await engine.forceCompact(messages)
        set({
          messages: compacted.messages,
          compactionCount: get().compactionCount + 1,
          contextSnapshot: engine.getContextSnapshot(compacted.messages),
        })
      },

      getContextSnapshot: () => {
        const { _engine: engine, messages } = get()
        if (!engine) return null
        return engine.getContextSnapshot(messages)
      },

      loadSessionById: async (sessionId: string) => {
        const session = await loadSession(sessionId)
        if (session) {
          set({
            currentSessionId: session.id,
            currentRunId: null,
            conversationThreadId: session.threadId ?? session.id,
            streamingText: '',
            thinkingText: '',
            activeTools: [],
            status: 'idle',
          })
        }
        return session
      },

      getSessions: async () => {
        return listSessions()
      },

      deleteSessionById: async (sessionId: string) => {
        await deleteSession(sessionId)
        if (get().currentSessionId === sessionId) {
          set({ currentSessionId: null })
        }
      },

      fetchOllamaModels: async () => {
        const ollamaConfig = useConfigStore.getState().ollama
        const models = await listOllamaModels(ollamaConfig.baseUrl)
        const mapped = models.map((model) => ({
          id: model.name,
          name: model.name,
          size: model.size,
        }))
        useConfigStore.getState().setAvailableModels(mapped.map((model) => model.id))
        return mapped
      },

      checkOllamaStatus: async () => {
        const ollamaConfig = useConfigStore.getState().ollama
        return checkOllamaConnection(ollamaConfig.baseUrl)
      },
    }),
    {
      name: 'engine-store',
      // Only persist config and provider, not runtime state
      partialize: (state) => ({
        activeProvider: state.activeProvider,
        config: state.config,
        currentSessionId: state.currentSessionId,
        currentRunId: state.currentRunId,
      }),
      merge: (persistedState, currentState) => {
        const typedState = persistedState as Partial<EngineStoreState> | undefined
        return {
          ...currentState,
          ...typedState,
          activeProvider: normalizeChatProvider(typedState?.activeProvider),
          config: {
            ...currentState.config,
            ...(typedState?.config ?? {}),
            systemPrompt: typedState?.config?.systemPrompt || currentState.config.systemPrompt,
          },
        }
      },
    },
  ),
)

// ── Selectors ──────────────────────────────────────────────────────────────

export const selectIsStreaming = (s: EngineStoreState) => s.status === 'streaming'
export const selectIsToolRunning = (s: EngineStoreState) => s.status === 'tool_running'
export const selectNeedsApproval = (s: EngineStoreState) => s.status === 'waiting_approval'
export const selectIsEngineReady = () => true
export const selectAvailableModels = (): string[] => []
export const selectContextWarning = (s: EngineStoreState) => s.contextWarning
export const selectCompactionCount = (s: EngineStoreState) => s.compactionCount
export const selectHasSession = (s: EngineStoreState) => s.currentSessionId !== null
export const selectIsOllamaProvider = (s: EngineStoreState) => s.activeProvider === 'ollama'
