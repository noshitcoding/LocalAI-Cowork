// ── Engine Store (Zustand) ──────────────────────────────────────────────────
// Main Zustand binding for the integrated Ollama-first engine.
// Wraps QueryEngine in a reactive Zustand store for UI binding.

import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { invoke } from '@tauri-apps/api/core'
import {
  QueryEngine,
  type EngineConfig,
  type EngineEvent,
  type Message,
  type ContentBlock,
  type AppState,
  type TokenUsage,
  type ApprovalResult,
  type ToolUIRequest,
  type ContextSnapshot,
  type SessionRecord,
  type SessionSummary,
  createAssistantMessage,
  createInitialAppState,
  createSystemMessage,
  createUserMessage,
  EMPTY_USAGE,
  registerBuiltinCommands,
  getAllCommands,
  DEFAULT_SYSTEM_PROMPT,
  DEFAULT_AGENTS,
  listOllamaModels,
  checkOllamaConnection,
  autoSaveSession,
  generateSessionTitle,
  loadSession,
  listSessions,
  deleteSession,
  buildSystemPromptWithMemory,
  extractTextContent,
} from '../engine'
import { useConfigStore } from './configStore'
import { parsePersistedSessionMessage } from '../utils/sessionThreads'

// ── Types ──────────────────────────────────────────────────────────────────

export type EngineStatus = 'idle' | 'streaming' | 'tool_running' | 'waiting_approval' | 'error'
export type EngineProvider = 'ollama' | 'engine'

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

type ChatHistorySeedMessage = {
  role: 'user' | 'assistant' | 'system'
  content: string
  debugContent?: string
}

type ConversationHistorySeed = {
  threadId: string | null
  messages: ChatHistorySeedMessage[]
}

type EngineUserInput = string | ContentBlock[]

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
  ) => Promise<void>
  abort: () => void
  resolveApproval: (result: ApprovalResult) => void
  clearCurrentToolUI: () => void
  clearMessages: () => void
  clearError: () => void

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
  _initEngine: (cwd: string) => QueryEngine
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
    const structuredMessage = parsePersistedSessionMessage(
      (message.debugContent?.trim() || message.content.trim()),
    )

    if (structuredMessage) {
      acc.push(structuredMessage)
      return acc
    }

    const preferredContent = message.role === 'user'
      ? (message.debugContent?.trim() || message.content.trim())
      : message.content.trim()

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
        maxTurns: 25,
        maxBudgetUsd: 0,
        permissionMode: 'default' as const,
        thinkingEnabled: true,
        thinkingBudget: 10000,
        autoCompact: true,
        appendSystemPrompt: '',
        ollamaBaseUrl: 'http://192.168.178.82:11434',
        ollamaModel: 'gpt-oss:20b',
        sessionPersistence: true,
      },

      _engine: null,

      // ── Config ───────────────────────────────────────────────────────────
      setActiveProvider: () => set({ activeProvider: 'ollama' }),
      setConfig: (patch) => set((s) => ({ config: { ...s.config, ...patch } })),
      setApiKey: (apiKey) => set((s) => ({ config: { ...s.config, apiKey } })),

      // ── Init Engine ──────────────────────────────────────────────────────
      _initEngine: (cwd: string): QueryEngine => {
        ensureCommandsRegistered()

        const { config } = get()
        const configState = useConfigStore.getState()
        const ollamaConfig = configState.ollama
        const effectiveThinkingEnabled = true
        const effectiveTimeoutMs = Math.max(ollamaConfig.timeoutMs, MIN_THINKING_TIMEOUT_MS)

        const engineConfig: EngineConfig = {
          backend: 'ollama',
          anthropic: {
            apiKey: config.apiKey,
            model: config.model,
            thinking: effectiveThinkingEnabled
              ? { type: 'enabled', budgetTokens: config.thinkingBudget }
              : { type: 'disabled' },
          },
          ollama: {
            baseUrl: ollamaConfig.baseUrl,
            model: ollamaConfig.model,
            temperature: ollamaConfig.temperature,
            contextWindow: ollamaConfig.contextWindow,
            timeoutMs: effectiveTimeoutMs,
            thinkingEnabled: effectiveThinkingEnabled,
          },
          cwd,
          systemPrompt: DEFAULT_SYSTEM_PROMPT,
          maxTurns: config.maxTurns,
          maxBudgetUsd: config.maxBudgetUsd,
          permissionMode: config.permissionMode,
          commands: getAllCommands(),
          agentDefinitions: DEFAULT_AGENTS,
          appendSystemPrompt: config.appendSystemPrompt,
          runId: get().currentRunId ?? undefined,
          sessionId: get().currentSessionId ?? undefined,
        }

        const engine = new QueryEngine(engineConfig)

        // Wire tool UI callback
        engine.setToolUICallback((ui) => {
          set({ currentToolUI: ui })
        })

        set({ _engine: engine })
        return engine
      },

      // ── Send Message ─────────────────────────────────────────────────────
      sendMessage: async (userInput, cwd, onEvent, historySeed) => {
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
            const configState = useConfigStore.getState()
            const ollamaConfig = configState.ollama
            const latestStore = get()
            const effectiveThinkingEnabled = true
            const effectiveTimeoutMs = Math.max(ollamaConfig.timeoutMs, MIN_THINKING_TIMEOUT_MS)
            const userInputText = extractUserInputText(userInput)
            let engine = state._engine
            if (!engine) {
              engine = state._initEngine(cwd)
            } else {
              // Update config on existing engine with latest Ollama settings
              engine.updateConfig({
                backend: 'ollama',
                cwd,
                anthropic: {
                  apiKey: latestStore.config.apiKey,
                  model: latestStore.config.model,
                  thinking: effectiveThinkingEnabled
                    ? { type: 'enabled', budgetTokens: latestStore.config.thinkingBudget }
                    : { type: 'disabled' },
                },
                ollama: {
                  baseUrl: ollamaConfig.baseUrl,
                  model: ollamaConfig.model,
                  temperature: ollamaConfig.temperature,
                  contextWindow: ollamaConfig.contextWindow,
                  timeoutMs: effectiveTimeoutMs,
                  thinkingEnabled: effectiveThinkingEnabled,
                },
                runId,
                sessionId: latestStore.currentSessionId ?? undefined,
              })
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
              const hydratedMessages = mapChatHistorySeedToEngineMessages(historySeed!.messages, ollamaConfig.model)
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
                DEFAULT_SYSTEM_PROMPT,
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
                model: ollamaConfig.model,
                provider: 'ollama',
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
                    }
                    break

                  case 'error':
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
                      void autoSaveSession(
                        sessionId,
                        generateSessionTitle(doneMessages),
                        cwd,
                        doneMessages,
                        event.totalUsage,
                        event.totalCostUsd,
                        engine!.getAppState(),
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
            messages: session.messages,
            totalUsage: session.totalUsage,
            totalCostUsd: session.totalCostUsd,
            appState: { ...createInitialAppState(session.cwd), ...session.appState },
            currentSessionId: session.id,
            currentRunId: null,
            conversationThreadId: session.id,
            streamingText: '',
            thinkingText: '',
            activeTools: [],
            status: 'idle',
            contextSnapshot: get()._engine?.getContextSnapshot(session.messages) ?? null,
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
