import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  createAssistantMessage,
  createInitialAppState,
  EMPTY_USAGE,
  extractTextContent,
  findToolByName,
  getEmptyToolPermissionContext,
  type AppState,
  type ApprovalResult,
  type ContentBlock,
  type Message,
  type TokenUsage,
  type Tool,
  type Tools,
  type ToolUseContext,
  type ToolUIRequest,
} from '../types'
import type { ContextSnapshot } from '../services/contextManager'
import type { EngineConfig, EngineEvent } from '../core/queryEngine'
import { getAllTools, registerAllBuiltinTools } from '../tools/registry'
import i18n from '../../i18n'

export type CodexEngineConfig = {
  authProfileId?: string
  model?: string
  reasoningEffort?: string
  ownerKind: 'chat' | 'task' | 'schedule' | 'crew'
  ownerId?: string
  memberId?: string
}

type RuntimeEnvelope = {
  profileId: string
  payload: {
    id?: number
    method?: string
    params?: Record<string, unknown>
  }
}

type ThreadOpenResult = {
  authProfileId: string
  rebuilt: boolean
  result: { thread?: { id?: string } }
}

type TurnStartResult = {
  turn?: { id?: string; status?: string }
}

type PendingApproval = {
  kind: 'app-server'
  profileId: string
  requestId: number
} | {
  kind: 'dynamic-tool'
  resolve: (result: ApprovalResult) => void
}

type CodexQueueEvent = EngineEvent
  | { type: 'codex_profile_limited'; error: string }
  | {
      type: 'codex_dynamic_tool'
      profileId: string
      requestId: number
      callId: string
      toolName: string
      input: Record<string, unknown>
    }

class AsyncEventQueue {
  private values: CodexQueueEvent[] = []
  private waiters: Array<(value: CodexQueueEvent) => void> = []

  push(value: CodexQueueEvent): void {
    const waiter = this.waiters.shift()
    if (waiter) waiter(value)
    else this.values.push(value)
  }

  next(): Promise<CodexQueueEvent> {
    const value = this.values.shift()
    if (value) return Promise.resolve(value)
    return new Promise((resolve) => this.waiters.push(resolve))
  }
}

function inputText(input: string | ContentBlock[]): string {
  if (typeof input === 'string') return input
  return input
    .filter((block): block is Extract<ContentBlock, { type: 'text' }> => block.type === 'text')
    .map((block) => block.text)
    .join('\n\n')
}

function historyText(messages: Message[]): string {
  return messages
    .filter((message) => message.type === 'user' || message.type === 'assistant')
    .map((message) => `${message.type === 'user' ? 'User' : 'Assistant'}:\n${extractTextContent(message)}`)
    .filter((entry) => entry.trim())
    .join('\n\n')
}

