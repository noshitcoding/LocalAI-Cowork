// ── Engine Store (Zustand) ──────────────────────────────────────────────────
// Main Zustand binding for the integrated Ollama-first engine.
// Wraps QueryEngine in a reactive Zustand store for UI binding.

import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { invoke } from '@tauri-apps/api/core'
import { QueryEngine, type EngineBackend, type EngineConfig, type EngineEvent } from '../engine/core/queryEngine'
import { getAllCommands, registerBuiltinCommands } from '../engine/commands/registry'
import { listOllamaModels, checkOllamaConnection } from '../engine/api/ollamaClient'
import { DEFAULT_AGENTS } from '../engine/coordinator/agentCoordinator'
import {
  buildSystemPromptWithMemory,
  captureAutomaticMemoryDraft,
  loadFrozenMemorySnapshot,
} from '../engine/memory/memorySystem'
import type { ContextSnapshot } from '../engine/services/contextManager'
import {
  createAssistantMessage,
  createInitialAppState,
  createUserMessage,
  EMPTY_USAGE,
  extractTextContent,
  type ApprovalResult,
  type AppState,
  type ContentBlock,
  type Message,
  type TokenUsage,
  type ToolUIRequest,
} from '../engine/types'
import { useConfigStore } from './configStore'
import { useChatStore } from './chatStore'
import { parsePersistedChatMessage } from '../utils/chatMessages'
import { splitPromptDebugContent } from '../utils/messageDisplay'
import { getChatProviderState, normalizeChatProvider, type ChatProviderKind, type ChatProviderSelection } from '../utils/chatProvider'
import type { PermissionMode } from '../engine/types/tool'
import type { AuthorizedTaskPath } from '../utils/taskProjectContext'
import { useCoworkStore } from './coworkStore'
import { setCredential } from '../security/credentialVault'
import { sanitizeEngineConfigForPersistence } from '../security/credentialPersistence'
import { hasTauriRuntime } from '../utils/safeInvoke'
import { CodexAppServerEngine } from '../engine/codex/codexAppServerEngine'

export type RunSecurityMode = 'checking' | 'windows_native_elevated' | 'host_read_only_broker'
type ChatEngine = QueryEngine | CodexAppServerEngine

type RunContextPrepareResult = {
  mode: Exclude<RunSecurityMode, 'checking'>
  sandboxId?: string | null
  workspaceRoot: string
  allowedDirectories: string[]
  roots: Array<{
    rootId: string
    rootLabel: string
    sourcePath: string
    workspacePath: string
    kind: 'file' | 'folder'
    access: 'read_only' | 'read_write'
    isPrimary: boolean
  }>
  copiedFiles: number
  skippedFiles: number
  warning?: string | null
}

type SandboxFileChange = {
  rootId?: string
  rootLabel?: string
  path: string
  kind: 'created' | 'modified' | 'deleted'
  size: number
  binary: boolean
  preview?: string | null
  applicable?: boolean
  policyError?: string | null
}

type SandboxRunDiff = {
  workspaceRoot: string
  changes: SandboxFileChange[]
}

async function offerSandboxChanges(runId: string): Promise<void> {
  const diff = await invoke<SandboxRunDiff>('sandbox_run_diff', { request: { runId } })
  if (diff.changes.length === 0 || typeof window === 'undefined') return
  const rendered = diff.changes.map((change) => {
    const detail = change.binary
      ? `[binary, ${change.size} bytes]`
      : change.preview
        ? `\n${change.preview}`
        : ''
    const root = change.rootLabel ? `[${change.rootLabel}] ` : ''
    const policy = change.policyError ? `\n[not applicable: ${change.policyError}]` : ''
    return `${change.kind.toUpperCase()} ${root}${change.path}${detail}${policy}`
  }).join('\n\n')
  const confirmed = window.confirm(
    `AI Sandbox changes (${diff.changes.length})\n\n${rendered}\n\nApply this complete change set to the original project?`,
  )
  if (!confirmed) return
  const applied = await invoke<{ applied: string[]; conflicts: string[]; rejected?: string[] }>('sandbox_run_apply', {
    request: { runId },
  })
  if (applied.conflicts.length > 0) {
    window.alert(`Some files changed outside the sandbox and were not overwritten:\n${applied.conflicts.join('\n')}`)
  }
  if ((applied.rejected?.length ?? 0) > 0) {
    window.alert(`Read-only or out-of-scope sandbox changes were not applied:\n${applied.rejected!.join('\n')}`)
  }
}

