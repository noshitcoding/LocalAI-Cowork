// ── Query Engine (Core) ─────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/core/query.ts + QueryEngine.ts
// The agentic loop: send messages → get response → execute tools → repeat
// Enhanced with: auto-compaction, context management, retry logic,
// tool result budgets, session persistence

import type {
  Message,
  AssistantMessage,
  UserMessage,
  AttachmentMessage,
  SystemMessage,
  ContentBlock,
  ContentBlockToolUse,
  ContentBlockToolResult,
  TokenUsage,
  StreamEvent,
  ToolProgressData,
  AppState,
  Tool,
  Tools,
  ToolUseContext,
  ToolResult,
  Command,
  AgentDefinition,
  MCPConnection,
  ToolPermissionContext,
  ApprovalRequest,
  ApprovalResult,
  ToolUIRequest,
} from '../types'
import { AgentCoordinator } from '../coordinator/agentCoordinator'
import {
  EMPTY_USAGE,
  accumulateUsage,
  generateUUID,
  createUserMessage,
  createAssistantMessage,
  extractTextContent,
  getToolUseBlocks,
  findToolByName,
  createInitialAppState,
  getEmptyToolPermissionContext,
} from '../types'
import {
  streamMessages,
  toAPIToolDefs,
  type AnthropicConfig,
  type SampleResult,
} from '../api/anthropicClient'
import {
  streamOllamaMessages,
  buildOllamaChatRequest,
  type OllamaEngineConfig,
} from '../api/ollamaClient'
import {
  streamOpenAiCompatibleMessages,
  type OpenAiCompatibleConfig,
} from '../api/openaiCompatibleClient'
import { getAllTools, getToolDefinitions, registerAllBuiltinTools } from '../tools/registry'
import { ContextManager } from '../services/contextManager'

// ── Engine Configuration ───────────────────────────────────────────────────

export type EngineBackend = 'ollama' | 'anthropic' | 'openai-compatible' | 'openrouter'

export type EngineConfig = {
  backend: EngineBackend
  anthropic?: AnthropicConfig
  ollama?: OllamaEngineConfig
  openAiCompatible?: OpenAiCompatibleConfig
  cwd: string
  systemPrompt: string
  maxTurns?: number
  maxBudgetUsd?: number
  debug?: boolean
  /** Permission mode */
  permissionMode?: 'default' | 'plan' | 'bypass' | 'strict'
  /** Allowed directories for file access */
  allowedDirectories?: string[]
  /** Custom tools (in addition to builtins) */
  customTools?: Tool[]
  /** Slash commands */
  commands?: Command[]
  /** Agent definitions for sub-agents */
  agentDefinitions?: AgentDefinition[]
  /** MCP server connections */
  mcpConnections?: MCPConnection[]
  /** Memory content (CLAUDE.md etc.) */
  memoryContent?: string
  /** Custom system prompt to append */
  appendSystemPrompt?: string
  /** Current persisted run ID */
  runId?: string
  /** Current session ID */
  sessionId?: string
  /** Current worker sandbox ID */
  sandboxId?: string
}

// ── Stream Events (yielded from queryEngine) ───────────────────────────────

export type EngineEvent =
  | { type: 'text_delta'; text: string }
  | { type: 'thinking_delta'; thinking: string }
  | { type: 'request_debug'; provider: 'ollama' | 'openai-compatible' | 'openrouter'; payload: string }
  | { type: 'tool_call_delta'; toolUseId: string; toolName: string; input: Record<string, unknown> }
  | { type: 'tool_use_start'; toolUseId: string; toolName: string; input: Record<string, unknown> }
  | { type: 'tool_use_complete'; toolUseId: string; toolName: string; result: string }
  | { type: 'tool_progress'; toolUseId: string; data: ToolProgressData }
  | { type: 'assistant_message'; message: AssistantMessage }
  | { type: 'usage_update'; usage: TokenUsage; costUsd: number; totalCostUsd: number }
  | { type: 'turn_complete'; turnCount: number; stopReason: string | null }
  | { type: 'approval_required'; request: ApprovalRequest }
  | { type: 'compaction'; removedCount: number; summary: string }
  | { type: 'context_warning'; level: 'warning' | 'critical'; estimatedTokens: number }
  | { type: 'retry'; reason: string; attempt: number }
  | { type: 'error'; error: string }
  | { type: 'done'; messages: Message[]; totalUsage: TokenUsage; totalCostUsd: number }

type PermissionDecision =
  | { kind: 'allow' }
  | { kind: 'deny'; reason: string }
  | { kind: 'ask'; request: ApprovalRequest }

// ── Query Engine Class ─────────────────────────────────────────────────────

export class QueryEngine {
  private config: EngineConfig
  private appState: AppState
  private tools: Tools
  private abortController: AbortController
  private approvalResolver: ((result: ApprovalResult) => void) | null = null
  private toolUICallback: ((ui: ToolUIRequest | null) => void) | null = null
  private initialized = false
  private contextManager: ContextManager
  private coordinator: AgentCoordinator