function stringValue(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

function objectValue(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' ? value as Record<string, unknown> : {}
}

function dynamicToolResultText(value: unknown): string {
  const text = typeof value === 'string' ? value : JSON.stringify(value ?? null)
  return text.length > 200_000 ? `${text.slice(0, 200_000)}\n[truncated]` : text
}

function mapUsage(params: Record<string, unknown>): TokenUsage {
  const usage = objectValue(params.tokenUsage ?? params.usage)
  const total = objectValue(usage.total ?? usage)
  return {
    input_tokens: Number(total.inputTokens ?? total.input_tokens ?? 0),
    output_tokens: Number(total.outputTokens ?? total.output_tokens ?? 0),
    cache_creation_input_tokens: 0,
    cache_read_input_tokens: Number(total.cachedInputTokens ?? total.cached_input_tokens ?? 0),
  }
}

function toolDescription(method: string, params: Record<string, unknown>): { name: string; description: string } {
  if (method.includes('fileChange')) {
    return { name: 'CodexFileChange', description: stringValue(params.reason) || i18n.t('Codex wants to apply file changes.') }
  }
  if (method.includes('permissions')) {
    return { name: 'CodexPermissions', description: stringValue(params.reason) || i18n.t('Codex requests additional permissions.') }
  }
  return { name: 'CodexCommand', description: stringValue(params.reason) || stringValue(params.command) || i18n.t('Codex wants to run a command.') }
}

export class CodexAppServerEngine {
  private config: EngineConfig
  private appState: AppState
  private tools: Tools
  private abortController = new AbortController()
  private toolUICallback: ((ui: ToolUIRequest | null) => void) | null = null
  private pendingApproval: PendingApproval | null = null
  private activeTurn: { profileId: string; threadId: string; turnId: string } | null = null

  constructor(config: EngineConfig) {
    this.config = config
    this.appState = createInitialAppState(config.cwd)
    registerAllBuiltinTools()
    this.tools = []
    this.refreshTools()
  }

  updateConfig(config: Partial<EngineConfig>): void {
    this.config = { ...this.config, ...config }
    if ('availableToolNames' in config || 'desktopControlEnabled' in config || 'customTools' in config) {
      this.refreshTools()
    }
    if (config.cwd) this.appState = { ...this.appState, cwd: config.cwd }
  }

  setToolUICallback(callback: (ui: ToolUIRequest | null) => void): void {
    this.toolUICallback = callback
  }

  getAppState(): AppState {
    return this.appState
  }

  getContextSnapshot(_messages?: Message[]): ContextSnapshot | null {
    return null
  }

  abort(): void {
    this.abortController.abort()
    this.abortController = new AbortController()
    const active = this.activeTurn
    if (!active) return
    void invoke('codex_turn_interrupt', {
      profileId: active.profileId,
      threadId: active.threadId,
      turnId: active.turnId,
    }).catch(() => {})
  }

  resolveApproval(result: ApprovalResult): void {
    const pending = this.pendingApproval
    if (!pending) return
    this.pendingApproval = null
    this.toolUICallback?.(null)
    if (pending.kind === 'dynamic-tool') {
      pending.resolve(result)
    } else {
      void invoke('codex_server_request_respond', {
        profileId: pending.profileId,
        requestId: pending.requestId,
        result: { decision: result.allowed ? 'accept' : 'decline' },
      }).catch(() => {})
    }
  }

  private refreshTools(): void {
    // The dynamic bridge fails closed: only the already-computed governance
    // capability ceiling is offered to the experimental App Server API.
    const allowed = new Set(this.config.availableToolNames ?? [])
    const desktopTools = new Set([
      'Desktopscreenshot', 'DesktopPrimaryDisplay', 'DesktopListWindows',
      'DesktopFocusWindow', 'DesktopLaunchApp', 'DesktopClick', 'DesktopMoveMouse',
      'DesktopTypeText', 'DesktopKeypress', 'DesktopScroll',
    ])
    const available = this.config.customTools
      ? [...getAllTools(), ...this.config.customTools]
      : [...getAllTools()]
    this.tools = available.filter((tool) => (
      allowed.has(tool.name)
      && tool.category !== 'agent'
      && (!tool.isEnabled || tool.isEnabled())
      && (this.config.desktopControlEnabled || !desktopTools.has(tool.name))
      && /^[a-zA-Z0-9_-]{1,128}$/.test(tool.name)
    ))
  }

  private dynamicToolDefinitions(): Array<Record<string, unknown>> {
    return this.tools.map((tool) => ({
      type: 'function',
      name: tool.name,
      description: tool.description.slice(0, 1024),
      inputSchema: tool.inputSchema,
    }))
  }

  private toolContext(messages: Message[]): ToolUseContext {
    const permissionContext = {
      ...getEmptyToolPermissionContext(),
      mode: this.config.permissionMode ?? 'default',
      allowedDirectories: this.config.allowedDirectories ?? [],
    }
    return {
      cwd: this.appState.cwd || this.config.cwd,
      abortController: this.abortController,
      debug: this.config.debug ?? false,
      model: this.config.codex?.model ?? 'codex',
      tools: this.tools,
      commands: this.config.commands ?? [],
      getAppState: () => this.appState,
      setAppState: (update) => { this.appState = update(this.appState) },
      setToolUI: this.toolUICallback ?? undefined,
      permissionContext,
      canUseTool: async () => ({ allowed: true }),
      mcpConnections: this.config.mcpConnections ?? [],
      agentDefinitions: this.config.agentDefinitions ?? [],
      memoryContent: this.config.memoryContent,
      runId: this.config.runId,
      threadId: this.config.threadId,
      sandboxId: this.config.sandboxId,
      messages,
    }
  }

  async *query(messages: Message[], input: string | ContentBlock[]): AsyncGenerator<EngineEvent> {
    const codex = this.config.codex
    if (!codex) throw new Error('Codex engine configuration is missing')
    if (!this.config.threadId && !codex.ownerId) throw new Error('Codex runs require an OpenCowork owner id')

    const queue = new AsyncEventQueue()
    let unlisten: UnlistenFn | null = null
    let finalText = ''
    let usage = { ...EMPTY_USAGE }
    let activeProfileId = codex.authProfileId ?? ''
    let activeThreadId = ''
    let activeTurnId = ''

    const handleRuntimeEvent = (event: RuntimeEnvelope) => {
      if (activeProfileId && event.profileId !== activeProfileId) return
      const payload = event.payload
      const method = payload.method ?? ''
      const params = objectValue(payload.params)
      const eventThreadId = stringValue(params.threadId)
      const eventTurnId = stringValue(params.turnId) || stringValue(objectValue(params.turn).id)
      if (activeThreadId && eventThreadId && eventThreadId !== activeThreadId) return
      if (activeTurnId && eventTurnId && eventTurnId !== activeTurnId) return

      if (typeof payload.id === 'number' && method.endsWith('/requestApproval')) {
        const tool = toolDescription(method, params)
        this.pendingApproval = { kind: 'app-server', profileId: event.profileId, requestId: payload.id }
        this.toolUICallback?.({
          type: 'approval',
          toolName: tool.name,
          content: tool.description,
          details: params,
        })
        queue.push({
          type: 'approval_required',
          request: {
            toolName: tool.name,
            input: params,
            description: tool.description,
            riskLevel: 'high',
            suggestedAction: 'ask',
          },
        })
        return
      }
      if (typeof payload.id === 'number' && method === 'item/tool/call') {
        queue.push({
          type: 'codex_dynamic_tool',
          profileId: event.profileId,
          requestId: payload.id,
          callId: stringValue(params.callId) || crypto.randomUUID(),
          toolName: stringValue(params.tool),
          input: objectValue(params.arguments),
        })
        return
      }

      switch (method) {
        case 'item/agentMessage/delta': {
          const delta = stringValue(params.delta)
          finalText += delta
          if (delta) queue.push({ type: 'text_delta', text: delta })
          break
        }
        case 'item/reasoning/summaryTextDelta':
        case 'item/reasoning/textDelta': {
          const delta = stringValue(params.delta)
          if (delta) queue.push({ type: 'thinking_delta', thinking: delta })
          break
        }
        case 'item/started': {
          const item = objectValue(params.item)
          const itemType = stringValue(item.type)
          if (['commandExecution', 'fileChange', 'mcpToolCall', 'dynamicToolCall', 'webSearch'].includes(itemType)) {
            queue.push({
              type: 'tool_use_start',
              toolUseId: stringValue(item.id) || crypto.randomUUID(),
              toolName: `Codex:${itemType}`,
              input: item,
            })
          }
          break
        }
        case 'item/completed': {
          const item = objectValue(params.item)
          const itemType = stringValue(item.type)
          if (itemType === 'agentMessage' && !finalText) finalText = stringValue(item.text)
          if (['commandExecution', 'fileChange', 'mcpToolCall', 'dynamicToolCall', 'webSearch'].includes(itemType)) {
            queue.push({
              type: 'tool_use_complete',
              toolUseId: stringValue(item.id) || crypto.randomUUID(),
              toolName: `Codex:${itemType}`,
              result: JSON.stringify(item),
              isError: item.status === 'failed',
            })
          }
          break
        }
        case 'thread/tokenUsage/updated':
          usage = mapUsage(params)
          queue.push({ type: 'usage_update', usage, costUsd: 0, totalCostUsd: 0 })
          break
        case 'error': {
          const error = objectValue(params.error)
          queue.push({ type: 'error', error: stringValue(error.message) || 'Codex turn failed' })
          break
        }
        case 'turn/completed': {
          const turn = objectValue(params.turn)
          const status = stringValue(turn.status)
          const error = objectValue(turn.error)
          const errorMessage = stringValue(error.message) || stringValue(error.code) || 'Codex turn failed'
          if (status === 'failed') {
            const limited = /rate.?limit|usage.?limit|quota|limit reached/i.test(errorMessage)
            if (limited && !codex.authProfileId) {
              queue.push({ type: 'codex_profile_limited', error: errorMessage })
              break
            }
            queue.push({ type: 'error', error: errorMessage })
          }
          queue.push({ type: 'turn_complete', turnCount: this.appState.turnCount + 1, stopReason: status || null })
          queue.push({
            type: 'assistant_message',
            message: createAssistantMessage(
              [{ type: 'text', text: finalText }],
              codex.model || 'codex',
              usage,
              status === 'completed' ? 'end_turn' : null,
            ),
          })
          queue.push({ type: 'done', messages: [], totalUsage: usage, totalCostUsd: 0 })
          break
        }
        case 'runtime/stopped':
        case 'runtime/protocolError':
          queue.push({ type: 'error', error: i18n.t('Codex App Server stopped unexpectedly.') })
          queue.push({ type: 'done', messages: [], totalUsage: usage, totalCostUsd: 0 })
          break
      }
    }

    try {
      unlisten = await listen<RuntimeEnvelope>('codex-runtime-event', ({ payload }) => handleRuntimeEvent(payload))
      const currentInput = inputText(input)
      let attempt = 0
      while (true) {
        attempt += 1
        finalText = ''
        usage = { ...EMPTY_USAGE }
        activeProfileId = codex.authProfileId ?? ''
        activeThreadId = ''
        activeTurnId = ''

        const opened = await invoke<ThreadOpenResult>('codex_thread_open', {
          request: {
            ownerKind: codex.ownerKind,
            ownerId: codex.ownerId || this.config.threadId,
            memberId: codex.memberId,
            authProfileId: codex.authProfileId,
            cwd: this.config.cwd,
            model: codex.model,
            permissionMode: this.config.permissionMode ?? 'default',
            dynamicTools: this.dynamicToolDefinitions(),
          },
        })
        activeProfileId = opened.authProfileId
        activeThreadId = opened.result.thread?.id ?? ''
        if (!activeThreadId) throw new Error('Codex thread could not be opened')

        const prompt = opened.rebuilt && messages.length > 0
          ? `The following is the sanitized OpenCowork conversation history. Continue from it without repeating prior answers.\n\n${historyText(messages)}\n\nCurrent user request:\n${currentInput}`
          : currentInput
        const started = await invoke<TurnStartResult>('codex_turn_start', {
          request: {
            authProfileId: activeProfileId,
            threadId: activeThreadId,
            prompt,
            cwd: this.config.cwd,
            model: codex.model,
            reasoningEffort: codex.reasoningEffort,
            permissionMode: this.config.permissionMode ?? 'default',
            writableRoots: this.config.allowedDirectories ?? [],
          },
        })
        activeTurnId = started.turn?.id ?? ''
        if (!activeTurnId) throw new Error('Codex turn/start returned no turn id')
        this.activeTurn = { profileId: activeProfileId, threadId: activeThreadId, turnId: activeTurnId }

        let retryAnotherProfile = false
        while (true) {
          const event = await queue.next()
          if (event.type === 'codex_profile_limited') {
            await invoke('codex_profile_mark_limited', {
              profileId: activeProfileId,
              reason: event.error,
            })
            yield { type: 'retry', reason: 'Codex account quota reached; trying the next account.', attempt }
            retryAnotherProfile = true
            break
          }
          if (event.type === 'codex_dynamic_tool') {
            const tool: Tool | undefined = findToolByName(this.tools, event.toolName)
            yield {
              type: 'tool_use_start',
              toolUseId: event.callId,
              toolName: event.toolName || 'CodexDynamicTool',
              input: event.input,
            }
            let deniedReason = tool ? '' : `Dynamic tool '${event.toolName}' is not allowed by the active governance policy.`
            const readOnly = tool ? Boolean(tool.isReadOnly?.(event.input)) : false
            if (!deniedReason && this.config.permissionMode === 'plan' && !readOnly) {
              deniedReason = `Dynamic tool '${event.toolName}' is not available in read-only plan mode.`
            }
            if (!deniedReason && tool && !readOnly && this.config.permissionMode !== 'bypass') {
              const approval = new Promise<ApprovalResult>((resolve) => {
                this.pendingApproval = { kind: 'dynamic-tool', resolve }
              })
              const request = {
                toolName: tool.name,
                input: event.input,
                description: tool.description,
                riskLevel: tool.riskLevel ?? 'high' as const,
                suggestedAction: 'ask' as const,
              }
              this.toolUICallback?.({
                type: 'approval',
                toolName: tool.name,
                content: tool.description,
                details: event.input,
              })
              yield { type: 'approval_required', request }
              const decision = await approval
              if (!decision.allowed) deniedReason = decision.reason
              this.pendingApproval = null
              this.toolUICallback?.(null)
            }

            if (deniedReason || !tool) {
              await invoke('codex_server_request_respond', {
                profileId: event.profileId,
                requestId: event.requestId,
                result: {
                  contentItems: [{ type: 'inputText', text: deniedReason }],
                  success: false,
                },
              })
              yield {
                type: 'tool_use_complete',
                toolUseId: event.callId,
                toolName: event.toolName || 'CodexDynamicTool',
                result: deniedReason,
                isError: true,
              }
              continue
            }

            try {
              const result = await tool.call(event.input, this.toolContext(messages))
              const text = dynamicToolResultText(result.data)
              const success = !result.isError && !result.awaitUserInput
              await invoke('codex_server_request_respond', {
                profileId: event.profileId,
                requestId: event.requestId,
                result: { contentItems: [{ type: 'inputText', text }], success },
              })
              yield {
                type: 'tool_use_complete',
                toolUseId: event.callId,
                toolName: tool.name,
                result: text,
                isError: !success,
              }
            } catch (toolError) {
              const message = toolError instanceof Error ? toolError.message : String(toolError)
              await invoke('codex_server_request_respond', {
                profileId: event.profileId,
                requestId: event.requestId,
                result: { contentItems: [{ type: 'inputText', text: message }], success: false },
              })
              yield {
                type: 'tool_use_complete',
                toolUseId: event.callId,
                toolName: tool.name,
                result: message,
                isError: true,
              }
            }
            continue
          }
          yield event
          if (event.type === 'done') break
        }
        if (!retryAnotherProfile) break
      }
      this.appState = { ...this.appState, turnCount: this.appState.turnCount + 1 }
    } finally {
      this.activeTurn = null
      this.pendingApproval = null
      this.toolUICallback?.(null)
      unlisten?.()
    }
  }
}