const CAPABILITY_TOOL_NAMES: Record<string, string[]> = {
  bash: ['Bash'],
  read_file: ['Read', 'ListDir', 'FileInfo'],
  edit_file: ['Write', 'Edit', 'MultiEdit', 'Append', 'DeleteFile', 'RenameFile', 'SaveSkill'],
  create_directory: ['CreateDirectory'],
  move_path: ['MovePath'],
  copy_path: ['CopyPath'],
  glob: ['Glob'],
  grep: ['Grep'],
  web_fetch: ['WebFetch'],
  web_search: ['WebSearch'],
  office_workflow: ['OfficeWorkflowTool'],
  todo: ['TaskCreate', 'TaskList', 'TaskUpdate', 'EnterPlanMode', 'ExitPlanMode'],
  delegate_task: ['Agent', 'Skill'],
  ask_user: ['AskUser'],
  mcp: ['MCPTool'],
}

const SAFE_INTERNAL_TOOL_NAMES = ['MemoryRead', 'MemoryWrite', 'ChatSearch', 'Think']
const LOCAL_MUTATION_CAPABILITIES = new Set([
  'bash', 'edit_file', 'create_directory', 'move_path', 'copy_path',
  'office_workflow', 'delegate_task',
])

function resolveEffectiveToolNames(
  mode: Exclude<RunSecurityMode, 'checking'>,
  hasReadableRoots: boolean,
): string[] {
  const policy = useCoworkStore.getState()
  const enabled = new Set(policy.enabledClaudeToolIds)
  const result = new Set(SAFE_INTERNAL_TOOL_NAMES)
  for (const capability of enabled) {
    if (mode === 'host_read_only_broker' && LOCAL_MUTATION_CAPABILITIES.has(capability)) continue
    if (mode === 'host_read_only_broker' && ['read_file', 'glob', 'grep'].includes(capability) && !hasReadableRoots) continue
    if (capability === 'bash' && !policy.policyFlags.allowShellExecution) continue
    if (['read_file', 'glob', 'grep'].includes(capability) && !policy.policyFlags.allowFileReadExtraction) continue
    if (capability === 'web_fetch' && !policy.policyFlags.allowWebFetch) continue
    if (capability === 'web_search' && !policy.policyFlags.allowWebSearch) continue
    if (capability === 'mcp' && !policy.policyFlags.allowMcpToolCalls) continue
    for (const toolName of CAPABILITY_TOOL_NAMES[capability] ?? []) result.add(toolName)
  }
  return [...result]
}

const DEFAULT_SYSTEM_PROMPT = `You are a helpful AI assistant in the LocalAI Cowork desktop app. You have access to tools for reading, writing, and searching files, running shell commands, and more.

Important rules:
1. Execute changes directly instead of only making suggestions, unless plan mode is active.
2. Before tool calls, briefly explain what you are doing.
3. Never delete or overwrite important data without explicit confirmation.
4. Give clear, precise answers.
5. Ask follow-up questions only when required.
   If the target is clear, complete it autonomously and ask only when critical information is missing or a destructive step is involved.
6. Do not create files that are not needed.
7. Proactively preserve only durable, high-signal facts. Use MemoryWrite for stable user preferences, environment facts, corrections, conventions, and completed-work lessons. Never store secrets, raw logs, or temporary details.
8. Curated memory is frozen when a chat starts. A write is persisted for future chats; use ChatSearch when exact details from older conversations are needed.
9. Reusable app skills belong in the central LocalAI Cowork skill store. When the user asks you to create, save, or register a skill, use SaveSkill. Do not create a skill JSON, Markdown file, or script in the current workspace unless the user explicitly asks for a standalone file.
10. AI-owned shell commands run only through the Bash tool in the prepared native sandbox. Never use desktop automation or a terminal window as a fallback for a failed Bash call.

Use the path and shell conventions of the operating system hosting LocalAI Cowork.`