  constructor(config: EngineConfig) {
    this.config = config
    this.appState = createInitialAppState(config.cwd)
    this.abortController = new AbortController()

    // Initialize context manager (Claude Code feature)
    this.contextManager = new ContextManager(
      {
        autoCompactEnabled: true,
        toolResultBudgetEnabled: true,
        maxPromptTooLongRetries: 2,
      },
      config.ollama?.contextWindow ?? 120000,
    )

    // Initialize tools
    if (!this.initialized) {
      registerAllBuiltinTools()
      this.initialized = true
    }

    const builtins = getAllTools()
    this.tools = config.customTools
      ? [...builtins, ...config.customTools]
      : builtins
    this.coordinator = new AgentCoordinator(this.config)
  }

  /** Set callback for tool UI (approval dialogs, progress etc.) */
  setToolUICallback(cb: (ui: ToolUIRequest | null) => void): void {
    this.toolUICallback = cb
  }

  /** Resolve a pending approval request */
  resolveApproval(result: ApprovalResult): void {
    if (this.approvalResolver) {
      this.approvalResolver(result)
      this.approvalResolver = null
    }
  }

  /** Abort the current query */
  abort(): void {
    this.abortController.abort()
    this.abortController = new AbortController()
  }

  /** Get current application state */
  getAppState(): AppState {
    return this.appState
  }

  /** Update configuration */
  updateConfig(partial: Partial<EngineConfig>): void {
    this.config = { ...this.config, ...partial }
    this.coordinator = new AgentCoordinator(this.config)
    // Update context manager if context window changed
    if (partial.ollama?.contextWindow) {
      this.contextManager.updateMaxTokens(partial.ollama.contextWindow)
    }
  }

  /** Get context snapshot for UI display */
  getContextSnapshot(messages: Message[]) {
    return this.contextManager.getSnapshot(messages)
  }

  /** Force manual compaction */
  async forceCompact(messages: Message[]): Promise<{ messages: Message[]; summary: string }> {
    if (!this.config.ollama) {
      return { messages, summary: '' }
    }
    const result = await this.contextManager.compactIfNeeded(messages, this.config.ollama)
    return {
      messages: result.messages,
      summary: result.summary ?? '',
    }
  }

  // ── The Main Query Loop (async generator) ────────────────────────────────
  // This is the heart — mirrors Claude Code's query() function
  //
  // Flow:
  //   1. Build system prompt + messages
  //   2. Call Anthropic API (streaming)
  //   3. If response has tool_use → execute tools → add results → goto 2
  //   4. If response is end_turn → yield done
  //   5. Repeat until max_turns or budget exhausted

