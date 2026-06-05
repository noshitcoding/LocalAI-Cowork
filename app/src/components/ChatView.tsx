import { useRef, useEffect, useMemo, useState, useCallback } from 'react'
import type { ClipboardEvent, FormEvent } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { useChatStore, getActiveThread } from '../stores/chatStore'
import type { LiveToolCall, LiveToolCallStatus, PermissionMode, PermissionConfig } from '../stores/chatStore'

import { useConfigStore } from '../stores/configStore'
import { useTaskStore } from '../stores/taskStore'
import { useLogStore } from '../stores/logStore'
import type { TaskStep } from '../stores/taskStore'
import type { ContentBlock } from '../engine'
import {
  createInlineImageAttachment,
  getAttachmentDisplayName,
  getAttachmentPreviewSrcForAttachment,
  hasLocalAttachmentPath,
  isImageAttachment,
  mergeAttachments,
  normalizeDialogSelection,
  toImageContentBlocks,
  type ChatAttachment,
} from '../utils/chatAttachments'
import { buildAttachmentPromptContext } from '../utils/attachmentPromptContext'
import { compactHistoryForPrompt } from '../utils/claudeBridge'
import { resolveAssistantPresentation, resolveDisplayedAssistantContent, resolveDisplayedThinkingContent, splitPromptDebugContent } from '../utils/messageDisplay'
import { appendWebSearchSources, mergeWebSearchSources, parseWebSearchSourcesFromToolResult, type WebSearchSource } from '../utils/webSearchSources'
import { detectModelCapabilities } from '../engine/api/ollamaClient'
import { MessageThinking, MessageVerbose } from './MessageThinking'
import { HighlightedChatText } from './HighlightedChatText'
import CrewLiveMonitor from './CrewLiveMonitor'
import { writeAuditEvent } from '../utils/audit'
import { buildSystemPromptFromPersonality } from '../utils/defaultSeeds'
import { getSlashCommandSuggestions, useCommandRegistry } from '../stores/commandRegistryStore'
import { usePersonalityStore } from '../stores/personalityStore'
import { useCoworkStore } from '../stores/coworkStore'
import { useMemoryStore } from '../stores/memoryStore'
import { safeInvoke } from '../utils/safeInvoke'
import { useEngineStore } from '../stores/engineStore'
import {
  CHAT_PROVIDER_LABELS,
  CHAT_PROVIDER_OPTIONS,
  createChatProviderSelection,
  getChatProviderFailureHint,
  getChatProviderState,
  normalizeChatProvider,
} from '../utils/chatProvider'
import { tr } from '../i18n'

function getParentDirectory(path: string): string {
  const normalized = path.trim()
  const lastSeparatorIndex = Math.max(normalized.lastIndexOf('\\'), normalized.lastIndexOf('/'))
  if (lastSeparatorIndex < 0) return '.'
  if (lastSeparatorIndex === 0) return normalized.slice(0, 1)
  if (lastSeparatorIndex === 2 && /^[a-zA-Z]:[\\/]/.test(normalized)) {
    return normalized.slice(0, 3)
  }
  return normalized.slice(0, lastSeparatorIndex)
}

function getEffectiveChatCwd(
  attachments: ChatAttachment[],
  workspaceDefaultPath: string,
): string {
  for (const attachment of attachments) {
    if (!hasLocalAttachmentPath(attachment)) continue
    const normalized = attachment.path.trim()
    if (normalized && attachment.kind === 'folder') {
      return normalized
    }
  }

  for (const attachment of attachments) {
    if (!hasLocalAttachmentPath(attachment)) continue
    const normalized = attachment.path.trim()
    if (normalized && attachment.kind === 'file') {
      return getParentDirectory(normalized)
    }
  }

  return workspaceDefaultPath.trim() || '.'
}

async function buildEngineUserInput(promptWithAttachments: string, attachments: ChatAttachment[]): Promise<string | ContentBlock[]> {
  const imageBlocks = await toImageContentBlocks(attachments)
  if (imageBlocks.length === 0) {
    return promptWithAttachments
  }

  return [
    { type: 'text', text: promptWithAttachments },
    ...imageBlocks,
  ]
}

function waitForNextPaint(): Promise<void> {
  if (typeof window === 'undefined') {
    return Promise.resolve()
  }

  return new Promise((resolve) => {
    window.requestAnimationFrame(() => {
      window.setTimeout(resolve, 0)
    })
  })
}

type LiveToolCallPatch = Partial<Omit<LiveToolCall, 'id' | 'startedAt'>> & {
  id: string
  toolName: string
  input: Record<string, unknown>
}

function upsertLiveToolCall(calls: LiveToolCall[], patch: LiveToolCallPatch): LiveToolCall[] {
  const now = Date.now()
  const existingIndex = calls.findIndex((call) => call.id === patch.id)
  if (existingIndex < 0) {
    return [
      ...calls,
      {
        id: patch.id,
        toolName: patch.toolName,
        input: patch.input,
        status: patch.status ?? 'requested',
        result: patch.result,
        error: patch.error,
        startedAt: now,
        finishedAt: patch.finishedAt,
      },
    ]
  }

  return calls.map((call, index) => {
    if (index !== existingIndex) return call
    return {
      ...call,
      ...patch,
      startedAt: call.startedAt,
      finishedAt: patch.finishedAt ?? call.finishedAt,
    }
  })
}

function findLiveToolCallId(calls: LiveToolCall[], toolName: string, input: Record<string, unknown>): string {
  const inputJson = JSON.stringify(input ?? {})
  const exact = calls.find((call) => call.toolName === toolName && JSON.stringify(call.input ?? {}) === inputJson)
  if (exact) return exact.id

  const active = [...calls].reverse().find((call) =>
    call.toolName === toolName && (
      call.status === 'requested' ||
      call.status === 'running' ||
      call.status === 'approval' ||
      call.status === 'waiting_input'
    )
  )
  if (active) return active.id

  return `approval-${toolName}-${Date.now()}`
}