export type EngineProvider = ChatProviderKind
export type EngineStatus = 'idle' | 'streaming' | 'tool_running' | 'waiting_approval' | 'error'

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
  appendSystemPrompt: string
  // Ollama-specific (persisted alongside configStore)
  ollamaBaseUrl: string
  ollamaModel: string
}

export type ContextWarning = {
  level: 'none' | 'low' | 'medium' | 'high' | 'critical'
  estimatedTokens: number
}

export type ContextCoverage = {
  totalPrevious: number
  sentPrevious: number
  omittedPrevious: number
  maxInputTokens: number
}

export type ChatHistorySeedMessage = {
  role: 'user' | 'assistant' | 'system'
  content: string | ContentBlock[]
  debugContent?: string
}

export type ConversationHistorySeed = {
  threadId: string | null
  messages: ChatHistorySeedMessage[]
  ownerKind?: 'chat' | 'task' | 'schedule' | 'crew'
  ownerId?: string
  memberId?: string
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
    return imageCount === 1 ? '[1 Image-attachment]' : `[${imageCount} Image-attachments]`
  }

  if (text && imageCount > 0) {
    const suffix = imageCount === 1 ? '[1 Image-attachment]' : `[${imageCount} Image-attachments]`
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

function stringifyRunPayload(value: unknown, maxLength = 4000): string {
  try {
    const text = typeof value === 'string' ? value : JSON.stringify(value)
    return text.length > maxLength ? `${text.slice(0, maxLength)}...` : text
  } catch {
    return String(value).slice(0, maxLength)
  }
}

async function appendRunEvent(
  runId: string,
  eventType: string,
  summary: string,
  payload: unknown,
  redactionLevel = 'metadata',
): Promise<void> {
  await invoke('engine_run_event_append', {
    request: {
      runId,
      eventType,
      summary,
      payloadJson: stringifyRunPayload(payload),
      redactionLevel,
    },
  })
}

type RuntimePermissionConfig = {
  mode: PermissionMode
  allowedDirectories: string[]
  authorizedPaths?: AuthorizedTaskPath[]
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

  // ── Context State ──────────────────────────────────────────────────────
  contextWarning: ContextWarning
  contextSnapshot: ContextSnapshot | null
  contextCoverage: ContextCoverage | null

  // ── Run State ──────────────────────────────────────────────────────────
  currentRunId: string | null
  conversationThreadId: string | null
  sandboxContext: { mode: RunSecurityMode; warning: string | null }

  // ── Configuration ──────────────────────────────────────────────────────
  config: EngineStoreConfig
  setConfig: (patch: Partial<Omit<EngineStoreConfig, 'apiKey'>>) => void
  setApiKey: (apiKey: string) => Promise<void>

  // ── Engine Actions ─────────────────────────────────────────────────────
  sendMessage: (
    userInput: EngineUserInput,
    cwd: string,
    onEvent?: (event: EngineEvent) => void,
    historySeed?: ConversationHistorySeed,
    providerSelection?: ChatProviderSelection,
    permissionConfig?: RuntimePermissionConfig,
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
      permissionConfig?: RuntimePermissionConfig
      crewId: string | null
      threadId: string
      runId: string
      securityMode: Exclude<RunSecurityMode, 'checking'>
    },
  ) => Promise<void>) | null
  setCrewTaskMessageHandler: (handler: ((
    params: {
      userInput: EngineUserInput
      cwd: string
      onEvent?: (event: EngineEvent) => void
      historySeed?: ConversationHistorySeed
      providerSelection?: ChatProviderSelection
      permissionConfig?: RuntimePermissionConfig
      crewId: string | null
      threadId: string
      runId: string
      securityMode: Exclude<RunSecurityMode, 'checking'>
    },
  ) => Promise<void>) | null) => void
  // ── New Actions (CC features) ──────────────────────────────────────────
  getContextSnapshot: () => ContextSnapshot | null
  fetchOllamaModels: () => Promise<Array<{ id: string; name: string; size: number }>>
  checkOllamaStatus: () => Promise<boolean>

  // ── Internal ───────────────────────────────────────────────────────────
  _engine: ChatEngine | null
  _initEngine: (
    cwd: string,
    providerSelection?: ChatProviderSelection,
    permissionConfig?: RuntimePermissionConfig,
    owner?: Pick<ConversationHistorySeed, 'ownerKind' | 'ownerId' | 'memberId'>,
  ) => Promise<ChatEngine>
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
    const debugContext = splitPromptDebugContent(message.debugContent).promptDebug
    const rawContent = debugContext || textContent
    const structuredMessage = parsePersistedChatMessage(rawContent)

    if (structuredMessage) {
      if (structuredMessage.type !== 'system') {
        acc.push(structuredMessage)
      }
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
      ? (debugContext || textContent)
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
        return acc
      default:
        return acc
    }
  }, [])
}