  async *query(
    messages: Message[],
    userInput?: string | ContentBlock[],
  ): AsyncGenerator<EngineEvent> {
    // Prepare conversation
    let conversation = [...messages]
    const hasUserInput = typeof userInput === 'string'
      ? userInput.trim().length > 0
      : Array.isArray(userInput) && userInput.length > 0
    if (hasUserInput && userInput !== undefined) {
      conversation.push(createUserMessage(userInput))
    }

    const maxTurns = this.config.maxTurns ?? 25
    let turnCount = 0
    let totalUsage: TokenUsage = { ...EMPTY_USAGE }
    let totalCostUsd = 0
    let textExecutionRecoveryCount = 0
    const maxTextExecutionRecoveries = 3

    // Build system prompt
    const fullSystemPrompt = this.buildSystemPrompt()
    const toolDefs = getToolDefinitions()

    // ── Agentic Loop ──────────────────────────────────────────────────────
    while (turnCount < maxTurns) {
      if (this.abortController.signal.aborted) {
        yield { type: 'error', error: 'Abgebrochen.' }
        break
      }

      turnCount++
      this.appState = { ...this.appState, turnCount }

      // ── Auto-Compaction (Claude Code feature) ────────────────────────────
      // Check context size before API call, compact if needed
      if (this.config.ollama && this.contextManager.shouldCompact(conversation)) {
        try {
          const compactResult = await this.contextManager.compactIfNeeded(
            conversation,
            this.config.ollama,
            this.abortController.signal,
          )
          if (compactResult.didCompact) {
            conversation = compactResult.messages
            yield {
              type: 'compaction',
              removedCount: conversation.length,
              summary: compactResult.summary ?? '',
            }
          }
        } catch {
          // compaction failure is non-fatal
        }
      }

      // ── Context Warning ──────────────────────────────────────────────────
      const snapshot = this.contextManager.getSnapshot(conversation)
      if (snapshot.warningLevel !== 'none') {
        yield {
          type: 'context_warning',
          level: snapshot.warningLevel,
          estimatedTokens: snapshot.totalTokens,
        }
      }

      // Apply tool result budget (Claude Code feature: truncate large results)
      const budgetedConversation = this.contextManager.applyBudget(conversation)

      // Convert messages to API format
      const apiMessages = this.toAPIConversation(budgetedConversation)

      // ── Stream from API (with retry for prompt-too-long) ─────────────────
      let sampleResult: SampleResult
      let retryAttempt = 0
      const MAX_RETRIES = 2

      while (true) {
        try {
          let currentUsage: TokenUsage = { ...EMPTY_USAGE }

          if (this.config.backend === 'ollama' && this.config.ollama) {
            const { debugPreview } = buildOllamaChatRequest(this.config.ollama, apiMessages, fullSystemPrompt, toolDefs)
            yield { type: 'request_debug', provider: 'ollama', payload: debugPreview }
          }

          const stream = this.createStream(apiMessages, fullSystemPrompt, toolDefs)

          let streamResult: IteratorResult<StreamEvent, SampleResult>

          while (true) {
            streamResult = await stream.next()

            if (streamResult.done) {
              sampleResult = streamResult.value
              break
            }

            const event = streamResult.value

            // Forward stream events
            switch (event.type) {
              case 'content_block_start': {
                if (event.content_block.type === 'tool_use') {
                  yield {
                    type: 'tool_call_delta',
                    toolUseId: event.content_block.id,
                    toolName: event.content_block.name,
                    input: event.content_block.input,
                  }
                }
                break
              }
              case 'content_block_delta': {
                if (event.delta.type === 'text_delta') {
                  yield { type: 'text_delta', text: event.delta.text }
                } else if (event.delta.type === 'thinking_delta') {
                  yield { type: 'thinking_delta', thinking: event.delta.thinking }
                }
                break
              }
              case 'message_start': {
                currentUsage = event.message.usage
                break
              }
              case 'message_delta': {
                if (event.usage) currentUsage = accumulateUsage(currentUsage, event.usage)
                break
              }
            }
          }

          // Reset retry counter on success
          this.contextManager.resetPromptTooLongCount()
          break // exit retry loop
        } catch (error) {
          const msg = error instanceof Error ? error.message : String(error)

          // ── Prompt-too-long retry (Claude Code feature) ──────────────────
          const isPromptTooLong = msg.toLowerCase().includes('too long') ||
            msg.toLowerCase().includes('context length') ||
            msg.toLowerCase().includes('maximum context') ||
            msg.includes('num_ctx')

          if (isPromptTooLong && retryAttempt < MAX_RETRIES && this.config.ollama) {
            retryAttempt++
            yield { type: 'retry', reason: 'Kontext zu lang — komprimiere...', attempt: retryAttempt }

            const compacted = await this.contextManager.handlePromptTooLong(
              conversation,
              this.config.ollama,
              this.abortController.signal,
            )

            if (compacted) {
              conversation = compacted
              // Rebuild API messages and retry
              const newApiMessages = this.toAPIConversation(
                this.contextManager.applyBudget(conversation),
              )
              apiMessages.length = 0
              apiMessages.push(...newApiMessages)
              continue
            }
          }

          yield { type: 'error', error: msg }
          // Yield done with current state on error
          yield {
            type: 'done',
            messages: conversation,
            totalUsage,
            totalCostUsd,
          }
          return
        }
      }

      // ── Process Response ─────────────────────────────────────────────────
      totalUsage = accumulateUsage(totalUsage, sampleResult.usage)
      const turnCost = sampleResult.costUsd
      totalCostUsd += turnCost

      this.appState = {
        ...this.appState,
        totalTokens: {
          input: totalUsage.input_tokens,
          output: totalUsage.output_tokens,
        },
        totalCostUsd,
      }

      yield {
        type: 'usage_update',
        usage: sampleResult.usage,
        costUsd: turnCost,
        totalCostUsd,
      }

      // Create assistant message
      const assistantMsg = createAssistantMessage(
        sampleResult.content,
        sampleResult.model,
        sampleResult.usage,
        sampleResult.stopReason as AssistantMessage['stopReason'],
      )
      conversation.push(assistantMsg)
      yield { type: 'assistant_message', message: assistantMsg }

      // ── Check for Tool Use ───────────────────────────────────────────────
      const toolUseBlocks = getToolUseBlocks(assistantMsg)

      if (toolUseBlocks.length === 0) {
        const assistantText = extractTextContent(assistantMsg)
        const latestUserText = this.getLatestUserText(conversation)

        if (textExecutionRecoveryCount < maxTextExecutionRecoveries && this.config.permissionMode !== 'plan') {
          const executionRecoveryMessage = this.buildNarratedExecutionRecoveryMessage(assistantText, latestUserText)
          if (executionRecoveryMessage) {
            textExecutionRecoveryCount += 1
            conversation.push(createUserMessage(executionRecoveryMessage))
            continue
          }

          const desktopRecoveryMessage = this.buildDesktopExecutionRecoveryMessage(assistantText, latestUserText)
          if (desktopRecoveryMessage) {
            textExecutionRecoveryCount += 1
            conversation.push(createUserMessage(desktopRecoveryMessage))
            continue
          }

          const fallbackApproval = this.buildTextPlanApprovalRequest(assistantText, latestUserText)
          if (fallbackApproval) {
            textExecutionRecoveryCount += 1
            const approvalPromise = this.beginApprovalRequest(fallbackApproval)
            yield { type: 'approval_required', request: fallbackApproval }
            const approved = await approvalPromise

            if (!approved.allowed) {
              yield { type: 'turn_complete', turnCount, stopReason: 'approval_denied' }
              break
            }

            conversation.push(createUserMessage(
              'Freigabe erteilt. Fuehre den zuletzt beschriebenen Plan jetzt direkt mit den verfuegbaren Tools aus. '
              + 'Antworte nicht erneut mit einem Plan. Nutze Tool-Calls fuer die Ausfuehrung und gib danach nur das Ergebnis aus.',
            ))
            continue
          }
        }

        // No tool calls, turn is complete
        yield { type: 'turn_complete', turnCount, stopReason: sampleResult.stopReason }
        break
      }

      // ── Execute Tools ────────────────────────────────────────────────────
      // Check budget before executing
      if (this.config.maxBudgetUsd && totalCostUsd >= this.config.maxBudgetUsd) {
        yield { type: 'error', error: `Budget-Limit erreicht: $${totalCostUsd.toFixed(4)} / $${this.config.maxBudgetUsd}` }
        break
      }

      const toolResults: ContentBlockToolResult[] = []
      const injectedMessages: Message[] = []
      const context = this.buildToolContext(conversation)
      let shouldAwaitUserInput = false

      // Separate concurrent-safe and sequential tools
      const concurrentTools: Array<{ block: ContentBlockToolUse; tool: Tool }> = []
      const sequentialTools: Array<{ block: ContentBlockToolUse; tool: Tool }> = []

      for (const block of toolUseBlocks) {
        const tool = findToolByName(this.tools, block.name)
        if (!tool) {
          toolResults.push({
            type: 'tool_result',
            tool_use_id: block.id,
            content: `Tool "${block.name}" nicht gefunden.`,
            is_error: true,
          })
          continue
        }

        // Announce tool start
        yield { type: 'tool_use_start', toolUseId: block.id, toolName: tool.name, input: block.input }

        // Permission check
        if (this.config.permissionMode !== 'bypass') {
          const decision = this.evaluatePermission(tool, block.input, context)
          let approved: ApprovalResult

          if (decision.kind === 'ask') {
            const approvalPromise = this.beginApprovalRequest(decision.request)
            yield { type: 'approval_required', request: decision.request }
            approved = await approvalPromise
          } else if (decision.kind === 'deny') {
            approved = { allowed: false, reason: decision.reason }
          } else {
            approved = { allowed: true }
          }

          if (!approved.allowed) {
            toolResults.push({
              type: 'tool_result',
              tool_use_id: block.id,
              content: `Tool "${tool.name}" abgelehnt: ${approved.reason}`,
              is_error: true,
            })
            yield { type: 'tool_use_complete', toolUseId: block.id, toolName: tool.name, result: `Abgelehnt: ${approved.reason}` }
            continue
          }
        }

        if (tool.isConcurrencySafe?.(block.input)) {
          concurrentTools.push({ block, tool })
        } else {
          sequentialTools.push({ block, tool })
        }
      }

      // Execute concurrent tools in parallel
      if (concurrentTools.length > 0) {
        const results = await Promise.allSettled(
          concurrentTools.map(async ({ block, tool }) => {
            const result = await this.executeTool(tool, block, context, (_data) => {
              // Emit progress events during tool execution
            })
            return { block, result }
          }),
        )

        for (const r of results) {
          if (r.status === 'fulfilled') {
            const { block, result } = r.value
            const resultStr = typeof result.data === 'string' ? result.data : JSON.stringify(result.data)
            toolResults.push({
              type: 'tool_result',
              tool_use_id: block.id,
              content: resultStr,
            })
            if (result.newMessages) {
              injectedMessages.push(...result.newMessages)
            }
            if (result.awaitUserInput) {
              shouldAwaitUserInput = true
            }
            yield { type: 'tool_use_complete', toolUseId: block.id, toolName: block.name, result: resultStr }
          } else {
            const block = concurrentTools[results.indexOf(r)].block
            toolResults.push({
              type: 'tool_result',
              tool_use_id: block.id,
              content: `Fehler: ${r.reason}`,
              is_error: true,
            })
          }
        }
      }

      // Execute sequential tools one at a time
      for (const { block, tool } of sequentialTools) {
        try {
          const toolExecution = this.executeToolStream(tool, block, context)
          let nextStep = await toolExecution.next()
          while (!nextStep.done) {
            yield nextStep.value
            nextStep = await toolExecution.next()
          }
          const result = nextStep.value
          const resultStr = typeof result.data === 'string' ? result.data : JSON.stringify(result.data)
          toolResults.push({
            type: 'tool_result',
            tool_use_id: block.id,
            content: resultStr,
          })
          if (result.newMessages) {
            injectedMessages.push(...result.newMessages)
          }
          if (result.awaitUserInput) {
            shouldAwaitUserInput = true
          }
          yield { type: 'tool_use_complete', toolUseId: block.id, toolName: tool.name, result: resultStr }

          // Apply context modifier if present
          if (result.contextModifier) {
            Object.assign(context, result.contextModifier(context))
          }
        } catch (err) {
          const msg = err instanceof Error ? err.message : String(err)
          toolResults.push({
            type: 'tool_result',
            tool_use_id: block.id,
            content: `Fehler: ${msg}`,
            is_error: true,
          })
          yield { type: 'tool_use_complete', toolUseId: block.id, toolName: tool.name, result: `Fehler: ${msg}` }
        }
      }

      // Add tool results as user message
      const toolResultMessage: UserMessage = {
        type: 'user',
        uuid: generateUUID(),
        content: toolResults,
        timestamp: Date.now(),
      }
      conversation.push(toolResultMessage)
      if (injectedMessages.length > 0) {
        conversation.push(...injectedMessages)
      }

      yield {
        type: 'turn_complete',
        turnCount,
        stopReason: shouldAwaitUserInput ? 'await_user' : 'tool_use',
      }
      if (shouldAwaitUserInput) {
        break
      }
      // Loop continues — API will see tool results and respond
    }

    // ── Final State ────────────────────────────────────────────────────────
    yield {
      type: 'done',
      messages: conversation,
      totalUsage,
      totalCostUsd,
    }
  }