function formatToolPayload(value: unknown): string {
  if (value === undefined || value === null || value === '') return ''
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

function getToolStatusLabel(status: LiveToolCallStatus): string {
  switch (status) {
    case 'requested':
      return 'Tool call detected'
    case 'running':
      return 'Tool is running'
    case 'approval':
      return 'Approval required'
    case 'waiting_input':
      return 'Waiting for answer'
    case 'completed':
      return 'Completed'
    case 'failed':
      return 'Fehlgeschlagen'
  }
}

function AskUserForm({ call, onRespond }: { call: LiveToolCall; onRespond: (answer: string) => void }) {
  const [selectedOptions, setSelectedOptions] = useState<string[]>([])
  const [freeText, setFreeText] = useState('')
  const options = call.options ?? []
  const allowMultiple = call.allowMultiple ?? false
  const allowFreeform = call.allowFreeformInput ?? true
  const freeTextLabel = call.freeTextLabel || 'Freitext'
  const freeTextPlaceholder = call.freeTextPlaceholder || 'Add optional details...'

  const handleToggleOption = (value: string) => {
    if (allowMultiple) {
      setSelectedOptions((prev) =>
        prev.includes(value) ? prev.filter((v) => v !== value) : [...prev, value],
      )
    } else {
      setSelectedOptions([value])
    }
  }

  const handleSubmit = () => {
    const parts: string[] = []
    if (selectedOptions.length > 0) {
      parts.push(selectedOptions.join(', '))
    }
    if (allowFreeform && freeText.trim()) {
      if (parts.length > 0) parts.push('')
      parts.push(freeText.trim())
    }
    const answer = parts.join('\n')
    if (!answer.trim()) return
    onRespond(answer)
  }

  const canSubmit = selectedOptions.length > 0 || (allowFreeform && freeText.trim().length > 0)

  return (
    <div className="ask-user-form">
      {options.length > 0 && (
        <div className="ask-user-options">
          {options.map((opt) => {
            const value = opt.value ?? opt.label
            const checked = selectedOptions.includes(value)
            return (
              <label key={value} className={`ask-user-option ${allowMultiple ? 'checkbox' : 'radio'}`}>
                <input
                  type={allowMultiple ? 'checkbox' : 'radio'}
                  checked={checked}
                  onChange={() => handleToggleOption(value)}
                />
                <span className="ask-user-option-label">{opt.label}</span>
              </label>
            )
          })}
        </div>
      )}
      {allowFreeform && (
        <div className="ask-user-freetext">
          <label>{freeTextLabel}</label>
          <textarea
            value={freeText}
            onChange={(e) => setFreeText(e.target.value)}
            placeholder={freeTextPlaceholder}
            rows={2}
          />
        </div>
      )}
      <button
        className="ask-user-submit"
        onClick={handleSubmit}
        disabled={!canSubmit}
      >{tr("Send answer")}</button>
    </div>
  )
}

function getStatusDotColor(status: LiveToolCallStatus): string {
  switch (status) {
    case 'requested':
      return 'var(--info)'
    case 'running':
      return 'var(--accent)'
    case 'approval':
      return 'var(--warning)'
    case 'waiting_input':
      return 'var(--info)'
    case 'completed':
      return 'var(--success)'
    case 'failed':
      return 'var(--danger)'
  }
}

function LiveToolCalls({ calls, onAskUserRespond }: {
  calls?: LiveToolCall[]
  onAskUserRespond?: (callId: string, answer: string) => void
}) {
  if (!Array.isArray(calls) || calls.length === 0) return null

  return (
    <div className="live-tool-call-list" aria-label={tr("Live Tool Calls")}>
      {calls.map((call) => {
        const inputPreview = formatToolPayload(call.input)
        const resultPreview = formatToolPayload(call.error ?? call.result)
        const isAskUserWaiting = call.status === 'waiting_input' && call.toolName === 'AskUser'
        const isActive = call.status === 'requested' || call.status === 'running' || call.status === 'approval' || call.status === 'waiting_input'

        return (
          <details
            key={call.id}
            className={`live-tool-call ${call.status}`}
            open={isActive}
          >
            <summary className="live-tool-call-header">
              <span
                className="live-tool-call-dot"
                aria-hidden="true"
                style={{ backgroundColor: getStatusDotColor(call.status) }}
              />
              <span className="live-tool-call-name">
                {call.toolName}
              </span>
              <span className="live-tool-call-status">{getToolStatusLabel(call.status)}</span>
            </summary>
            <div className="live-tool-call-body">
              {isAskUserWaiting && onAskUserRespond ? (
                <AskUserForm call={call} onRespond={(answer) => onAskUserRespond(call.id, answer)} />
              ) : (
                <>
                  {inputPreview && (
                    <div className="live-tool-call-section">
                      <div className="live-tool-call-section-label">{tr("Input")}</div>
                      <pre>{inputPreview}</pre>
                    </div>
                  )}
                  {resultPreview && (
                    <div className="live-tool-call-section">
                      <div className="live-tool-call-section-label">{call.error ? 'Error' : 'Result'}</div>
                      <pre>{resultPreview}</pre>
                    </div>
                  )}
                </>
              )}
            </div>
          </details>
        )
      })}
    </div>
  )
}

export default function ChatView() {
  const ollama = useConfigStore((s) => s.ollama)
  const availableModels = useConfigStore((s) => s.availableModels)
  const verboseMode = useConfigStore((s) => s.preferences.verboseMode)
  const limitThinkingWindow = useConfigStore((s) => s.preferences.limitThinkingWindow)
  const superVerboseAuditLogging = useConfigStore((s) => s.preferences.superVerboseAuditLogging)
  const workspaceDefaultPath = useConfigStore((s) => s.preferences.workspaceDefaultPath)
  const setOllama = useConfigStore((s) => s.setOllama)
  const setAvailableModels = useConfigStore((s) => s.setAvailableModels)
  const llmProfiles = useConfigStore((s) => s.llmProfiles)
  const defaultLlmProfileIds = useConfigStore((s) => s.defaultLlmProfileIds)
  const llmProfileModels = useConfigStore((s) => s.llmProfileModels)
  const activeProvider = useEngineStore((s) => s.activeProvider)
  const engineSendMessage = useEngineStore((s) => s.sendMessage)
  const resolveEngineApproval = useEngineStore((s) => s.resolveApproval)
  const liveThinkingText = useEngineStore((s) => s.thinkingText)
  const liveThinkingThreadId = useEngineStore((s) => s.conversationThreadId)
  const {
    activeThreadId,
    pendingApproval,
    busy,
    error,
    addMessage,
    updateMessage,
    setThreadProviderSettings,
    setThreadPermissionConfig,
    setPendingApproval,
    clearApproval,
    setBusy,
    setError,
  } = useChatStore()

  const { createTask, updateTaskStatus, setTaskSteps } = useTaskStore()
  const addLog = useLogStore((s) => s.addLog)
  const activeThread = useChatStore(getActiveThread)
  const inputRef = useRef<HTMLTextAreaElement>(null)
  const logRef = useRef<HTMLDivElement>(null)
  const activeMessages = Array.isArray(activeThread?.messages) ? activeThread.messages : []
  const lastActiveMessage = activeMessages[activeMessages.length - 1]
  const providerContext = useMemo(
    () => ({
      ollama,
      availableModels,
      llmProfiles,
      defaultLlmProfileIds,
      llmProfileModels,
    }),
    [availableModels, defaultLlmProfileIds, llmProfileModels, llmProfiles, ollama],
  )
  const providerState = useMemo(
    () => getChatProviderState(providerContext, activeProvider, activeThread?.providerSettings),
    [activeProvider, activeThread?.providerSettings, providerContext],
  )
  const selectableModels = providerState.selectableModels
  const approvalSteps = Array.isArray(pendingApproval) ? pendingApproval : []
  const visibleMessages = activeMessages.filter((message) => message.role !== 'system' || message.visibleInChat)
  const modelCapabilities = providerState.provider === 'ollama'
    ? detectModelCapabilities(providerState.model)
    : null
  const selectedModelAvailable = selectableModels.length === 0 || selectableModels.includes(providerState.model)
  const [attachments, setAttachments] = useState<ChatAttachment[]>([])
  const [attachmentNotice, setAttachmentNotice] = useState<string | null>(null)
  const [modelNotice, setModelNotice] = useState<string | null>(null)
  const [slashSuggestions, setSlashSuggestions] = useState<string[]>([])

  const personalities = usePersonalityStore((s) => s.personalities)
  const activePersonalityId = usePersonalityStore((s) => s.activeId)
  const globalInstruction = useCoworkStore((s) => s.globalInstruction)
  const memoryHints = useMemoryStore((s) => s.hints)
  const registryCommands = useCommandRegistry((s) => s.commands)
  const executeSlashCommand = useCommandRegistry((s) => s.executeCommand)

  useEffect(() => {
    const node = logRef.current
    if (!node) return

    const frame = window.requestAnimationFrame(() => {
      node.scrollTop = node.scrollHeight
    })

    return () => window.cancelAnimationFrame(frame)
  }, [
    activeThreadId,
    activeMessages.length,
    lastActiveMessage?.id,
    lastActiveMessage?.streaming,
    lastActiveMessage?.content,
    lastActiveMessage?.thinkingContent,
    lastActiveMessage?.verboseContent,
    lastActiveMessage?.crewLive?.updatedAt,
  ])

  useEffect(() => {
    if (providerState.provider !== 'ollama') {
      setModelNotice(null)
      return
    }

    let cancelled = false

    const refreshModels = async () => {
      try {
        const health = await safeInvoke<{
          ok: boolean
          models: string[]
        }>('ollama_health_check', { config: ollama }, { ok: false, models: [] })

        if (cancelled) return

        const models = Array.isArray(health.models) ? health.models : []
        setAvailableModels(models)

        if (!health.ok) {
          setModelNotice(`Ollama endpoint is currently unreachable: ${ollama.baseUrl}`)
          return
        }

        if (activeThreadId && models.length > 0 && !models.includes(providerState.model)) {
          const fallbackModel = models[0]
          setThreadProviderSettings(activeThreadId, {
            ...createChatProviderSelection(providerState),
            model: fallbackModel,
          })
          setModelNotice(`Model ${providerState.model} is not available at ${ollama.baseUrl} Switching to ${fallbackModel} for this chat.`)
          return
        }

        setModelNotice(null)
      } catch {
        if (!cancelled) {
          setModelNotice(`Model list for ${ollama.baseUrl} could not be refreshed.`)
        }
      }
    }

    void refreshModels()

    return () => {
      cancelled = true
    }
  }, [activeThreadId, providerState, ollama, setAvailableModels, setThreadProviderSettings])

  const handleInputChange = (value: string) => {
    const matches = getSlashCommandSuggestions(registryCommands, value).map((command) => command.command)
    setSlashSuggestions(matches)
  }

  const handleAskUserRespond = useCallback((callId: string, answer: string) => {
    if (!activeThreadId || !answer.trim()) return

    // Mark the AskUser tool call as completed
    const assistantMsg = activeMessages.find((m) =>
      m.role === 'assistant' &&
      m.liveToolCalls?.some((call) => call.id === callId && call.status === 'waiting_input'),
    )
    if (assistantMsg) {
      updateMessage(activeThreadId, assistantMsg.id, {
        liveToolCalls: assistantMsg.liveToolCalls?.map((call) =>
          call.id === callId
            ? { ...call, status: 'completed', result: answer }
            : call,
        ),
      })
    }

    // Add user answer as a new message
    addMessage(activeThreadId, {
      role: 'user',
      content: answer,
      timestamp: Date.now(),
    })

    // Continue the conversation by sending the answer to the engine
    const answerInput = answer.trim()
    const cwd = getEffectiveChatCwd(attachments, workspaceDefaultPath)

    setBusy(true)
    setError(null)

    let rawAssistantMessage = ''
    let rawThinkingMessage = ''
    let engineErrorMessage = ''
    const usedToolNames = new Set<string>()
    let liveToolCalls: LiveToolCall[] = []

    const createdAssistantMessageId = addMessage(activeThreadId, {
      role: 'assistant',
      content: '',
      timestamp: Date.now(),
      streaming: true,
    })

    const updateLiveToolCall = (patch: LiveToolCallPatch) => {
      liveToolCalls = upsertLiveToolCall(liveToolCalls, patch)
      updateMessage(activeThreadId, createdAssistantMessageId, {
        liveToolCalls,
      })
    }

    void engineSendMessage(
      answerInput,
      cwd,
      (event) => {
        switch (event.type) {
          case 'text_delta': {
          rawAssistantMessage += event.text
          const presentation = resolveAssistantPresentation(rawAssistantMessage, {
            verboseMode,
            thinkingContent: rawThinkingMessage,
          })
          updateMessage(activeThreadId, createdAssistantMessageId, {
            content: presentation.content,
            thinkingContent: presentation.thinkingContent,
          })
          break
        }
        case 'thinking_delta':
          rawThinkingMessage += event.thinking
          updateMessage(activeThreadId, createdAssistantMessageId, {
            thinkingContent: rawThinkingMessage,
          })
          break
        case 'assistant_message': {
          const blocks = Array.isArray(event.message.content) ? event.message.content : []
          const textFromEvent = blocks
            .filter((block): block is { type: 'text'; text: string } => block.type === 'text')
            .map((block) => block.text)
            .join('\n')
            .trim()
          const thinkingFromEvent = blocks
            .filter((block): block is { type: 'thinking'; thinking: string } => block.type === 'thinking')
            .map((block) => block.thinking)
            .join('\n\n')
            .trim()
          const toolUseBlocks = blocks
            .filter((block): block is { type: 'tool_use'; id: string; name: string; input: Record<string, unknown> } =>
              block.type === 'tool_use'
              && typeof block.id === 'string'
              && typeof block.name === 'string'
              && typeof block.input === 'object'
              && block.input !== null,
            )

          if (!rawAssistantMessage && textFromEvent) {
            rawAssistantMessage = textFromEvent
          }
          if (!rawThinkingMessage && thinkingFromEvent) {
            rawThinkingMessage = thinkingFromEvent
          }
          for (const block of toolUseBlocks) {
            updateLiveToolCall({
              id: block.id,
              toolName: block.name,
              input: block.input,
              status: 'requested',
            })
          }

          const presentation = resolveAssistantPresentation(rawAssistantMessage, {
            verboseMode,
            thinkingContent: rawThinkingMessage,
          })
          updateMessage(activeThreadId, createdAssistantMessageId, {
            content: presentation.content,
            thinkingContent: presentation.thinkingContent,
          })
          break
        }
        case 'tool_call_delta':
          updateLiveToolCall({
            id: event.toolUseId,
            toolName: event.toolName,
            input: event.input,
            status: 'requested',
          })
          break
        case 'tool_use_start':
          usedToolNames.add(event.toolName)
          updateLiveToolCall({
            id: event.toolUseId,
            toolName: event.toolName,
            input: event.input,
            status: 'running',
          })
          break
        case 'tool_use_complete':
          usedToolNames.add(event.toolName)
          {
            const toolFailed = event.result.trim().toLowerCase().startsWith('fehler:')
            const nextStatus: LiveToolCallStatus = toolFailed
              ? 'failed'
              : event.toolName === 'AskUser'
                ? 'waiting_input'
                : 'completed'
            updateLiveToolCall({
              id: event.toolUseId,
              toolName: event.toolName,
              input: liveToolCalls.find((call) => call.id === event.toolUseId)?.input ?? {},
              status: nextStatus,
              result: event.result,
              error: toolFailed ? event.result : undefined,
              finishedAt: Date.now(),
            })
            if (event.toolName === 'AskUser' && !toolFailed) {
              setBusy(false)
            }
          }
          break
        case 'error':
          engineErrorMessage = event.error
          break
      }
    }, {
      threadId: activeThreadId,
      messages: activeMessages.map((message) => ({
        role: message.role,
        content: typeof message.content === 'string' ? message.content : '',
        debugContent: message.debugContent,
      })),
    }, createChatProviderSelection(providerState))
      .then(() => {
        const fallbackText = engineErrorMessage
          ? `LLM request failed: ${engineErrorMessage}\n\n${getChatProviderFailureHint(providerState.provider)}`
          : usedToolNames.size > 0
            ? `The engine used tools (${Array.from(usedToolNames).join(', ')}), but no visible final text provided.`
            : 'The model did not provide a visible response. Please try again or check the model/prompt.'
        const presentation = resolveAssistantPresentation(rawAssistantMessage, {
          verboseMode,
          thinkingContent: rawThinkingMessage,
          fallbackText,
        })
        updateMessage(activeThreadId, createdAssistantMessageId, {
          content: presentation.content,
          debugContent: presentation.debugContent,
          thinkingContent: presentation.thinkingContent,
          streaming: false,
        }, {
          persist: true,
        })
        setBusy(false)
      })
      .catch((err) => {
        const msg = err instanceof Error ? err.message : String(err)
        setError(msg)
        setBusy(false)
      })
  }, [activeThreadId, activeMessages, updateMessage, addMessage, engineSendMessage, attachments, workspaceDefaultPath, providerState, verboseMode, setBusy, setError])

  const handleSend = async (event: FormEvent) => {
    event.preventDefault()
    const text = inputRef.current?.value?.trim() ?? ''
    const hasAttachments = attachments.length > 0
    if ((!text && !hasAttachments) || busy || !activeThreadId) return
    const fallbackAttachmentPrompt = 'Please analyze the attached images or files and complete the task.'
    const effectiveInput = text || fallbackAttachmentPrompt

    // Slash command detection: if input starts with / and matches a registered command
    if (text.startsWith('/')) {
      const parts = text.split(/\s+/)
      const cmdName = parts[0]
      const cmdArgs = parts.slice(1).join(' ')
      const matchedCmd = registryCommands.find(c => c.command === cmdName)
      if (matchedCmd) {
        addMessage(activeThreadId, {
          role: 'user',
          content: text,
          timestamp: Date.now(),
        })
        addMessage(activeThreadId, {
          role: 'assistant',
          content: `⚡ Command ${cmdName} executed: ${matchedCmd.label}`,
          timestamp: Date.now(),
        })
        executeSlashCommand(matchedCmd.id, cmdArgs || undefined)
        if (inputRef.current) inputRef.current.value = ''
        setSlashSuggestions([])
        return
      }
    }
    setSlashSuggestions([])

    const mergedForSend = mergeAttachments([], attachments)
    const userMessage = {
      role: 'user' as const,
      content: text,
      timestamp: Date.now(),
      attachments: mergedForSend.next,
    }
    const userMessageId = addMessage(activeThreadId, userMessage)
    if (inputRef.current) inputRef.current.value = ''
    setAttachments([])
    setAttachmentNotice(null)
    setBusy(true)
    setError(null)

    const effectiveTimeoutMs = providerState.provider === 'ollama'
      ? Math.max(ollama.timeoutMs, 600000)
      : providerState.timeoutMs
    if (providerState.provider === 'ollama' && effectiveTimeoutMs !== ollama.timeoutMs) {
      setOllama({ timeoutMs: effectiveTimeoutMs })
    }

    let assistantMessageId: string | null = null
    let requestPreviewMessageId: string | null = null
    let promptWithAttachments = effectiveInput
    let attachmentBuild = {
      context: '',
      parsedFiles: 0,
      failedFiles: [] as Array<{ path: string; error: string }>,
    }

    try {
      await waitForNextPaint()

      attachmentBuild = await buildAttachmentPromptContext(mergedForSend.next, effectiveInput)
      const attachmentContext = attachmentBuild.context
      promptWithAttachments = attachmentContext ? `${effectiveInput}\n\n${attachmentContext}` : effectiveInput
      const engineUserInput = await buildEngineUserInput(promptWithAttachments, mergedForSend.next)
      const history = activeMessages
        .filter((m) => m.role !== 'system')
        .map((m) => ({ role: m.role, content: typeof m.content === 'string' ? m.content : '' }))
        .filter((m) => m.content.trim().length > 0 || m.role === 'user')

      const compactedHistory = compactHistoryForPrompt(history, 12)

      // Inject system prompt from active personality
      const activePersonality = personalities.find(p => p.id === activePersonalityId)
        ?? personalities.find(p => p.is_default)
        ?? personalities[0]
      if (activePersonality) {
        const systemPrompt = buildSystemPromptFromPersonality(activePersonality, globalInstruction, memoryHints)
        if (systemPrompt) {
          compactedHistory.compacted.unshift({ role: 'system', content: systemPrompt })
        }
      }

      updateMessage(activeThreadId, userMessageId, {
        debugContent: promptWithAttachments,
      }, {
        persist: true,
      })

      if (superVerboseAuditLogging) {
        void writeAuditEvent('super_verbose', 'chat_user_prompt', {
          view: 'chat',
          threadId: activeThreadId,
          prompt: text,
          promptWithAttachments,
          attachments: mergedForSend.next,
          history,
        })
      }

      const started = Date.now()
      addLog({
        level: 'info',
        area: 'llm',
        message: 'LLM request started',
        details: {
          provider: providerState.provider,
          endpoint: providerState.endpoint,
          model: providerState.model,
          timeoutMs: effectiveTimeoutMs,
          historyItems: history.length,
          compactedHistoryItems: compactedHistory.compacted.length,
          compactedDroppedItems: compactedHistory.droppedCount,
          promptChars: promptWithAttachments.length,
          parsedAttachments: attachmentBuild.parsedFiles,
          failedAttachments: attachmentBuild.failedFiles.length,
          source: 'chat',
        },
      })
      if (superVerboseAuditLogging) {
        void writeAuditEvent('super_verbose', 'chat_llm_request_started', {
          view: 'chat',
          threadId: activeThreadId,
          prompt: text,
          promptWithAttachments,
          history,
          provider: providerState.provider,
          endpoint: providerState.endpoint,
          model: providerState.model,
        })
      }
      if (attachmentBuild.failedFiles.length > 0) {
        addLog({
          level: 'warn',
          area: 'file_safety',
          message: 'Attachment analysis partially failed',
          details: {
            failures: attachmentBuild.failedFiles,
          },
        })
      }
      let rawAssistantMessage = ''
      let rawThinkingMessage = ''
      let engineErrorMessage = ''
      let approvalSummary = ''
      let approvalTaskCreated = false
      let webSearchSources: WebSearchSource[] = []
      const usedToolNames = new Set<string>()
      let liveToolCalls: LiveToolCall[] = []
      const createdAssistantMessageId = addMessage(activeThreadId, {
        role: 'assistant',
        content: '',
        timestamp: Date.now(),
        streaming: true,
      })
      assistantMessageId = createdAssistantMessageId

      const cwd = getEffectiveChatCwd(mergedForSend.next, workspaceDefaultPath)
      const updateLiveToolCall = (patch: LiveToolCallPatch) => {
        liveToolCalls = upsertLiveToolCall(liveToolCalls, patch)
        updateMessage(activeThreadId, createdAssistantMessageId, {
          liveToolCalls,
        })
      }

      await engineSendMessage(engineUserInput, cwd, (event) => {
        switch (event.type) {
          case 'text_delta': {
            rawAssistantMessage += event.text
            const presentation = resolveAssistantPresentation(rawAssistantMessage, {
              verboseMode,
              thinkingContent: rawThinkingMessage,
            })
            updateMessage(activeThreadId, createdAssistantMessageId, {
              content: presentation.content,
              thinkingContent: presentation.thinkingContent,
            })
            break
          }
          case 'thinking_delta':
            rawThinkingMessage += event.thinking
            updateMessage(activeThreadId, createdAssistantMessageId, {
              thinkingContent: rawThinkingMessage,
            })
            break
          case 'request_debug':
            if (userMessageId) {
              updateMessage(activeThreadId, userMessageId, {
                debugContent: `${promptWithAttachments}\n\n[OLLAMA REQUEST PREVIEW]\n${event.payload}`,
              })
            }
            if (requestPreviewMessageId) {
              updateMessage(activeThreadId, requestPreviewMessageId, {
                content: `Ollama Request Preview\n${event.payload}`,
              })
            } else {
              requestPreviewMessageId = addMessage(activeThreadId, {
                role: 'system',
                content: `Ollama Request Preview\n${event.payload}`,
                visibleInChat: true,
                timestamp: Date.now(),
              })
            }
            break
          case 'assistant_message': {
            const blocks = Array.isArray(event.message.content) ? event.message.content : []
            const textFromEvent = blocks
              .filter((block): block is { type: 'text'; text: string } => block.type === 'text')
              .map((block) => block.text)
              .join('\n')
              .trim()
            const thinkingFromEvent = blocks
              .filter((block): block is { type: 'thinking'; thinking: string } => block.type === 'thinking')
              .map((block) => block.thinking)
              .join('\n\n')
              .trim()
            const toolUseBlocks = blocks
              .filter((block): block is { type: 'tool_use'; id: string; name: string; input: Record<string, unknown> } =>
                block.type === 'tool_use'
                && typeof block.id === 'string'
                && typeof block.name === 'string'
                && typeof block.input === 'object'
                && block.input !== null,
              )

            if (!rawAssistantMessage && textFromEvent) {
              rawAssistantMessage = textFromEvent
            }
            if (!rawThinkingMessage && thinkingFromEvent) {
              rawThinkingMessage = thinkingFromEvent
            }
            for (const block of toolUseBlocks) {
              updateLiveToolCall({
                id: block.id,
                toolName: block.name,
                input: block.input,
                status: 'requested',
              })
            }

            const presentation = resolveAssistantPresentation(rawAssistantMessage, {
              verboseMode,
              thinkingContent: rawThinkingMessage,
            })
            updateMessage(activeThreadId, createdAssistantMessageId, {
              content: presentation.content,
              thinkingContent: presentation.thinkingContent,
            })
            break
          }
          case 'tool_call_delta':
            updateLiveToolCall({
              id: event.toolUseId,
              toolName: event.toolName,
              input: event.input,
              status: 'requested',
            })
            break
          case 'approval_required': {
            approvalSummary = `${event.request.toolName}: ${event.request.description}`
            setPendingApproval([approvalSummary])
            setBusy(false)
            updateLiveToolCall({
              id: findLiveToolCallId(liveToolCalls, event.request.toolName, event.request.input),
              toolName: event.request.toolName,
              input: event.request.input,
              status: 'approval',
              result: event.request.description,
            })

            if (!approvalTaskCreated) {
              const taskId = createTask(text, text.slice(0, 60), activeThreadId)
              const steps: TaskStep[] = [{
                id: `${taskId}-step-0`,
                index: 0,
                title: approvalSummary,
                state: 'pending',
                requiresApproval: true,
                riskLevel: event.request.riskLevel,
                output: null,
              }]
              setTaskSteps(taskId, steps)
              updateTaskStatus(taskId, 'waiting_approval')
              approvalTaskCreated = true
            }

            addLog({
              level: 'warn',
              area: 'llm',
              message: `Approval required: ${event.request.toolName}`,
              details: event.request,
            })
            break
          }
          case 'tool_use_start':
            usedToolNames.add(event.toolName)
            updateLiveToolCall({
              id: event.toolUseId,
              toolName: event.toolName,
              input: event.input,
              status: 'running',
            })
            addLog({
              level: 'info',
              area: 'llm',
              message: `Tool started: ${event.toolName}`,
              details: { toolName: event.toolName, input: event.input },
            })
            break
          case 'tool_use_complete':
            usedToolNames.add(event.toolName)
            {
              const toolFailed = event.result.trim().toLowerCase().startsWith('fehler:')
              const nextStatus: LiveToolCallStatus = toolFailed
                ? 'failed'
                : event.toolName === 'AskUser'
                  ? 'waiting_input'
                  : 'completed'
              updateLiveToolCall({
                id: event.toolUseId,
                toolName: event.toolName,
                input: liveToolCalls.find((call) => call.id === event.toolUseId)?.input ?? {},
                status: nextStatus,
                result: event.result,
                error: toolFailed ? event.result : undefined,
                finishedAt: Date.now(),
              })
              // Allow user to respond when AskUser is waiting for input
              if (event.toolName === 'AskUser' && !toolFailed) {
                setBusy(false)
              }
            }
            if (event.toolName === 'WebSearch') {
              webSearchSources = mergeWebSearchSources(
                webSearchSources,
                parseWebSearchSourcesFromToolResult(event.result),
              )
            }
            addLog({
              level: 'info',
              area: 'llm',
              message: `Tool fertig: ${event.toolName}`,
              details: { toolName: event.toolName, result: event.result.slice(0, 500) },
            })
            break
          case 'error':
            engineErrorMessage = event.error
            addLog({ level: 'error', area: 'llm', message: event.error })
            break
        }
      }, {
        threadId: activeThreadId,
        messages: activeMessages.map((message) => ({
          role: message.role,
          content: typeof message.content === 'string' ? message.content : '',
          debugContent: message.debugContent,
        })),
      }, createChatProviderSelection(providerState))

      const fallbackText = engineErrorMessage
        ? `LLM request failed: ${engineErrorMessage}\n\n${getChatProviderFailureHint(providerState.provider)}`
        : approvalSummary
          ? `Approval required: ${approvalSummary}`
          : usedToolNames.size > 0
            ? `The engine used tools (${Array.from(usedToolNames).join(', ')}), but no visible final text provided.`
            : 'The model did not provide a visible response. Please try again or check the model/prompt.'
      const presentation = resolveAssistantPresentation(rawAssistantMessage, {
        verboseMode,
        thinkingContent: rawThinkingMessage,
        fallbackText,
      })
      const finalContent = appendWebSearchSources(presentation.content, webSearchSources)
      updateMessage(activeThreadId, createdAssistantMessageId, {
        content: finalContent,
        debugContent: presentation.debugContent,
        thinkingContent: presentation.thinkingContent,
        streaming: false,
      }, {
        persist: true,
      })
      addLog({
        level: 'info',
        area: 'llm',
        message: 'LLM request succeeded',
        details: {
          provider: providerState.provider,
          endpoint: providerState.endpoint,
          model: providerState.model,
          cwd,
          durationMs: Date.now() - started,
          responseChars: rawAssistantMessage.length,
          usedTools: Array.from(usedToolNames),
        },
      })
      if (superVerboseAuditLogging) {
        void writeAuditEvent('super_verbose', 'chat_llm_request_finished', {
          view: 'chat',
          threadId: activeThreadId,
          prompt: text,
          promptWithAttachments,
          assistantRawResponse: rawAssistantMessage,
          assistantVisibleResponse: presentation.content,
          provider: providerState.provider,
          endpoint: providerState.endpoint,
          model: providerState.model,
          cwd,
          usedTools: Array.from(usedToolNames),
          approvalSummary,
        })
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      setError(message)
      addLog({
        level: 'error',
        area: 'llm',
        message: 'LLM request failed',
        details: {
          provider: providerState.provider,
          endpoint: providerState.endpoint,
          model: providerState.model,
          timeoutMs: effectiveTimeoutMs,
          error: message,
          source: 'chat',
        },
      })
      if (superVerboseAuditLogging) {
        void writeAuditEvent('super_verbose', 'chat_llm_request_failed', {
          view: 'chat',
          threadId: activeThreadId,
          prompt: text,
          promptWithAttachments,
          error: message,
          provider: providerState.provider,
          endpoint: providerState.endpoint,
          model: providerState.model,
        })
      }
      const failureContent = `LLM request failed: ${message}\n\n${getChatProviderFailureHint(providerState.provider)}`
      if (assistantMessageId) {
        updateMessage(activeThreadId, assistantMessageId, { content: failureContent, streaming: false }, { persist: true })
      } else {
        addMessage(activeThreadId, {
          role: 'assistant',
          content: failureContent,
          timestamp: Date.now(),
        })
      }
    } finally {
      setBusy(false)
    }
  }

  const engineAbort = useEngineStore((s) => s.abort)

  const handleStop = () => {
    engineAbort()
    if (activeThreadId) {
      const streamingMessage = [...activeMessages].reverse().find((message) => message.role === 'assistant' && message.streaming)
      if (streamingMessage) {
        const content = streamingMessage.content?.trim()
          ? `${streamingMessage.content}\n\nGenerierung abgebrochen.`
          : 'Generierung abgebrochen.'
        updateMessage(activeThreadId, streamingMessage.id, {
          content,
          streaming: false,
        }, {
          persist: true,
        })
      }
    }
    setBusy(false)
    setError(null)
  }

  const handleApprove = () => {
    if (approvalSteps.length === 0 || !activeThreadId) return
    setBusy(true)
    resolveEngineApproval({ allowed: true })
    addMessage(activeThreadId, {
      role: 'system',
      content: `Plan freigegeben: ${approvalSteps.join(' | ')}`,
      timestamp: Date.now(),
    })
    clearApproval()
  }

  const handleReject = () => {
    if (approvalSteps.length === 0 || !activeThreadId) return
    resolveEngineApproval({ allowed: false, reason: 'Declined by user.' })
    addMessage(activeThreadId, {
      role: 'system',
      content: `Plan declined: ${approvalSteps.join(' | ')}`,
      timestamp: Date.now(),
    })
    clearApproval()
    setBusy(false)
  }

  const handleProviderChange = (provider: string) => {
    if (!activeThreadId) return
    const nextProvider = normalizeChatProvider(provider)
    const nextProviderState = getChatProviderState(providerContext, activeProvider, { provider: nextProvider })
    setThreadProviderSettings(activeThreadId, createChatProviderSelection(nextProviderState))
    setModelNotice(null)
    addLog({
      level: 'info',
      area: 'llm',
      message: 'Provider changed for this chat',
      details: {
        provider: nextProvider,
        label: CHAT_PROVIDER_LABELS[nextProvider],
      },
    })
  }

  const handleModelChange = (model: string) => {
    if (!activeThreadId) return

    setThreadProviderSettings(activeThreadId, {
      ...createChatProviderSelection(providerState),
      model,
    })

    addLog({
      level: 'info',
      area: 'llm',
      message: 'Model changed for this chat',
      details: {
        provider: providerState.provider,
        previousModel: providerState.model,
        nextModel: model,
        endpoint: providerState.endpoint,
      },
    })
  }

  const addNewAttachments = (newItems: ChatAttachment[]) => {
    if (newItems.length === 0) return
    setAttachments((prev) => {
      const merged = mergeAttachments(prev, newItems)
      if (merged.rejectedCount > 0) {
        setAttachmentNotice(tr("Maximal 25 verbundene Elemente pro Message erreicht."))
      } else {
        setAttachmentNotice(null)
      }
      return merged.next
    })
  }

  const handleAttachFiles = async () => {
    const selected = await open({
      directory: false,
      multiple: true,
      filters: [
        { name: 'Documents', extensions: ['pdf', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx', 'odt', 'ods', 'odp', 'txt', 'rtf', 'csv'] },
        { name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg'] },
        { name: 'All files', extensions: ['*'] },
      ],
    })
    const selectedPaths = normalizeDialogSelection(selected)
    addNewAttachments(selectedPaths.map((path) => ({ path, kind: 'file' })))
  }

  const handleInputPaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const imageFiles = Array.from(event.clipboardData.items)
      .filter((item) => item.type.startsWith('image/'))
      .map((item) => item.getAsFile())
      .filter((file): file is File => file !== null)

    if (imageFiles.length === 0) {
      return
    }

    event.preventDefault()
    void Promise.all(imageFiles.map((file) => createInlineImageAttachment(file)))
      .then((inlineAttachments) => {
        addNewAttachments(inlineAttachments)
        setAttachmentNotice(null)
      })
      .catch(() => {
        setAttachmentNotice(tr("Could not read image from clipboard."))
      })
  }

  const handleRemoveAttachment = (target: ChatAttachment) => {
    setAttachments((prev) => prev.filter((item) => !(item.path === target.path && item.kind === target.kind)))
    setAttachmentNotice(null)
  }

  // Permission config for this thread
  const [showPermissionConfig, setShowPermissionConfig] = useState(false)
  const [permissionMode, setPermissionMode] = useState<PermissionMode>(activeThread?.permissionConfig?.mode || 'default')
  const [allowedDirectories, setAllowedDirectories] = useState<string[]>(activeThread?.permissionConfig?.allowedDirectories || [])
  const [newDirectory, setNewDirectory] = useState('')

  const handleSavePermissionConfig = () => {
    if (!activeThreadId) return
    const config: PermissionConfig = { mode: permissionMode, allowedDirectories }
    setThreadPermissionConfig(activeThreadId, config)
    setShowPermissionConfig(false)
  }

  const handleAddDirectory = () => {
    if (newDirectory.trim() && !allowedDirectories.includes(newDirectory.trim())) {
      setAllowedDirectories([...allowedDirectories, newDirectory.trim()])
      setNewDirectory('')
    }
  }

  const handleRemoveDirectory = (dir: string) => {
    setAllowedDirectories(allowedDirectories.filter(d => d !== dir))
  }

  if (!activeThread) return null

  return (
    <div className="chat-view">
      {/* Permission Config Panel */}
      {showPermissionConfig && (
        <div className="permission-config-panel">
          <h3>{tr("Permission settings for this chat")}</h3>
          <div className="permission-config-row">
            <label>{tr("Mode:")}</label>
            <select value={permissionMode} onChange={(e) => setPermissionMode(e.target.value as PermissionMode)}>
              <option value="default">{tr("Standard")}</option>
              <option value="plan">{tr("Plan-Mode")}</option>
              <option value="bypass">{tr("Bypass (allow everything)")}</option>
              <option value="strict">{tr("Strict (ask everything)")}</option>
            </select>
          </div>
          <div className="permission-config-row">
            <label>{tr("Allowed directories:")}</label>
            <div className="directory-list">
              {allowedDirectories.map(dir => (
                <span key={dir} className="directory-chip">
                  {dir}
                  <button type="button" onClick={() => handleRemoveDirectory(dir)}>{tr("×")}</button>
                </span>
              ))}
            </div>
            <div className="directory-input">
              <input
                type="text"
                value={newDirectory}
                onChange={(e) => setNewDirectory(e.target.value)}
                placeholder={tr("Add new directory")}
              />
              <button type="button" onClick={handleAddDirectory}>{tr("Add")}</button>
            </div>
          </div>
          <div className="permission-config-actions">
            <button type="button" onClick={handleSavePermissionConfig}>{tr("Save")}</button>
            <button type="button" onClick={() => setShowPermissionConfig(false)}>{tr("Cancel")}</button>
          </div>
        </div>
      )}

      <button
        type="button"
        className="permission-config-toggle"
        onClick={() => setShowPermissionConfig(!showPermissionConfig)}
      >{tr("🔒 Permissions")}</button>

      <div className="chat-log" ref={logRef}>
        {visibleMessages.map((msg, index) => {
          const content = typeof msg.content === 'string' ? msg.content : ''
          const { promptDebug, ollamaRequestPreview } = splitPromptDebugContent(msg.debugContent)
          const attachmentsForMessage = Array.isArray(msg.attachments) ? msg.attachments : []
          const imageAttachments = attachmentsForMessage.filter((item) => item.kind === 'file' && isImageAttachment(item))
          const liveThinkingBelongsToThread = liveThinkingThreadId === activeThreadId
          const displayedThinkingContent = resolveDisplayedThinkingContent(
            msg.thinkingContent,
            liveThinkingBelongsToThread ? liveThinkingText : undefined,
            {
              streaming: msg.streaming,
              preferLive: liveThinkingBelongsToThread && msg.streaming && index === visibleMessages.length - 1,
            },
          )
          const displayedContent = resolveDisplayedAssistantContent(content, displayedThinkingContent)
          return (
          <div key={msg.id} className={`cowork-msg ${msg.role}${msg.crewLive ? ' crew-live-message' : ''}`}>
            <div className="msg-avatar">
              {msg.role === 'user' ? '👤' : msg.role === 'assistant' ? '✦' : '⚙️'}
            </div>
            <div className="msg-body">
              <div className="msg-role">
                {msg.role === 'user' ? 'Du' : msg.role === 'assistant' ? 'Open_Cowork' : 'System'}
              </div>
              {msg.crewLive ? (
                <CrewLiveMonitor live={msg.crewLive} />
              ) : (
                <div className="msg-content">
                  {displayedContent ? <HighlightedChatText content={displayedContent} /> : null}
                </div>
              )}
              <MessageThinking
                content={displayedThinkingContent}
                limitToRollingWindow={limitThinkingWindow}
                streaming={msg.streaming}
              />
              <LiveToolCalls
                calls={msg.liveToolCalls}
                onAskUserRespond={handleAskUserRespond}
              />
              {verboseMode && (
                <MessageVerbose
                  content={msg.verboseContent}
                  limitToRollingWindow={limitThinkingWindow}
                />
              )}
              {attachmentsForMessage.length > 0 && (
                <>
                  {imageAttachments.length > 0 && (
                    <div className="message-attachment-previews">
                      {imageAttachments.map((item) => (
                        <div key={`preview-${item.kind}-${item.path}`} className="message-attachment-preview" title={item.label ?? item.path}>
                          <img
                            src={getAttachmentPreviewSrcForAttachment(item)}
                            alt={getAttachmentDisplayName(item)}
                            className="message-attachment-image"
                            loading="lazy"
                          />
                        </div>
                      ))}
                    </div>
                  )}
                  <div className="message-attachments">
                    {attachmentsForMessage.map((item) => (
                      <span key={`${item.kind}-${item.path}`} className="message-attachment-chip" title={item.label ?? item.path}>
                        {item.kind === 'folder' ? '📁' : isImageAttachment(item) ? '🖼️' : '📄'} {getAttachmentDisplayName(item)}
                      </span>
                    ))}
                  </div>
                </>
              )}
              {ollamaRequestPreview && (
                <details className="message-debug">
                  <summary>{tr("Ollama Request Preview")}</summary>
                  <pre>{ollamaRequestPreview}</pre>
                </details>
              )}
              {verboseMode && promptDebug && promptDebug !== content && (
                <details className="message-debug">
                  <summary>{tr("Verbose: internal prompt")}</summary>
                  <pre>{promptDebug}</pre>
                </details>
              )}
            </div>
          </div>
          )
        })}
        {busy && !activeMessages.some((msg) => msg.streaming) && (
          <div className="cowork-msg assistant">
            <div className="msg-avatar">✦</div>
            <div className="msg-body">
              <div className="msg-role">{tr("Open_Cowork")}</div>
              <div className="msg-content typing">
                <span className="typing-dot" />
                <span className="typing-dot" />
                <span className="typing-dot" />
              </div>
            </div>
          </div>
        )}
      </div>

      {approvalSteps.length > 0 && (
        <div className="approval-banner">
          <div className="approval-header">
            <span className="approval-icon">⚠️</span>
            <span>{tr("These steps require your approval:")}</span>
          </div>
          <ol className="approval-steps">
            {approvalSteps.map((step, idx) => (
              <li key={`${step}-${idx}`}>{step}</li>
            ))}
          </ol>
          <div className="approval-actions">
            <button type="button" className="btn-approve" onClick={handleApprove} disabled={busy}>{tr("✓ Approve")}</button>
            <button type="button" className="btn-reject" onClick={handleReject} disabled={busy}>{tr("✗ Reject")}</button>
          </div>
        </div>
      )}

      {error && <p className="error cowork-error">{error}</p>}

      <form className="cowork-input" onSubmit={handleSend}>
        <div className="chat-input-toolbar">
          <label>{tr("Provider")}<select
              className="model-selector chat-model-selector"
              value={providerState.provider}
              onChange={(e) => handleProviderChange(e.target.value)}
              disabled={busy}
            >
              {CHAT_PROVIDER_OPTIONS.map((provider) => (
                <option key={provider} value={provider}>
                  {CHAT_PROVIDER_LABELS[provider]}
                </option>
              ))}
            </select>
          </label>
          <label>{tr("Model")}<select
              className="model-selector chat-model-selector"
              value={providerState.model}
              onChange={(e) => handleModelChange(e.target.value)}
              disabled={busy}
            >
              {selectableModels.length > 0 ? (
                selectableModels.map((model) => (
                  <option key={model} value={model}>
                    {model}
                  </option>
                ))
              ) : (
                <option value={providerState.model}>{providerState.model || 'no model set'}</option>
              )}
              {selectableModels.length > 0 && providerState.model && !selectableModels.includes(providerState.model) && (
                <option value={providerState.model}>{providerState.model}</option>
              )}
            </select>
            <p
              className="hint-text"
              style={{
                marginTop: 6,
                maxWidth: 460,
                color: (modelNotice || !selectedModelAvailable) ? 'var(--danger)' : 'var(--text-muted)',
              }}
            >
              {modelNotice
                ?? (providerState.provider === 'ollama' && modelCapabilities
                  ? `Endpoint: ${providerState.endpoint} | Modelfamilie: ${modelCapabilities.family} | ${modelCapabilities.supportsTools ? 'Tool-Calls enabled' : 'Tool-Calls disabled'}`
                  : `Endpoint: ${providerState.endpoint || 'not set'} | Provider: ${providerState.label}`)}
            </p>
          </label>
          <div className="attachment-actions">
            <button type="button" className="btn-attach" onClick={handleAttachFiles} disabled={busy}>{tr("File/Image")}</button>
          </div>
        </div>

        <div className="chat-input-main">
          {attachments.length > 0 && (
            <div className="attachment-list" aria-label={tr("Connected items")}>
              {attachments.map((item) => (
                  <span key={`${item.kind}-${item.path}`} className="attachment-chip" title={item.label ?? item.path}>
                  <span className="attachment-chip-label">
                      {item.kind === 'folder' ? 'Folder' : isImageAttachment(item) ? 'Image' : 'File'}: {getAttachmentDisplayName(item)}
                  </span>
                  <button
                    type="button"
                    className="attachment-remove"
                    onClick={() => handleRemoveAttachment(item)}
                      aria-label={`attachment entfernen: ${getAttachmentDisplayName(item)}`}
                    disabled={busy}
                  >{tr("×")}</button>
                </span>
              ))}
            </div>
          )}

          {attachmentNotice && <p className="attachment-notice">{attachmentNotice}</p>}

          {slashSuggestions.length > 0 && (
            <div style={{
              position: 'absolute', bottom: '100%', left: 0, right: 0,
              background: 'var(--bg-secondary)', border: '1px solid var(--border-color)',
              borderRadius: 'var(--radius-sm)', marginBottom: 4, maxHeight: 200, overflow: 'auto',
              zIndex: 10,
            }}>
              {slashSuggestions.map(cmd => {
                const full = registryCommands.find(c => c.command === cmd)
                return (
                  <button key={cmd} type="button" style={{
                    display: 'block', width: '100%', textAlign: 'left', padding: '6px 10px',
                    background: 'transparent', border: 'none', cursor: 'pointer',
                    color: 'var(--text-primary)', fontSize: 13,
                  }}
                  onClick={() => {
                    if (inputRef.current) inputRef.current.value = cmd + ' '
                    setSlashSuggestions([])
                    inputRef.current?.focus()
                  }}>
                    <span style={{ color: 'var(--accent)', fontFamily: 'monospace', marginRight: 8 }}>{cmd}</span>
                    <span style={{ color: 'var(--text-muted)', fontSize: 11 }}>{full?.label}</span>
                  </button>
                )
              })}
            </div>
          )}

          <textarea
            ref={inputRef}
            rows={2}
            placeholder={tr("Send message or /command...")}
            disabled={busy}
            onChange={(e) => handleInputChange(e.target.value)}
            onPaste={handleInputPaste}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault()
                handleSend(e)
              }
            }}
          />
        </div>

        {busy ? (
          <button type="button" onClick={handleStop} className="btn-stop">{tr("⏹ Stopp")}</button>
        ) : (
          <button type="submit" disabled={busy} className="btn-send">{tr("Send")}{'->'}
          </button>
        )}
      </form>
    </div>
  )
}