function getResolvedProvider(providerState: ReturnType<typeof getChatProviderState>): EngineBackend {
  if (providerState.provider === 'codex') return 'codex'
  return providerState.compatibilityProvider ?? 'openai-compatible'
}

function buildChatEngineConfig(
  provider: EngineBackend,
  config: EngineStoreConfig,
  cwd: string,
  runId?: string,
  threadId?: string,
  providerSelection?: ChatProviderSelection,
  permissionConfig?: RuntimePermissionConfig,
  owner?: Pick<ConversationHistorySeed, 'ownerKind' | 'ownerId' | 'memberId'>,
): EngineConfig {
  const configState = useConfigStore.getState()
  const providerState = getChatProviderState(configState, provider, providerSelection)
  const ollamaConfig = configState.ollama
  const toolsetPolicyId = useCoworkStore.getState().activeToolsetPolicyId
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
      baseUrl: ollamaConfig.baseUrl,
      model: ollamaConfig.model,
      temperature: ollamaConfig.temperature,
      contextWindow: providerState.contextWindow,
      timeoutMs: effectiveOllamaTimeoutMs,
      thinkingEnabled: effectiveThinkingEnabled,
    },
    openAiCompatible: provider === 'openai-compatible' || provider === 'openrouter'
      ? {
          provider,
          profileId: providerState.profileId,
          preset: providerState.preset,
          authMode: providerState.preset === 'ollama' ? 'none' : 'bearer',
          apiKey: providerState.apiKey,
          baseUrl: providerState.endpoint,
          model: providerState.model,
          timeoutMs: providerState.timeoutMs,
          verifyTlsCertificates: providerState.verifyTlsCertificates,
        }
      : undefined,
    codex: provider === 'codex'
      ? {
          authProfileId: providerState.authProfileId,
          model: providerState.model || undefined,
          reasoningEffort: providerState.reasoningEffort,
          ownerKind: owner?.ownerKind ?? 'chat',
          ownerId: owner?.ownerId ?? threadId,
          memberId: owner?.memberId,
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
    threadId,
    toolsetPolicyId,
    availableToolNames: resolveEffectiveToolNames(
      'host_read_only_broker',
      (permissionConfig?.authorizedPaths?.length ?? 0) > 0,
    ),
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
      activeProvider: 'openai-compatible',

      // Context
      contextWarning: { level: 'none', estimatedTokens: 0 },
      contextSnapshot: null,
      contextCoverage: null,

      // Run
      currentRunId: null,
      conversationThreadId: null,
      sandboxContext: { mode: 'checking', warning: null },

      config: {
        apiKey: '',
        model: 'claude-sonnet-4-20250514',
        systemPrompt: DEFAULT_SYSTEM_PROMPT,
        maxTurns: 25,
        maxBudgetUsd: 0,
        permissionMode: 'default' as const,
        thinkingEnabled: true,
        thinkingBudget: 10000,
        appendSystemPrompt: '',
        ollamaBaseUrl: 'http://localhost:11434',
        ollamaModel: 'llama3.1:8b',
      },

      _engine: null,
      // ── Crew Task Message Handler ──────────────────────────────────
      crewTaskMessageHandler: null,
      setCrewTaskMessageHandler: (handler) => set({ crewTaskMessageHandler: handler }),
      // ── Config ───────────────────────────────────────────────────────────
      setActiveProvider: (provider) => set({ activeProvider: normalizeChatProvider(provider) }),
      setConfig: (patch) => set((s) => ({ config: { ...s.config, ...patch } })),
      setApiKey: async (apiKey) => {
        await setCredential({ scope: 'engine', ownerId: 'legacy-engine', field: 'api_key' }, apiKey)
        set((state) => ({ config: { ...state.config, apiKey: '' } }))
      },

      // ── Init Engine ──────────────────────────────────────────────────────
      _initEngine: async (cwd, providerSelection, permissionConfig, owner): Promise<ChatEngine> => {
        ensureCommandsRegistered()

        const { config, activeProvider, currentRunId, conversationThreadId } = get()
        const providerState = getChatProviderState(useConfigStore.getState(), activeProvider, providerSelection)
        const engineConfig = buildChatEngineConfig(
          getResolvedProvider(providerState),
          config,
          cwd,
          currentRunId ?? undefined,
          conversationThreadId ?? undefined,
          providerSelection,
          permissionConfig,
          owner,
        )

        const engine: ChatEngine = engineConfig.backend === 'codex'
          ? new CodexAppServerEngine(engineConfig)
          : new QueryEngine(engineConfig)

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
                throw new Error('The engine is already processing another request.')
              }
            }

            // Resolve the addressed thread from the history seed. Background task
            // runs must never inherit the currently visible chat's runner/backend.
            const chatState = useChatStore.getState()
            const targetThread = chatState.threads.find((thread) => thread.id === historySeed?.threadId)
              ?? chatState.threads.find((thread) => thread.id === chatState.activeThreadId)
            const isCrewTask = targetThread?.runner === 'crew' && targetThread?.crewId

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
                sandboxContext: { mode: 'checking', warning: null },
              })

              try {
                const authorizedPaths = permissionConfig?.authorizedPaths ?? []
                let persistedRun = false
                try {
                  await invoke('engine_run_create', {
                    request: {
                      id: runId,
                      threadId: targetThread!.id,
                      title: extractUserInputText(userInput).slice(0, 120) || 'Crew Run',
                      inputSummary: extractUserInputText(userInput).slice(0, 1000),
                      source: 'crew_chat',
                      status: 'running',
                      phase: 'crew_runtime',
                      cwd,
                      authorizedPaths,
                      metadataJson: JSON.stringify({ runner: 'crew' }),
                    },
                  })
                  persistedRun = true
                } catch (error) {
                  if (hasTauriRuntime()) throw error
                }
                const prepared: RunContextPrepareResult = persistedRun && hasTauriRuntime()
                  ? await invoke<RunContextPrepareResult>('engine_run_prepare_context', {
                      request: { runId, authorizedPaths, preferredCwd: cwd || null },
                    })
                  : {
                      mode: 'host_read_only_broker',
                      sandboxId: null,
                      workspaceRoot: '',
                      allowedDirectories: authorizedPaths.map((entry) => entry.path),
                      roots: authorizedPaths.map((entry, index) => ({
                        rootId: `root-${index.toString().padStart(3, '0')}`,
                        rootLabel: entry.label ?? entry.path,
                        sourcePath: entry.path,
                        workspacePath: entry.path,
                        kind: entry.kind,
                        access: 'read_only',
                        isPrimary: entry.isPrimary === true,
                      })),
                      copiedFiles: 0,
                      skippedFiles: 0,
                      warning: 'Native sandbox is unavailable; crew local access is read-only.',
                    }
                const mappedPermissionConfig: RuntimePermissionConfig = {
                  mode: permissionConfig?.mode ?? get().config.permissionMode,
                  allowedDirectories: prepared.roots
                    .filter((root) => root.kind === 'folder')
                    .map((root) => root.workspacePath),
                  authorizedPaths: prepared.roots.map((root) => ({
                    id: root.rootId,
                    path: root.workspacePath,
                    kind: root.kind,
                    access: root.access,
                    label: root.rootLabel,
                    isPrimary: root.isPrimary,
                  })),
                }
                set({ sandboxContext: { mode: prepared.mode, warning: prepared.warning ?? null } })
                await get().crewTaskMessageHandler!({
                  userInput,
                  cwd: prepared.workspaceRoot || cwd,
                  onEvent,
                  historySeed,
                  providerSelection,
                  permissionConfig: mappedPermissionConfig,
                  crewId: targetThread!.crewId!,
                  threadId: targetThread!.id,
                  runId,
                  securityMode: prepared.mode,
                })
                if (prepared.mode === 'windows_native_elevated') {
                  try {
                    await offerSandboxChanges(runId)
                  } catch (error) {
                    void appendRunEvent(runId, 'sandbox_diff_error', 'Crew sandbox change review failed', {
                      error: error instanceof Error ? error.message : String(error),
                    }).catch(() => {})
                  }
                }
                if (persistedRun) {
                  void invoke('engine_run_update', {
                    request: { id: runId, status: 'completed', phase: 'completed' },
                  }).catch(() => {})
                }
              } finally {
                set({
                  status: 'idle',
                  streamingText: '',
                  thinkingText: '',
                  currentRunId: null,
                })
              }
              return
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
              sandboxContext: { mode: 'checking', warning: null },
            })

            if (historySeed?.threadId) {
              set({ conversationThreadId: historySeed.threadId })
            }

            // Get or create engine
            const latestStore = get()
            const userInputText = extractUserInputText(userInput)
            const providerState = getChatProviderState(useConfigStore.getState(), latestStore.activeProvider, providerSelection)
            const provider = getResolvedProvider(providerState)
            const toolsetPolicyId = useCoworkStore.getState().activeToolsetPolicyId
            let engine = state._engine
            if (
              !engine
              || (provider === 'codex' && !(engine instanceof CodexAppServerEngine))
              || (provider !== 'codex' && engine instanceof CodexAppServerEngine)
            ) {
              engine?.abort()
              engine = await state._initEngine(cwd, providerSelection, permissionConfig, historySeed)
            } else {
              engine.updateConfig(buildChatEngineConfig(
                provider,
                latestStore.config,
                cwd,
                runId,
                historySeed?.threadId ?? latestStore.conversationThreadId ?? undefined,
                providerSelection,
                permissionConfig,
                historySeed,
              ))
            }

            // The persisted chat is the source of truth. Rebuild provider-neutral
            // engine messages before every request, including after model changes.
            if (historySeed && Array.isArray(historySeed.messages)) {
              const hydratedMessages = mapChatHistorySeedToEngineMessages(historySeed.messages, providerState.model)
              set({
                messages: hydratedMessages,
                conversationThreadId: historySeed?.threadId ?? null,
                contextSnapshot: engine.getContextSnapshot(hydratedMessages),
                contextCoverage: null,
              })
            }

            const threadId = historySeed?.threadId ?? get().conversationThreadId ?? undefined
            const frozenSnapshot = await loadFrozenMemorySnapshot(threadId)

            let persistedRun = false
            try {
              await invoke('engine_run_create', {
                request: {
                  id: runId,
                  threadId: threadId ?? null,
                  title: userInputText.slice(0, 120) || 'Engine Run',
                  inputSummary: userInputText.slice(0, 1000),
                  status: 'running',
                  phase: 'llm_turn',
                  cwd,
                  model: providerState.model,
                  provider,
                  toolsetPolicyId,
                  authorizedPaths: permissionConfig?.authorizedPaths ?? [],
                  metadataJson: JSON.stringify({
                    permissionMode: latestStore.config.permissionMode,
                    maxTurns: latestStore.config.maxTurns,
                  }),
                },
              })
              persistedRun = true
            } catch (error) {
              if (hasTauriRuntime() || permissionConfig?.authorizedPaths) {
                throw error
              }
              // Browser/dev fallback has no persisted engine run.
            }

            const authorizedPaths = permissionConfig?.authorizedPaths ?? []
            const prepared: RunContextPrepareResult = persistedRun && hasTauriRuntime()
              ? await invoke<RunContextPrepareResult>('engine_run_prepare_context', {
                  request: {
                    runId,
                    authorizedPaths,
                    preferredCwd: cwd || null,
                  },
                })
              : {
                  mode: 'host_read_only_broker',
                  sandboxId: null,
                  workspaceRoot: '',
                  allowedDirectories: authorizedPaths.map((entry) => entry.path),
                  roots: authorizedPaths.map((entry, index) => ({
                    rootId: `root-${index.toString().padStart(3, '0')}`,
                    rootLabel: entry.label ?? entry.path,
                    sourcePath: entry.path,
                    workspacePath: entry.path,
                    kind: entry.kind,
                    access: 'read_only',
                    isPrimary: entry.isPrimary === true,
                  })),
                  copiedFiles: 0,
                  skippedFiles: 0,
                  warning: 'Native sandbox is unavailable in this runtime; local access is read-only.',
                }
            const sandboxPrepared = prepared.mode === 'windows_native_elevated'
            const effectiveToolNames = resolveEffectiveToolNames(
              prepared.mode,
              prepared.allowedDirectories.length > 0,
            )
            const securityPrompt = sandboxPrepared
              ? 'This run is inside the native Windows sandbox. Work only in mapped sandbox roots. Changes are reviewed before they are applied to original shared paths.'
              : 'This run has no native process sandbox. Local shared paths are read-only. Shell, local mutation, Office artifact, skill-writing, delegation, and desktop-control tools are unavailable. Never use a host terminal fallback.'
            engine.updateConfig({
              cwd: prepared.workspaceRoot || cwd,
              runId,
              sandboxId: prepared.sandboxId ?? undefined,
              allowedDirectories: prepared.allowedDirectories,
              availableToolNames: effectiveToolNames,
              appendSystemPrompt: [latestStore.config.appendSystemPrompt, securityPrompt]
                .filter(Boolean)
                .join('\n\n'),
            })
            set({
              sandboxContext: {
                mode: prepared.mode,
                warning: prepared.warning ?? null,
              },
            })
            void appendRunEvent(runId, 'run_security_context', `Run security mode: ${prepared.mode}`, {
              mode: prepared.mode,
              sandboxId: prepared.sandboxId ?? null,
              rootCount: prepared.roots.length,
              effectiveTools: effectiveToolNames,
              warning: prepared.warning ?? null,
            }).catch(() => {})

            // Unscoped filesystem helpers are used only against the isolated copy.
            if (sandboxPrepared) {
              try {
                const { systemPrompt, memoryContent } = await buildSystemPromptWithMemory(
                  prepared.workspaceRoot,
                  latestStore.config.systemPrompt || DEFAULT_SYSTEM_PROMPT,
                  { userInput: userInputText, frozenSnapshot },
                )
                engine.updateConfig({ systemPrompt, memoryContent, threadId })
              } catch {
                // Workspace memory is optional.
              }
              try {
                await captureAutomaticMemoryDraft(prepared.workspaceRoot, userInputText, runId)
              } catch {
                // Automatic draft capture must never block the user turn.
              }
            }

            void invoke('memory_upsert', {
              id: crypto.randomUUID(),
              scope: 'chat',
              scopeRef: threadId ?? null,
              category: 'run_input',
              key: runId,
              content: userInputText,
              sourceRunId: runId,
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
                    void appendRunEvent(
                      runId,
                      'tool_start',
                      `Tool started: ${event.toolName}`,
                      {
                        toolUseId: event.toolUseId,
                        toolName: event.toolName,
                        input: event.input,
                      },
                    ).catch(() => {})
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
                    void appendRunEvent(
                      runId,
                      'tool_result',
                      `Tool completed: ${event.toolName}`,
                      {
                        toolUseId: event.toolUseId,
                        toolName: event.toolName,
                        result: event.result,
                        isError: event.isError,
                      },
                    ).catch(() => {})
                    set((s) => ({
                      activeTools: s.activeTools.map(t =>
                        t.id === event.toolUseId
                          ? { ...t, status: event.isError ? 'failed' as const : 'completed' as const, result: event.result }
                          : t,
                      ),
                    }))
                    break

                  case 'approval_required':
                    void appendRunEvent(
                      runId,
                      'approval_requested',
                      'Approval requested',
                      { request: event.request },
                    ).catch(() => {})
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
                    void appendRunEvent(
                      runId,
                      'error',
                      event.error.slice(0, 240),
                      { error: event.error },
                    ).catch(() => {})
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
                          inputTokens: event.totalUsage.input_tokens,
                          outputTokens: event.totalUsage.output_tokens,
                          costUsd: event.totalCostUsd,
                        },
                      }).catch(() => {})
                      void invoke('memory_upsert', {
                        id: crypto.randomUUID(),
                        scope: 'chat',
                        scopeRef: threadId ?? null,
                        category: 'run_output',
                        key: runId,
                        content: summary || 'Run completed.',
                        sourceRunId: runId,
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

                    // Update context snapshot
                    try {
                      const snap = engine!.getContextSnapshot(event.messages)
                      set({ contextSnapshot: snap })
                    } catch { /* optional */ }
                    if (sandboxPrepared) {
                      try {
                        await offerSandboxChanges(runId)
                      } catch (error) {
                        void appendRunEvent(runId, 'sandbox_diff_error', 'Sandbox change review failed', {
                          error: error instanceof Error ? error.message : String(error),
                        }).catch(() => {})
                      }
                    }
                    break

                  case 'context_coverage':
                    set({ contextCoverage: event })
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
        const chatState = useChatStore.getState()
        const activeThread = chatState.threads.find((thread) => thread.id === chatState.activeThreadId)
        if (engine) engine.abort()
        if (activeThread?.runner === 'crew' && activeThread.crewId) {
          void invoke('crew_stop', { request: { crewId: activeThread.crewId } }).catch(() => {})
        }
        if (currentRunId) {
          void invoke('engine_run_cancel', { id: currentRunId }).catch(() => {})
        }
        set({ status: 'idle', streamingText: '', thinkingText: '', currentRunId: null })
      },

      // ── Approval ─────────────────────────────────────────────────────────
      resolveApproval: (result) => {
        const { _engine: engine, currentRunId } = get()
        if (engine) {
          if (currentRunId) {
            void appendRunEvent(
              currentRunId,
              'approval_decided',
              result.allowed ? 'Approval allowed' : 'Approval denied',
              result,
            ).catch(() => {})
          }
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
        contextSnapshot: null,
        contextCoverage: null,
        currentRunId: null,
        conversationThreadId: null,
      }),

      clearError: () => set({ error: null, status: 'idle' }),

      // ── New Actions (CC features) ────────────────────────────────────────
      getContextSnapshot: () => {
        const { _engine: engine, messages } = get()
        if (!engine) return null
        return engine.getContextSnapshot(messages)
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
        config: sanitizeEngineConfigForPersistence(state.config),
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
export const selectIsOllamaProvider = () => false