  // ── Private Methods ──────────────────────────────────────────────────────

  private buildSystemPrompt(): string {
    const parts: string[] = []

    // Base system prompt
    parts.push(this.config.systemPrompt)

    // Memory content
    if (this.config.memoryContent) {
      parts.push(`\n\n<memory>\n${this.config.memoryContent}\n</memory>`)
    }

    // Available tools description
    const tools = this.tools
    if (tools.length > 0) {
      const toolList = tools
        .map((t) => {
          const aliasInfo = t.aliases && t.aliases.length > 0
            ? ` (Aliases: ${t.aliases.join(', ')})`
            : ''
          return `- ${t.name}${aliasInfo}: ${t.description}`
        })
        .join('\n')
      parts.push(`\n\nVerfuegbare Tools:\n${toolList}`)
      parts.push(
        '\n\nTool-Nutzung:\n'
        + '- Wenn eine Aufgabe Dateioperationen verlangt, fuehre sie mit Tools direkt aus statt nur Schritte zu beschreiben.\n'
        + '- Fuer Strukturarbeit nutze bevorzugt ListDir/Glob zum Verstehen und danach CreateDirectory, MovePath, CopyPath, Write oder Edit.\n'
        + '- Bei Desktop-Aufgaben arbeite im Loop Beobachten -> Aktion -> Verifikation. Wenn ein Verifikations-Screenshot zeigt, dass das Ziel noch nicht erreicht ist, fuehre direkt den naechsten Tool-Call aus statt nur die Absicht zu beschreiben.\n'
        + '- Behaupte bei Desktop-Steuerung nie, dass ein Klick, Schliessen, Fokuswechsel oder eine Eingabe erfolgreich war, bevor der aktuelle Verifikations-Screenshot das bestaetigt.\n'
        + '- Wenn dein lokales Modell keine nativen Tool-Calls sendet, gib Tool-Aufrufe notfalls exakt als eigene Zeile im Format ToolName({"arg":"wert"}) aus.',
      )
    }

    // Plan mode notice
    if (this.appState.planMode) {
      parts.push('\n\n⚠ PLAN-MODUS AKTIV: Beschreibe nur, was du tun wuerdest. Fuehre keine Aenderungen aus.')
    }

    // Append custom system prompt
    if (this.config.appendSystemPrompt) {
      parts.push(`\n\n${this.config.appendSystemPrompt}`)
    }

    return parts.join('')
  }

  private getLatestUserText(messages: Message[]): string {
    for (let index = messages.length - 1; index >= 0; index--) {
      const message = messages[index]
      if (message.type !== 'user') continue
      const text = extractTextContent(message).trim()
      if (text) return text
    }
    return ''
  }

  private buildTextPlanApprovalRequest(
    assistantText: string,
    latestUserText: string,
  ): ApprovalRequest | null {
    const normalizedAssistant = assistantText.trim()
    const normalizedUser = latestUserText.trim()
    if (!normalizedAssistant || !normalizedUser) return null

    const looksExecutable = /(sortier|verschieb|bewege|kopier|organisier|erstelle|schreibe|loesch|lösch|umbenenn|move|copy|organize|sort|create|write|delete|rename)/i.test(normalizedUser)
    if (!looksExecutable) return null

    const lines = normalizedAssistant
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean)
    const structuredPlanLines = lines.filter((line) => /^\d+[.)]\s+/.test(line) || /^[-*]\s+/.test(line)).length
    const mentionsApproval = /(approval|freigabe|destruktiv|riskant|vorsicht|plan:|dateiverschiebung)/i.test(normalizedAssistant)

    if (!mentionsApproval && structuredPlanLines < 2) return null

    const summary = lines.slice(0, 4).join(' | ').slice(0, 260)
    return {
      toolName: 'PlannedExecution',
      input: {
        task: normalizedUser.slice(0, 500),
        plan: normalizedAssistant.slice(0, 2000),
      },
      description: summary || normalizedAssistant.slice(0, 260),
      riskLevel: 'medium',
      suggestedAction: 'ask',
    }
  }

  private buildDesktopExecutionRecoveryMessage(
    assistantText: string,
    latestUserText: string,
  ): string | null {
    const normalizedAssistant = assistantText.trim()
    const normalizedUser = latestUserText.trim()
    if (!normalizedAssistant || !normalizedUser) return null

    const hasDesktopTools = this.tools.some((tool) => tool.category === 'desktop')
    if (!hasDesktopTools) return null

    const desktopIntent = /(desktop|bildschirm|screen|screenshot|fenster|window|dialog|papierkorb|kicad|click|klick|taste|keypress|tippe|type|scroll|focus|fokus|schlie(?:ss|ß)|close|oeffne|öffne|starte|launch)/i
    const planLanguage = /(ich werde|werde ich|ich versuche|ich probiere|nun werde ich|als naechstes|als nächstes|sobald|danach werde ich|ich ziele|ich starte jetzt|werde nun)/i
    const explicitToolCall = /[A-Za-z][A-Za-z0-9_]*\s*\(\s*\{/.test(normalizedAssistant)

    if ((!desktopIntent.test(normalizedUser) && !desktopIntent.test(normalizedAssistant)) || !planLanguage.test(normalizedAssistant) || explicitToolCall) {
      return null
    }

    return 'Fuehre den naechsten Desktop-Schritt jetzt direkt mit den verfuegbaren Desktop-Tools aus. Bleibe im Loop Beobachten -> Aktion -> Verifikation: nutze den aktuellen Verifikations-Screenshot, pruefe ob das Ziel erreicht wurde, und wenn nicht, fuehre sofort den naechsten plausiblen Tool-Call aus. Antworte nicht erneut nur mit einer Absichtserklaerung.'
  }

  private buildNarratedExecutionRecoveryMessage(
    assistantText: string,
    latestUserText: string,
  ): string | null {
    const normalizedAssistant = assistantText.trim()
    const normalizedUser = latestUserText.trim()
    if (!normalizedAssistant) return null

    const hasActionTools = this.tools.some((tool) => tool.category === 'desktop' || tool.category === 'shell' || tool.category === 'filesystem')
    if (!hasActionTools) return null

    const planLanguage = /(ich werde|werde ich|ich versuche|ich probiere|nun werde ich|als naechstes|als nächstes|sobald|danach werde ich|ich starte jetzt|ich beginne jetzt|werde nun|ich werde nun)/i
    const shellOrFilesystemIntent = /(powershell|shell|bash|cmd|prozess|stop-process|taskkill|dateisystem|filesystem|explorer|terminal|konsole|verzeichnis|directory|folder|path:|^[A-Z]:\\)/i
    const explicitToolCall = /[A-Za-z][A-Za-z0-9_]*\s*\(\s*\{/.test(normalizedAssistant)

    if (explicitToolCall || !planLanguage.test(normalizedAssistant)) {
      return null
    }

    if (!shellOrFilesystemIntent.test(normalizedAssistant) && !shellOrFilesystemIntent.test(normalizedUser)) {
      return null
    }

    return 'Fuehre den zuletzt beschriebenen naechsten Schritt jetzt direkt mit den verfuegbaren Tools aus. Nutze Shell-, Datei- oder Desktop-Tools sofort statt den Plan weiter zu beschreiben. Wenn ein Schritt scheitert, probiere den naechsten plausiblen Tool-Call und verifiziere das Ergebnis, anstatt nur Absichten zu formulieren.'
  }

  private toAPIConversation(messages: Message[]): Array<{ role: 'user' | 'assistant'; content: string | ContentBlock[] }> {
    const apiMsgs: Array<{ role: 'user' | 'assistant'; content: string | ContentBlock[] }> = []

    for (const msg of messages) {
      switch (msg.type) {
        case 'user':
          apiMsgs.push({ role: 'user', content: msg.content })
          break
        case 'assistant':
          apiMsgs.push({ role: 'assistant', content: msg.content })
          break
        case 'system':
          // System messages become user messages with [system] prefix
          apiMsgs.push({ role: 'user', content: [{ type: 'text', text: `[System: ${msg.content}]` }] })
          break
        case 'attachment':
          apiMsgs.push({ role: 'user', content: msg.content })
          break
        // Skip tombstone, progress, summary messages
      }
    }

    // Ensure messages alternate user/assistant properly
    return this.ensureAlternation(apiMsgs)
  }

  private ensureAlternation(messages: Array<{ role: 'user' | 'assistant'; content: string | ContentBlock[] }>): Array<{ role: 'user' | 'assistant'; content: string | ContentBlock[] }> {
    if (messages.length === 0) return messages

    const result: Array<{ role: 'user' | 'assistant'; content: string | ContentBlock[] }> = []

    for (const msg of messages) {
      const lastRole = result.length > 0 ? result[result.length - 1].role : null

      if (lastRole === msg.role) {
        // Merge same-role consecutive messages
        const lastMsg = result[result.length - 1]
        if (typeof lastMsg.content === 'string' && typeof msg.content === 'string') {
          lastMsg.content = `${lastMsg.content}\n${msg.content}`
        } else {
          const prevBlocks = typeof lastMsg.content === 'string'
            ? [{ type: 'text' as const, text: lastMsg.content }]
            : lastMsg.content
          const newBlocks = typeof msg.content === 'string'
            ? [{ type: 'text' as const, text: msg.content }]
            : msg.content
          lastMsg.content = [...prevBlocks, ...newBlocks]
        }
      } else {
        result.push({ ...msg })
      }
    }

    // First message must be user
    if (result.length > 0 && result[0].role !== 'user') {
      result.unshift({ role: 'user', content: '' })
    }

    return result
  }

  private buildToolContext(messages: Message[]): ToolUseContext {
    const permissionContext: ToolPermissionContext = this.config.permissionMode === 'bypass'
      ? { ...getEmptyToolPermissionContext(), mode: 'bypass', allowedDirectories: this.config.allowedDirectories ?? [] }
      : { ...getEmptyToolPermissionContext(), mode: this.config.permissionMode ?? 'default', allowedDirectories: this.config.allowedDirectories ?? [] }

    return {
      cwd: this.appState.cwd || this.config.cwd,
      abortController: this.abortController,
      debug: this.config.debug ?? false,
      model: this.getModel(),
      tools: this.tools,
      commands: this.config.commands ?? [],
      getAppState: () => this.appState,
      setAppState: (f) => { this.appState = f(this.appState) },
      setToolUI: this.toolUICallback ?? undefined,
      requestApproval: (req) => this.requestApproval(req),
      permissionContext,
      canUseTool: async (toolName, input, ctx) => this.checkPermission(
        findToolByName(this.tools, toolName)!,
        input,
        ctx,
      ),
      mcpConnections: this.config.mcpConnections ?? [],
      agentDefinitions: this.config.agentDefinitions ?? [],
      memoryContent: this.config.memoryContent,
      runId: this.config.runId,
      sessionId: this.config.sessionId,
      sandboxId: this.config.sandboxId,
      messages,
    }
  }

  private async executeTool(
    tool: Tool,
    block: ContentBlockToolUse,
    context: ToolUseContext,
    onProgress: (data: ToolProgressData) => void,
  ): Promise<ToolResult> {
    if (tool.name === 'Agent') {
      return this.executeAgentTool(block, context, onProgress)
    }
    return tool.call(block.input, context, (progress) => {
      onProgress(progress.data)
    })
  }

  private async *executeToolStream(
    tool: Tool,
    block: ContentBlockToolUse,
    context: ToolUseContext,
  ): AsyncGenerator<EngineEvent, ToolResult> {
    const progressQueue: ToolProgressData[] = []
    let wakeUp: (() => void) | null = null
    let settled = false
    let result: ToolResult | null = null
    let failure: unknown = null

    const notify = () => {
      if (wakeUp) {
        const resolve = wakeUp
        wakeUp = null
        resolve()
      }
    }

    void this.executeTool(tool, block, context, (data) => {
      progressQueue.push(data)
      notify()
    })
      .then((value) => {
        result = value
        settled = true
        notify()
      })
      .catch((error) => {
        failure = error
        settled = true
        notify()
      })

    while (!settled || progressQueue.length > 0) {
      while (progressQueue.length > 0) {
        yield {
          type: 'tool_progress',
          toolUseId: block.id,
          data: progressQueue.shift()!,
        }
      }

      if (!settled) {
        await new Promise<void>((resolve) => {
          wakeUp = resolve
        })
      }
    }

    if (failure) {
      throw failure
    }

    return result ?? { data: '' }
  }

  private async executeAgentTool(
    block: ContentBlockToolUse,
    context: ToolUseContext,
    onProgress: (data: ToolProgressData) => void,
  ): Promise<ToolResult> {
    const requestedName = String(block.input.agent_name ?? '').trim()
    const prompt = String(block.input.prompt ?? '').trim()

    if (!requestedName || !prompt) {
      return { data: 'Fehler: agent_name und prompt sind erforderlich.' }
    }

    const definition = (this.config.agentDefinitions ?? []).find((item) =>
      item.id === requestedName || item.name.toLowerCase() === requestedName.toLowerCase(),
    )

    if (!definition) {
      return { data: `Fehler: Agent "${requestedName}" nicht gefunden.` }
    }

    onProgress({
      type: 'agent_progress',
      agentName: definition.name,
      content: `Agent "${definition.name}" startet...`,
    })

    let finalResult = ''
    let failure: string | null = null
    const forwardedMessages: Array<UserMessage | AssistantMessage | AttachmentMessage | SystemMessage> = []

    for await (const event of this.coordinator.spawnAgent(definition, prompt, context.messages)) {
      if (event.type === 'text_delta') {
        finalResult += event.text
      } else if (event.type === 'assistant_message') {
        forwardedMessages.push(event.message)
      } else if (event.type === 'error') {
        failure = event.error
      } else if (event.type === 'done') {
        const lastAssistant = [...event.messages]
          .reverse()
          .find((message): message is AssistantMessage => message.type === 'assistant')
        if (lastAssistant) {
          finalResult = extractTextContent(lastAssistant)
        }
      }
    }

    if (failure) {
      return { data: `Sub-Agent fehlgeschlagen: ${failure}` }
    }

    const summarized = finalResult.trim() || `Sub-Agent "${definition.name}" abgeschlossen.`
    return {
      data: summarized,
      newMessages: forwardedMessages,
    }
  }

  private getModel(): string {
    if (this.config.backend === 'ollama') return this.config.ollama?.model ?? 'llama3.1:8b'
    if (this.config.backend === 'openai-compatible' || this.config.backend === 'openrouter') {
      return this.config.openAiCompatible?.model ?? 'gpt-4.1-mini'
    }
    return this.config.anthropic?.model ?? 'claude-sonnet-4-20250514'
  }

  private createStream(
    apiMessages: Array<{ role: 'user' | 'assistant'; content: string | ContentBlock[] }>,
    systemPrompt: string,
    toolDefs: ReturnType<typeof getToolDefinitions>,
  ): AsyncGenerator<StreamEvent, SampleResult> {
    if (this.config.backend === 'ollama') {
      return streamOllamaMessages(
        this.config.ollama!,
        apiMessages,
        systemPrompt,
        toolDefs,
        this.abortController.signal,
      )
    }

    if (this.config.backend === 'openai-compatible' || this.config.backend === 'openrouter') {
      return streamOpenAiCompatibleMessages(
        this.config.openAiCompatible!,
        apiMessages,
        systemPrompt,
        toAPIToolDefs(toolDefs),
        this.abortController.signal,
      )
    }

    return streamMessages(
      this.config.anthropic!,
      apiMessages,
      systemPrompt,
      toAPIToolDefs(toolDefs),
      this.abortController.signal,
    )
  }

  private async checkPermission(
    tool: Tool,
    input: Record<string, unknown>,
    context: ToolUseContext,
  ): Promise<ApprovalResult> {
    const decision = this.evaluatePermission(tool, input, context)

    if (decision.kind === 'allow') {
      return { allowed: true }
    }

    if (decision.kind === 'deny') {
      return { allowed: false, reason: decision.reason }
    }

    return this.requestApproval(decision.request)
  }

  private evaluatePermission(
    tool: Tool,
    input: Record<string, unknown>,
    context: ToolUseContext,
  ): PermissionDecision {
    const mode = context.permissionContext.mode

    // Bypass mode: always allow
    if (mode === 'bypass') return { kind: 'allow' }

    // Read-only tools in default mode: allow
    if (mode === 'default' && tool.isReadOnly?.(input)) return { kind: 'allow' }

    // Check deny rules
    for (const rule of context.permissionContext.denyRules) {
      if (tool.name.match(new RegExp(rule.pattern, 'i'))) {
        return { kind: 'deny', reason: `Blockiert durch Regel: ${rule.source}` }
      }
    }

    // Check allow rules
    for (const rule of context.permissionContext.allowRules) {
      if (tool.name.match(new RegExp(rule.pattern, 'i'))) {
        return { kind: 'allow' }
      }
    }

    // Asking the user is itself the human-in-the-loop step. Requiring a
    // separate approval before a low-risk clarification creates a dead-end UX.
    if (tool.category === 'user_interaction' && tool.isReadOnly?.(input) && (tool.riskLevel ?? 'low') === 'low') {
      return { kind: 'allow' }
    }

    // Strict mode and plan mode: ask for everything writeable
    if (mode === 'strict' || mode === 'plan' || tool.riskLevel === 'high') {
      return {
        kind: 'ask',
        request: {
          toolName: tool.name,
          input: input,
          description: `${tool.name}: ${JSON.stringify(input).slice(0, 200)}`,
          riskLevel: tool.riskLevel ?? 'medium',
          suggestedAction: 'ask',
        },
      }
    }

    // Default: allow read-only and low-risk, ask for medium+
    if (tool.riskLevel === 'low') return { kind: 'allow' }

    return {
      kind: 'ask',
      request: {
        toolName: tool.name,
        input: input,
        description: `${tool.name}: ${JSON.stringify(input).slice(0, 200)}`,
        riskLevel: tool.riskLevel ?? 'medium',
        suggestedAction: 'ask',
      },
    }
  }

  private requestApproval(request: ApprovalRequest): Promise<ApprovalResult> {
    return this.beginApprovalRequest(request)
  }

  private beginApprovalRequest(request: ApprovalRequest): Promise<ApprovalResult> {
    // Show approval UI
    if (this.toolUICallback) {
      this.toolUICallback({
        type: 'approval',
        toolName: request.toolName,
        content: request.description,
        details: { input: request.input, riskLevel: request.riskLevel },
      })
    }

    // Wait for user resolution
    return new Promise<ApprovalResult>((resolve) => {
      this.approvalResolver = resolve

      // Auto-approve after timeout if no resolver set up externally
      setTimeout(() => {
        if (this.approvalResolver === resolve) {
          this.approvalResolver = null
          resolve({ allowed: true, reason: 'Auto-approved (timeout)' })
        }
      }, 60000) // 60s timeout
    })
  }
}

// ── Factory ────────────────────────────────────────────────────────────────

export function createQueryEngine(config: EngineConfig): QueryEngine {
  return new QueryEngine(config)
}
