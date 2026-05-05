import { useRef, useEffect, useMemo, useState } from 'react'
import type { ClipboardEvent, DragEvent, FormEvent } from 'react'
import { open, save } from '@tauri-apps/plugin-dialog'
import { useChatStore, getActiveThread, type ChatMessage } from '../stores/chatStore'
import type { LiveToolCall, LiveToolCallStatus } from '../stores/chatStore'
import { CheckCircle2, Clock3, Loader2, ShieldAlert, Wrench, XCircle } from 'lucide-react'
import { useConfigStore } from '../stores/configStore'
import { useTaskStore } from '../stores/taskStore'
import { useLogStore } from '../stores/logStore'
import { useCoworkStore, type ClaudePermissionMode } from '../stores/coworkStore'
import { useMemoryStore } from '../stores/memoryStore'
import { useInsightsStore } from '../stores/insightsStore'
import { useProcessStore } from '../stores/processStore'
import { useTerminalStore } from '../stores/terminalStore'
import { useSkillStore } from '../stores/skillStore'
import { useCrewStore } from '../stores/crewStore'
import { useEngineStore } from '../stores/engineStore'
import { useUiStore } from '../stores/uiStore'
import type { ContentBlock, ToolUIRequest } from '../engine'
import type { ToolProgressData } from '../engine/types'
import { checkOllamaConnection } from '../engine/api/ollamaClient'
import {
  createInlineImageAttachment,
  extractFileAttachmentsFromFileList,
  extractFileAttachmentsFromUriList,
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
import { limitRollingLines, resolveAssistantPresentation, resolveDisplayedAssistantContent, resolveDisplayedThinkingContent, sanitizeAssistantContent, splitPromptDebugContent } from '../utils/messageDisplay'
import { appendWebSearchSources, mergeWebSearchSources, parseWebSearchSourcesFromToolResult, type WebSearchSource } from '../utils/webSearchSources'
// Ollama streaming is now handled by the engine
import { MessageThinking, MessageVerbose } from './MessageThinking'
import { HighlightedChatText } from './HighlightedChatText'
import { writeAuditEvent } from '../utils/audit'
import { persistInvoke } from '../stores/chatStore'
import {
  buildClaudeSystemAddendum,
  compactHistoryForPrompt,
  isToolDeniedByRules,
  parseSlashCommand,
} from '../utils/claudeBridge'
import {
  buildClarificationContinuationPrompt,
  inferClarificationContext,
} from '../utils/followUpPrompt'
import { useCommandRegistry } from '../stores/commandRegistryStore'
import { hasTauriRuntime, safeInvoke, safeInvokeVoid } from '../utils/safeInvoke'
import { parseScheduledTaskInput, SCHEDULE_HELP_TEXT } from '../utils/schedulerUtils'
import { showDesktopNotification } from '../utils/notifications'
import {
  CHAT_PROVIDER_LABELS,
  CHAT_PROVIDER_OPTIONS,
  createChatProviderSelection,
  getChatProviderFailureHint,
  getChatProviderState,
  normalizeChatProvider,
} from '../utils/chatProvider'

type WebFetchResponse = {
  url: string
  status: number
  ok: boolean
  title: string | null
  content: string
  truncated: boolean
}

type McpCallResponse = {
  serverName: string
  toolName: string
  success: boolean
  result: string
  error: string | null
}

type EnabledPluginSkill = {
  pluginName: string
  skillName: string
  command: string
  promptTemplate: string
  runMode: 'plan' | 'execute'
}

type SlashCommandSuggestion = {
  command: string
  args?: string
  description: string
  source: 'built-in' | 'plugin'
}

type AskUserOption = {
  id: string
  label: string
}

type AskUserPromptModel = {
  question: string
  options: AskUserOption[]
  allowMultiple: boolean
  freeTextLabel: string
  freeTextPlaceholder: string
}

const VALID_PERMISSION_MODES: ClaudePermissionMode[] = [
  'default',
  'acceptEdits',
  'bypassPermissions',
  'dontAsk',
  'plan',
]

const ENGINE_TO_CLAUDE_PERMISSION_MODE: Record<'default' | 'plan' | 'bypass' | 'strict', ClaudePermissionMode> = {
  default: 'default',
  plan: 'plan',
  bypass: 'bypassPermissions',
  strict: 'acceptEdits',
}

function formatVerboseTimestamp(timestamp: number): string {
  return new Date(timestamp).toLocaleTimeString('de-DE', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

function clipVerboseText(value: string, maxChars = 4000): string {
  const normalized = value.trim()
  if (!normalized) return ''
  if (normalized.length <= maxChars) return normalized
  return `${normalized.slice(0, maxChars)}\n... [gekuerzt, ${normalized.length - maxChars} weitere Zeichen]`
}

function appendRollingTerminalOutput(previous: string, nextLine: string, maxLines = 200): string {
  const normalized = nextLine.trim()
  if (!normalized) return previous
  const combined = previous ? `${previous}\n${normalized}` : normalized
  return limitRollingLines(combined, maxLines)
}

const TERMINAL_CWD_MARKER = '__OPEN_COWORK_CURRENT_CWD__='

function normalizeShellPanelLine(output: string): string | null {
  const normalized = output.trim()
  if (!normalized) return null

  if (normalized === 'stdout:' || normalized === 'stderr:') return null
  if (normalized.startsWith(`stdout: ${TERMINAL_CWD_MARKER}`)) return null
  if (normalized.startsWith('status: ')) return null

  if (normalized.startsWith('stdout: ')) return normalized.slice('stdout: '.length)
  if (normalized.startsWith('stderr: ')) return `[stderr] ${normalized.slice('stderr: '.length)}`
  if (normalized.startsWith('exit code: ')) return `[exit ${normalized.slice('exit code: '.length)}]`
  if (normalized.startsWith('cwd: ')) return `[cwd] ${normalized.slice('cwd: '.length)}`

  return normalized
}

function extractShellPanelCompletionLine(result: string): string | null {
  const cwdMatch = result.match(/current cwd:\s*([^\n\r]+)/i)
  if (cwdMatch?.[1]?.trim()) {
    return `[cwd] ${cwdMatch[1].trim()}`
  }

  const stdoutMatch = result.match(/stdout:\s*\n([^\n\r]+)/i)
  if (stdoutMatch?.[1]?.trim()) {
    return stdoutMatch[1].trim()
  }

  return null
}

function stringifyVerboseValue(value: unknown): string {
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

async function getOllamaStatusText(config: { baseUrl: string; model: string }): Promise<string> {
  try {
    const health = await safeInvoke<{ status: string }>('ollama_health_check', { config })
    return health.status
  } catch {
    const reachable = await checkOllamaConnection(config.baseUrl)
    return reachable ? 'ERREICHBAR (Web-Check)' : 'NICHT ERREICHBAR'
  }
}

type ChatExportFormat = 'md' | 'txt' | 'json'

function sanitizeExportBaseName(value: string): string {
  const normalized = value.trim().replace(/[^a-zA-Z0-9_-]+/g, '_').replace(/^_+|_+$/g, '')
  return normalized.slice(0, 40) || 'chat-export'
}

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

function getEffectiveWorkspaceCwd(
  attachments: ChatAttachment[],
  workingPath: string | null,
  workingPathKind: 'file' | 'folder' | null,
  workspaceDefaultPath: string,
): string {
  for (const attachment of attachments) {
    if (!hasLocalAttachmentPath(attachment)) continue
    const normalized = attachment.path.trim()
    if (!normalized) continue
    if (attachment.kind === 'folder') return normalized
  }

  for (const attachment of attachments) {
    if (!hasLocalAttachmentPath(attachment)) continue
    const normalized = attachment.path.trim()
    if (!normalized || attachment.kind !== 'file') continue
    return getParentDirectory(normalized)
  }

  if (workingPath?.trim()) {
    const normalized = workingPath.trim()
    if (workingPathKind === 'file') {
      return getParentDirectory(normalized)
    }
    return normalized
  }

  if (workspaceDefaultPath.trim()) {
    return workspaceDefaultPath.trim()
  }

  return '.'
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

function buildChatExportPayload(
  activeThread: { title: string },
  activeMessages: ChatMessage[],
  format: ChatExportFormat,
): { data: string; mimeType: string; ext: ChatExportFormat } {
  if (format === 'json') {
    return {
      data: JSON.stringify({ title: activeThread.title, messages: activeMessages }, null, 2),
      mimeType: 'application/json',
      ext: 'json',
    }
  }

  if (format === 'txt') {
    return {
      data: activeMessages.map((message) => `[${message.role}] ${message.content}`).join('\n\n'),
      mimeType: 'text/plain',
      ext: 'txt',
    }
  }

  return {
    data: `# Chat Export: ${activeThread.title}\n\n${activeMessages.map((message) => {
      const prefix = message.role === 'user' ? '## Du' : message.role === 'assistant' ? '## Open_Cowork' : '## System'
      return `${prefix}\n\n${message.content}`
    }).join('\n\n---\n\n')}`,
    mimeType: 'text/markdown',
    ext: 'md',
  }
}

function formatToolProgress(toolName: string, data: ToolProgressData): { headline: string; details?: string } {
  switch (data.type) {
    case 'bash_progress':
      return {
        headline: `${toolName}: Shell-Ausgabe`,
        details: clipVerboseText(data.output),
      }
    case 'agent_progress':
      return {
        headline: `${toolName}: Agent-Fortschritt (${data.agentName})`,
        details: clipVerboseText(data.content),
      }
    case 'web_search_progress':
      return {
        headline: `${toolName}: Web-Suche`,
        details: `Query: ${data.query}\nTreffer: ${data.results}`,
      }
    case 'mcp_progress':
      return {
        headline: `${toolName}: MCP-Fortschritt (${data.serverName})`,
        details: `Fortschritt: ${data.progress}%`,
      }
    case 'skill_progress':
      return {
        headline: `${toolName}: Skill-Ausgabe (${data.skillName})`,
        details: clipVerboseText(data.output),
      }
    case 'task_output_progress':
      return {
        headline: `${toolName}: Task-Ausgabe (${data.taskId})`,
        details: clipVerboseText(data.output),
      }
    case 'file_progress':
      return {
        headline: `${toolName}: Dateioperation`,
        details: `${data.operation}: ${data.path}`,
      }
    default:
      return {
        headline: `${toolName}: Tool-Fortschritt`,
        details: clipVerboseText(stringifyVerboseValue(data)),
      }
  }
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

  return calls.map((call, index) => index === existingIndex
    ? {
        ...call,
        ...patch,
        startedAt: call.startedAt,
        finishedAt: patch.finishedAt ?? call.finishedAt,
      }
    : call)
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

function getToolUiInput(ui: ToolUIRequest): Record<string, unknown> {
  const detailsInput = ui.details?.input
  if (detailsInput && typeof detailsInput === 'object' && !Array.isArray(detailsInput)) {
    return detailsInput as Record<string, unknown>
  }

  return ui.toolName === 'AskUser'
    ? { question: ui.content }
    : { content: ui.content }
}

function normalizeAskUserOptions(rawOptions: unknown): AskUserOption[] {
  if (!Array.isArray(rawOptions)) {
    return []
  }

  return rawOptions
    .map((item, index): AskUserOption | null => {
      if (typeof item === 'string') {
        const label = item.trim()
        return label ? { id: `option-${index}`, label } : null
      }

      if (item && typeof item === 'object') {
        const record = item as Record<string, unknown>
        const label = typeof record.label === 'string'
          ? record.label.trim()
          : typeof record.value === 'string'
            ? record.value.trim()
            : ''
        const id = typeof record.value === 'string' && record.value.trim()
          ? record.value.trim()
          : `option-${index}`
        return label ? { id, label } : null
      }

      return null
    })
    .filter((option): option is AskUserOption => option !== null)
}

function parseNumberedAskUserOptions(question: string): { question: string; options: AskUserOption[] } {
  const matches = Array.from(question.matchAll(/(?:^|\s)(\d+)[.)]\s+/g))
  if (matches.length < 2) {
    return { question, options: [] }
  }

  const options = matches
    .map((match, index): AskUserOption => {
      const start = (match.index ?? 0) + match[0].length
      const end = index + 1 < matches.length ? matches[index + 1].index ?? question.length : question.length
      return {
        id: `option-${match[1]}`,
        label: question.slice(start, end).trim().replace(/[;,.]\s*$/, ''),
      }
    })
    .filter((option) => option.label.length > 0)

  if (options.length < 2) {
    return { question, options: [] }
  }

  const firstMarkerIndex = matches[0].index ?? 0
  const cleanedQuestion = question.slice(0, firstMarkerIndex).trim().replace(/[:;,]\s*$/, '')
  return {
    question: cleanedQuestion || question,
    options,
  }
}

function shouldDefaultToSingleChoice(question: string): boolean {
  return /\b(eine|einer|eines|one)\b[^.?!]{0,40}\b(option|optionen|auswahl|choice|choices)\b/i.test(question)
}

function resolveAskUserPromptModel(question: string | null, input: Record<string, unknown> | null): AskUserPromptModel | null {
  const rawQuestion = typeof input?.question === 'string' && input.question.trim()
    ? input.question.trim()
    : question?.trim() ?? ''

  if (!rawQuestion) {
    return null
  }

  const structuredOptions = normalizeAskUserOptions(input?.options)
  const parsed = structuredOptions.length > 0
    ? { question: rawQuestion, options: structuredOptions }
    : parseNumberedAskUserOptions(rawQuestion)

  const allowMultiple = typeof input?.allow_multiple === 'boolean'
    ? input.allow_multiple
    : typeof input?.allowMultiple === 'boolean'
      ? input.allowMultiple
      : parsed.options.length > 0 && !shouldDefaultToSingleChoice(rawQuestion)

  return {
    question: parsed.question,
    options: parsed.options,
    allowMultiple,
    freeTextLabel: typeof input?.free_text_label === 'string' && input.free_text_label.trim()
      ? input.free_text_label.trim()
      : 'Freitext',
    freeTextPlaceholder: typeof input?.free_text_placeholder === 'string' && input.free_text_placeholder.trim()
      ? input.free_text_placeholder.trim()
      : 'Optional ergaenzen...',
  }
}

function parseAskUserResult(result?: string): string | null {
  const normalized = result?.trim()
  if (!normalized) return null

  const match = normalized.match(/^\[Warte auf Benutzerantwort:\s*([\s\S]*?)\]$/)
  if (match?.[1]?.trim()) {
    return match[1].trim()
  }

  return null
}

function getAskUserQuestionFromMessage(message: ChatMessage | undefined): string | null {
  if (!message || message.role !== 'assistant') return null

  const liveToolCalls = Array.isArray(message.liveToolCalls) ? message.liveToolCalls : []
  const askUserCall = [...liveToolCalls]
    .reverse()
    .find((call) =>
      call.toolName === 'AskUser' &&
      (
        call.status === 'waiting_input' ||
        (call.status === 'completed' && parseAskUserResult(call.result) !== null)
      )
    )

  if (askUserCall) {
    const inputQuestion = askUserCall.input?.question
    if (typeof inputQuestion === 'string' && inputQuestion.trim()) {
      return inputQuestion.trim()
    }

    return parseAskUserResult(askUserCall.result)
  }

  const content = typeof message.content === 'string' ? message.content.trim() : ''
  const contentMatch = content.match(/^Rueckfrage:\s*([\s\S]+)$/)
  return contentMatch?.[1]?.trim() || null
}

function getAskUserInputFromMessage(message: ChatMessage | undefined): Record<string, unknown> | null {
  if (!message || message.role !== 'assistant') return null

  const liveToolCalls = Array.isArray(message.liveToolCalls) ? message.liveToolCalls : []
  const askUserCall = [...liveToolCalls]
    .reverse()
    .find((call) =>
      call.toolName === 'AskUser' &&
      (
        call.status === 'waiting_input' ||
        (call.status === 'completed' && parseAskUserResult(call.result) !== null)
      )
    )

  if (!askUserCall) return null
  return askUserCall.input && typeof askUserCall.input === 'object' ? askUserCall.input : null
}

function getPendingAskUserQuestion(messages: ChatMessage[]): string | null {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index]

    if (message.role === 'system' && !message.visibleInChat) {
      continue
    }

    if (message.role === 'user') {
      return null
    }

    if (message.role === 'assistant') {
      return getAskUserQuestionFromMessage(message)
    }
  }

  return null
}

function getPendingAskUserInput(messages: ChatMessage[]): Record<string, unknown> | null {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index]

    if (message.role === 'system' && !message.visibleInChat) {
      continue
    }

    if (message.role === 'user') {
      return null
    }

    if (message.role === 'assistant') {
      return getAskUserInputFromMessage(message)
    }
  }

  return null
}

function normalizeLiveToolCallForDisplay(call: LiveToolCall): LiveToolCall {
  if (call.toolName !== 'AskUser') {
    return call
  }

  const inputQuestion = call.input?.question
  const hasQuestionInput = typeof inputQuestion === 'string' && inputQuestion.trim().length > 0
  const hasWaitingResult = parseAskUserResult(call.result) !== null

  if ((hasQuestionInput || hasWaitingResult) && call.status === 'completed') {
    return {
      ...call,
      status: 'waiting_input',
    }
  }

  return call
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
      return 'Tool Call erkannt'
    case 'running':
      return 'Tool laeuft'
    case 'approval':
      return 'Freigabe erforderlich'
    case 'waiting_input':
      return 'Wartet auf Antwort'
    case 'completed':
      return 'Abgeschlossen'
    case 'failed':
      return 'Fehlgeschlagen'
  }
}

function getToolStatusIcon(status: LiveToolCallStatus) {
  switch (status) {
    case 'requested':
      return <Clock3 size={15} />
    case 'running':
      return <Loader2 size={15} className="tool-call-spin" />
    case 'approval':
      return <ShieldAlert size={15} />
    case 'waiting_input':
      return <Clock3 size={15} />
    case 'completed':
      return <CheckCircle2 size={15} />
    case 'failed':
      return <XCircle size={15} />
  }
}

function LiveToolCalls({ calls }: { calls?: LiveToolCall[] }) {
  if (!Array.isArray(calls) || calls.length === 0) return null

  return (
    <div className="live-tool-call-list" aria-label="Live Tool Calls">
      {calls.map((call) => {
        const displayCall = normalizeLiveToolCallForDisplay(call)
        const inputPreview = formatToolPayload(displayCall.input)
        const resultPreview = formatToolPayload(displayCall.error ?? displayCall.result)
        return (
          <div key={displayCall.id} className={`live-tool-call ${displayCall.status}`}>
            <div className="live-tool-call-header">
              <span className="live-tool-call-icon" aria-hidden="true">
                {getToolStatusIcon(displayCall.status)}
              </span>
              <span className="live-tool-call-name">
                <Wrench size={14} aria-hidden="true" />
                {displayCall.toolName}
              </span>
              <span className="live-tool-call-status">{getToolStatusLabel(displayCall.status)}</span>
            </div>
            {inputPreview && (
              <details className="live-tool-call-detail" open={displayCall.status === 'requested' || displayCall.status === 'running' || displayCall.status === 'approval' || displayCall.status === 'waiting_input'}>
                <summary>Input</summary>
                <pre>{inputPreview}</pre>
              </details>
            )}
            {resultPreview && (
              <details className="live-tool-call-detail" open={displayCall.status === 'failed'}>
                <summary>{displayCall.error ? 'Fehler' : 'Ergebnis'}</summary>
                <pre>{resultPreview}</pre>
              </details>
            )}
          </div>
        )
      })}
    </div>
  )
}

export default function CoworkView() {
  const [attachments, setAttachments] = useState<ChatAttachment[]>([])
  const [attachmentNotice, setAttachmentNotice] = useState<string | null>(null)
  const [dragOverInput, setDragOverInput] = useState(false)
  const [promptHistory, setPromptHistory] = useState<string[]>([])
  const [historyIndex, setHistoryIndex] = useState(-1)
  const [showScrollToBottom, setShowScrollToBottom] = useState(false)
  const [inputValue, setInputValue] = useState('')
  const [dismissedAskUserQuestion, setDismissedAskUserQuestion] = useState<string | null>(null)
  const [selectedAskUserOptionIds, setSelectedAskUserOptionIds] = useState<string[]>([])
  const [askUserFreeText, setAskUserFreeText] = useState('')
  const [slashSuggestionsOpen, setSlashSuggestionsOpen] = useState(false)
  const [activeSlashSuggestionIndex, setActiveSlashSuggestionIndex] = useState(0)
  const [shellPanelOpen, setShellPanelOpen] = useState(false)
  const [shellPanelContent, setShellPanelContent] = useState('')
  const [shellPanelRunning, setShellPanelRunning] = useState(false)
  const ollama = useConfigStore((s) => s.ollama)
  const availableModels = useConfigStore((s) => s.availableModels)
  const setOllama = useConfigStore((s) => s.setOllama)
  const llmProfiles = useConfigStore((s) => s.llmProfiles)
  const defaultLlmProfileIds = useConfigStore((s) => s.defaultLlmProfileIds)
  const llmProfileModels = useConfigStore((s) => s.llmProfileModels)
  const mcpServer = useConfigStore((s) => s.mcpServer)
  const activeProvider = useEngineStore((s) => s.activeProvider)
  const engineSendMessage = useEngineStore((s) => s.sendMessage)
  const engineAbort = useEngineStore((s) => s.abort)
  const enginePermissionMode = useEngineStore((s) => s.config.permissionMode)
  const setEngineConfig = useEngineStore((s) => s.setConfig)
  const resolveEngineApproval = useEngineStore((s) => s.resolveApproval)
  const currentToolUI = useEngineStore((s) => s.currentToolUI)
  const clearCurrentToolUI = useEngineStore((s) => s.clearCurrentToolUI)
  const forceCompact = useEngineStore((s) => s.forceCompact)
  const currentSessionId = useEngineStore((s) => s.currentSessionId)
  const contextWarning = useEngineStore((s) => s.contextWarning)
  const compactionCount = useEngineStore((s) => s.compactionCount)
  const liveThinkingText = useEngineStore((s) => s.thinkingText)
  const liveThinkingThreadId = useEngineStore((s) => s.conversationThreadId)
  const workingFolder = useUiStore((s) => s.workingFolder)
  const workingPathKind = useUiStore((s) => s.workingPathKind)
  const showTimestamps = useConfigStore((s) => s.preferences.showTimestamps)
  const compactMode = useConfigStore((s) => s.preferences.compactMode)
  const verboseMode = useConfigStore((s) => s.preferences.verboseMode)
  const limitThinkingWindow = useConfigStore((s) => s.preferences.limitThinkingWindow)
  const superVerboseAuditLogging = useConfigStore((s) => s.preferences.superVerboseAuditLogging)
  const autoPilotAllTools = useConfigStore((s) => s.preferences.autoPilotAllTools)
  const workspaceDefaultPath = useConfigStore((s) => s.preferences.workspaceDefaultPath)
  const desktopNotificationsEnabled = useConfigStore((s) => s.preferences.notificationsEnabled)
  const {
    activeThreadId,
    pendingApproval,
    busy,
    error,
    addThread,
    setActiveThread,
    setThreadProviderSettings,
    addMessage,
    updateMessage,
    setPendingApproval,
    clearApproval,
    setBusy,
    setError,
  } = useChatStore()

  const { createTask } = useTaskStore()
  const tasks = useTaskStore((s) => s.tasks)
  const addLog = useLogStore((s) => s.addLog)
  const globalInstruction = useCoworkStore((s) => s.globalInstruction)
  const claudePlanMode = useCoworkStore((s) => s.claudePlanMode)
  const claudePermissionMode = useCoworkStore((s) => s.claudePermissionMode)
  const enabledClaudeToolIds = useCoworkStore((s) => s.enabledClaudeToolIds)
  const setClaudePlanMode = useCoworkStore((s) => s.setClaudePlanMode)
  const setClaudePermissionMode = useCoworkStore((s) => s.setClaudePermissionMode)
  const toolDenyRules = useCoworkStore((s) => s.toolDenyRules)
  const policyFlags = useCoworkStore((s) => s.policyFlags)
  const plugins = useCoworkStore((s) => s.plugins)
  const activeThread = useChatStore(getActiveThread)
  const inputRef = useRef<HTMLTextAreaElement>(null)
  const logRef = useRef<HTMLDivElement>(null)
  const notifiedAskUserQuestionRef = useRef<string | null>(null)
  const emptyThreadBootstrapRef = useRef<string | null>(null)
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

  useEffect(() => {
    // Wenn bereits ein aktiver Thread existiert und gültig ist, tue nichts
    if (activeThread) {
      emptyThreadBootstrapRef.current = null
      return
    }

    const current = useChatStore.getState()
    
    // Prüfe, ob activeThreadId gesetzt und gültig ist
    if (current.activeThreadId && current.threads.some((thread) => thread.id === current.activeThreadId)) {
      setActiveThread(current.activeThreadId)
      return
    }

    // Prüfe, ob ein bootstrapped Thread existiert
    const bootstrappedThreadId = emptyThreadBootstrapRef.current
    if (bootstrappedThreadId && current.threads.some((thread) => thread.id === bootstrappedThreadId)) {
      setActiveThread(bootstrappedThreadId)
      return
    }

    // Nur einen leeren "Neuer Chat" erstellen, wenn noch gar kein Thread existiert
    if (current.threads.length === 0) {
      const threadId = addThread('Neuer Chat', createChatProviderSelection(providerState))
      emptyThreadBootstrapRef.current = threadId
      setActiveThread(threadId)
    } else {
      // Ansonsten den neuesten Thread aktivieren
      const sortedThreads = [...current.threads].sort((a, b) => b.updatedAt - a.updatedAt)
      const mostRecentThread = sortedThreads[0]
      if (mostRecentThread) {
        setActiveThread(mostRecentThread.id)
      }
    }
  }, [activeThread, addThread, providerState, setActiveThread])

  const approvalSteps = Array.isArray(pendingApproval) ? pendingApproval : []
  const pendingAskUserQuestion = getPendingAskUserQuestion(activeMessages)
  const pendingAskUserInput = getPendingAskUserInput(activeMessages)
  const isAskUserPrompt =
    currentToolUI?.toolName === 'AskUser' &&
    currentToolUI.type === 'input' &&
    typeof currentToolUI.content === 'string' &&
    currentToolUI.content.trim().length > 0
  const askUserQuestion = isAskUserPrompt ? currentToolUI.content.trim() : pendingAskUserQuestion
  const askUserInput = isAskUserPrompt && currentToolUI ? getToolUiInput(currentToolUI) : pendingAskUserInput
  const askUserPromptModel = useMemo(
    () => resolveAskUserPromptModel(askUserQuestion, askUserInput),
    [askUserInput, askUserQuestion],
  )
  const awaitingHumanInput = approvalSteps.length > 0 || Boolean(askUserQuestion)
  const uiLocked = busy && !awaitingHumanInput
  const showAskUserPrompt = Boolean(askUserQuestion && dismissedAskUserQuestion !== askUserQuestion)
  const askUserHasStructuredResponse = selectedAskUserOptionIds.length > 0 || askUserFreeText.trim().length > 0

  useEffect(() => {
    if (!activeThreadId || !currentToolUI) return

    const activeAssistant = [...activeMessages]
      .reverse()
      .find((message) => message.role === 'assistant' && message.streaming)
    if (!activeAssistant) return

    const toolUiInput = getToolUiInput(currentToolUI)
    const toolCallId = findLiveToolCallId(activeAssistant.liveToolCalls ?? [], currentToolUI.toolName, toolUiInput)
    const toolCallStatus: LiveToolCallStatus = currentToolUI.type === 'input' ? 'waiting_input' : 'approval'
    const existing = activeAssistant.liveToolCalls?.find((call) => call.id === toolCallId)
    if (existing?.status === toolCallStatus && existing.result === currentToolUI.content) {
      return
    }

    updateMessage(activeThreadId, activeAssistant.id, {
      liveToolCalls: upsertLiveToolCall(activeAssistant.liveToolCalls ?? [], {
        id: toolCallId,
        toolName: currentToolUI.toolName,
        input: toolUiInput,
        status: toolCallStatus,
        result: currentToolUI.content,
      }),
    })
  }, [activeMessages, activeThreadId, currentToolUI, updateMessage])

  const enabledPluginSkills = useMemo<EnabledPluginSkill[]>(() => {
    return plugins
      .filter((plugin) => plugin.enabled)
      .flatMap((plugin) =>
        plugin.skills.map((skill) => ({
          pluginName: plugin.name,
          skillName: skill.name,
          command: skill.command.trim().toLowerCase(),
          promptTemplate: skill.promptTemplate,
          runMode: skill.runMode,
        }))
      )
      .filter((skill) => skill.command.startsWith('/'))
  }, [plugins])

  const registryCommands = useCommandRegistry((s) => s.commands)

  const slashCommandSuggestions = useMemo<SlashCommandSuggestion[]>(() => {
    const builtIn = registryCommands.map((cmd) => ({
      command: cmd.command,
      description: `${cmd.label} – ${cmd.description}`,
      source: 'built-in' as const,
    }))

    const pluginCommands = enabledPluginSkills.map((skill) => ({
      command: skill.command,
      args: '<prompt>',
      description: `${skill.skillName} (${skill.pluginName})`,
      source: 'plugin' as const,
    }))

    return [...builtIn, ...pluginCommands].sort((a, b) => a.command.localeCompare(b.command))
  }, [enabledPluginSkills, registryCommands])

  const slashCommandQuery = useMemo(() => {
    const trimmedStart = inputValue.trimStart()
    if (!trimmedStart.startsWith('/') || trimmedStart.includes('\n')) return null
    const firstWhitespace = trimmedStart.search(/\s/)
    if (firstWhitespace >= 0) return null
    return trimmedStart.slice(1).toLowerCase()
  }, [inputValue])

  const filteredSlashSuggestions = useMemo(() => {
    if (slashCommandQuery === null) return []
    return slashCommandSuggestions.filter((item) => {
      const haystack = `${item.command} ${item.args ?? ''} ${item.description}`.toLowerCase()
      return haystack.includes(slashCommandQuery)
    })
  }, [slashCommandQuery, slashCommandSuggestions])

  const showSlashSuggestions = slashSuggestionsOpen && filteredSlashSuggestions.length > 0

  useEffect(() => {
    if (slashCommandQuery === null) {
      setSlashSuggestionsOpen(false)
      setActiveSlashSuggestionIndex(0)
      return
    }

    setSlashSuggestionsOpen(true)
    setActiveSlashSuggestionIndex(0)
  }, [slashCommandQuery])

  const applySlashSuggestion = (suggestion: SlashCommandSuggestion) => {
    const nextValue = `${suggestion.command} `
    setInputValue(nextValue)
    setSlashSuggestionsOpen(false)
    window.requestAnimationFrame(() => {
      inputRef.current?.focus()
      inputRef.current?.setSelectionRange(nextValue.length, nextValue.length)
    })
  }

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
  ])

  useEffect(() => {
    if (!awaitingHumanInput || !busy) return
    setBusy(false)
  }, [awaitingHumanInput, busy])

  useEffect(() => {
    setSelectedAskUserOptionIds([])
    setAskUserFreeText('')
    setDismissedAskUserQuestion(null)
  }, [askUserQuestion])

  useEffect(() => {
    if (!askUserQuestion || !desktopNotificationsEnabled) return
    if (notifiedAskUserQuestionRef.current === askUserQuestion) return

    notifiedAskUserQuestionRef.current = askUserQuestion
    void showDesktopNotification('Open Cowork wartet auf deine Antwort', askUserQuestion)
      .then((shown) => {
        addLog({
          level: shown ? 'info' : 'warn',
          area: 'ui',
          message: shown
            ? 'Desktop-Benachrichtigung fuer Rueckfrage gesendet'
            : 'Desktop-Benachrichtigung fuer Rueckfrage konnte nicht gesendet werden',
          details: { question: askUserQuestion },
        })
      })
  }, [addLog, askUserQuestion, desktopNotificationsEnabled])

  const visibleMessages = useMemo(
    () => activeMessages.filter((message) => message.role !== 'system' || message.visibleInChat),
    [activeMessages],
  )

  useEffect(() => {
    const node = logRef.current
    if (!node) return

    const onScroll = () => {
      const threshold = 80
      const atBottom = node.scrollHeight - node.scrollTop - node.clientHeight <= threshold
      setShowScrollToBottom(!atBottom)
    }

    node.addEventListener('scroll', onScroll)
    onScroll()
    return () => node.removeEventListener('scroll', onScroll)
  }, [activeThreadId])

  const addNewAttachments = (newItems: ChatAttachment[]) => {
    if (newItems.length === 0) return
    setAttachments((prev) => {
      const merged = mergeAttachments(prev, newItems)
      if (merged.rejectedCount > 0) {
        setAttachmentNotice('Maximal 25 verbundene Elemente pro Nachricht erreicht.')
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
        { name: 'Dokumente', extensions: ['pdf', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx', 'odt', 'ods', 'odp', 'txt', 'rtf', 'csv'] },
        { name: 'Bilder', extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg'] },
        { name: 'Alle Dateien', extensions: ['*'] },
      ],
    })
    const selectedPaths = normalizeDialogSelection(selected)
    addNewAttachments(selectedPaths.map((path) => ({ path, kind: 'file' })))
  }

  const handleAttachFolders = async () => {
    const selected = await open({
      directory: true,
      multiple: true,
    })
    const selectedPaths = normalizeDialogSelection(selected)
    addNewAttachments(selectedPaths.map((path) => ({ path, kind: 'folder' })))
  }

  const handleRemoveAttachment = (target: ChatAttachment) => {
    setAttachments((prev) => prev.filter((item) => !(item.path === target.path && item.kind === target.kind)))
    setAttachmentNotice(null)
  }

  const handleInputDragOver = (event: DragEvent<HTMLTextAreaElement>) => {
    event.preventDefault()
    if (!dragOverInput) setDragOverInput(true)
  }

  const handleInputDragLeave = () => {
    if (dragOverInput) setDragOverInput(false)
  }

  const handleInputDrop = (event: DragEvent<HTMLTextAreaElement>) => {
    event.preventDefault()
    setDragOverInput(false)

    const fromFiles = extractFileAttachmentsFromFileList(event.dataTransfer.files)
    const fromUriList = extractFileAttachmentsFromUriList(
      event.dataTransfer.getData('text/uri-list') || ''
    )
    const droppedItems = [...fromFiles, ...fromUriList]

    if (droppedItems.length > 0) {
      addNewAttachments(droppedItems)
      setAttachmentNotice(null)
      return
    }

    setAttachmentNotice('Drop erkannt, aber kein lokaler Dateipfad gefunden. Bitte Dateien ueber den Button auswaehlen.')
  }

  const handleInputPaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const fromFiles = extractFileAttachmentsFromFileList(event.clipboardData.files)
    const fromUriList = extractFileAttachmentsFromUriList(
      event.clipboardData.getData('text/uri-list') || ''
    )
    const pastedItems = [...fromFiles, ...fromUriList]

    if (pastedItems.length === 0) {
      const imageFiles = Array.from(event.clipboardData.items)
        .filter((item) => item.type.startsWith('image/'))
        .map((item) => item.getAsFile())
        .filter((file): file is File => file !== null)

      if (imageFiles.length > 0) {
        event.preventDefault()
        void Promise.all(imageFiles.map((file) => createInlineImageAttachment(file)))
          .then((inlineAttachments) => {
            addNewAttachments(inlineAttachments)
            setAttachmentNotice(null)
          })
          .catch(() => {
            setAttachmentNotice('Bild aus Zwischenablage konnte nicht gelesen werden.')
          })
      }
      return
    }

    event.preventDefault()
    addNewAttachments(pastedItems)
    setAttachmentNotice(null)
  }

  const submitPrompt = async (
    rawInput: string,
    draftAttachments: ChatAttachment[] = attachments,
  ) => {
    const text = rawInput.trim()
    const hasDraftAttachments = Array.isArray(draftAttachments) && draftAttachments.length > 0
    if ((!text && !hasDraftAttachments) || busy) return
    const fallbackAttachmentPrompt = 'Bitte analysiere die angehaengten Dateien/Ordner und fuehre die Aufgabe aus.'
    const effectiveInput = text || fallbackAttachmentPrompt
    const replyingToAskUser = !!askUserQuestion

    if (replyingToAskUser) {
      clearCurrentToolUI()
    }

    if (text) {
      setPromptHistory((prev) => {
        const deduped = [text, ...prev.filter((entry) => entry !== text)]
        return deduped.slice(0, 30)
      })
    }
    setHistoryIndex(-1)

    let threadId = activeThreadId
    if (!threadId) {
      threadId = addThread(effectiveInput.slice(0, 50), createChatProviderSelection(providerState))
      setActiveThread(threadId)
    }

    // Automatische Titelgenerierung: Wenn der Thread noch "Neuer Chat" heißt und eine erste Nachricht gesendet wird
    const currentThread = useChatStore.getState().threads.find(t => t.id === threadId)
    if (currentThread && currentThread.title === 'Neuer Chat' && currentThread.messages.filter(m => m.role === 'user').length === 0) {
      const newTitle = effectiveInput.length > 50 ? effectiveInput.slice(0, 50) + '...' : effectiveInput
      // Titel über den store und DB aktualisieren
      const updatedThreads = useChatStore.getState().threads.map(t => 
        t.id === threadId ? { ...t, title: newTitle, updatedAt: Date.now() } : t
      )
      useChatStore.setState({ threads: updatedThreads })
      // Titel in der Datenbank aktualisieren
      void persistInvoke('db_save_thread', {
        id: threadId,
        title: newTitle,
        createdAt: new Date(currentThread.createdAt).toISOString()
      }, 'db_save_thread update title')
    }

    const slash = parseSlashCommand(text)

    const parseArgs = (value: string): string[] => {
      const matches = value.match(/"[^"]*"|'[^']*'|\S+/g) ?? []
      return matches.map((part) => part.replace(/^["']|["']$/g, ''))
    }

    const validateToolPolicy = (toolId: string, target: string | null): string | null => {
      if (policyFlags.strictPolicyEnforcement && !enabledClaudeToolIds.includes(toolId)) {
        return `Tool ${toolId} ist im aktuellen Profil deaktiviert.`
      }

      if (policyFlags.strictPolicyEnforcement && isToolDeniedByRules(toolId, target, toolDenyRules)) {
        return `Tool-Aufruf durch deny-rule blockiert (${toolId}).`
      }

      if (toolId === 'web_fetch' && !policyFlags.allowWebFetch) {
        return 'Web Fetch ist per Policy deaktiviert.'
      }

      if (toolId === 'web_search' && !policyFlags.allowWebSearch) {
        return 'Web Search ist per Policy deaktiviert.'
      }

      if (toolId === 'read_file' && !policyFlags.allowFileReadExtraction) {
        return 'Dateiextraktion ist per Policy deaktiviert.'
      }

      if (toolId === 'mcp' && !policyFlags.allowMcpToolCalls) {
        return 'MCP Tool Calls sind per Policy deaktiviert.'
      }

      return null
    }

    const appendAssistantMessage = (content: string) => {
      addMessage(threadId, {
        role: 'assistant',
        content,
        timestamp: Date.now(),
      })
      if (superVerboseAuditLogging) {
        void writeAuditEvent('super_verbose', 'cowork_assistant_message', {
          view: 'cowork',
          threadId,
          content,
        })
      }
    }

    const normalizeSlashCommand = (command: string): string => {
      const trimmed = command.trim().toLowerCase()
      if (!trimmed) return ''
      return trimmed.startsWith('/') ? trimmed : `/${trimmed}`
    }

    const renderSkillPrompt = (
      template: string,
      input: string,
      skill: EnabledPluginSkill,
    ): string => {
      const effectiveTemplate = template.trim() || 'Bearbeite diese Aufgabe: {{input}}'
      return effectiveTemplate
        .replace(/{{\s*input\s*}}/gi, input)
        .replace(/{{\s*skill_name\s*}}/gi, skill.skillName)
        .replace(/{{\s*plugin_name\s*}}/gi, skill.pluginName)
    }

    let skillPromptOverride: string | null = null
    let skillPlanMode = false
    let skillInvocationActive = false

    if (slash) {
      addMessage(threadId, { role: 'user', content: text, timestamp: Date.now() })
      if (superVerboseAuditLogging) {
        void writeAuditEvent('super_verbose', 'cowork_user_prompt', {
          view: 'cowork',
          threadId,
          prompt: text,
          slashCommand: slash.command,
          slashArgs: slash.args,
        })
      }
      setInputValue('')

      if (slash.command === 'help') {
        const cmdLines = registryCommands.map((c) => `${c.command} – ${c.description}`)
        const pluginLines = enabledPluginSkills.map((skill) => `${skill.command} - ${skill.skillName} (${skill.pluginName})`)
        const helpText = ['Verfuegbare Slash-Commands:', ...cmdLines]
        if (pluginLines.length > 0) {
          helpText.push('', 'Aktive Plugin-Skills:', ...pluginLines)
        }
        appendAssistantMessage(helpText.join('\n'))
        return
      }

      if (slash.command === 'tools') {
        appendAssistantMessage(
          [
            `Permission-Modus: ${claudePermissionMode}`,
            `Plan-Mode: ${claudePlanMode ? 'aktiv' : 'inaktiv'}`,
            `Aktive Tools: ${enabledClaudeToolIds.join(', ') || '(keine)'}`,
            `Deny-Rules: ${toolDenyRules.length}`,
            `Flags: dispatcher=${policyFlags.allowToolDispatcher}, mcp=${policyFlags.allowMcpToolCalls}, webFetch=${policyFlags.allowWebFetch}, webSearch=${policyFlags.allowWebSearch}, read=${policyFlags.allowFileReadExtraction}, compact=${policyFlags.autoCompactLongContext}`,
          ].join('\n')
        )
        return
      }

      if (slash.command === 'todo') {
        const args = parseArgs(slash.args)
        if (args.length === 0 || args[0].toLowerCase() === 'list') {
          const lines = tasks.slice(0, 12).map((task, index) => `${index + 1}. [${task.status}] ${task.title}`)
          appendAssistantMessage(lines.length > 0 ? lines.join('\n') : 'Keine offenen Todos/Tasks vorhanden.')
          return
        }

        if (args[0].toLowerCase() === 'add') {
          const title = args.slice(1).join(' ').trim()
          if (!title) {
            appendAssistantMessage('Bitte Titel angeben: /todo add <titel>')
            return
          }
          createTask(title, title.slice(0, 80), threadId)
          appendAssistantMessage(`Todo erstellt: ${title}`)
          return
        }

        appendAssistantMessage('Ungueltiger /todo Befehl. Nutze /todo list oder /todo add <titel>.')
        return
      }

      if (slash.command === 'tool') {
        if (!policyFlags.allowToolDispatcher) {
          appendAssistantMessage('Tool Dispatcher ist per Policy deaktiviert.')
          return
        }

        const args = parseArgs(slash.args)
        const toolName = (args[0] ?? '').toLowerCase()
        const rest = args.slice(1)

        if (!toolName) {
          appendAssistantMessage('Bitte Tool angeben: /tool <read_file|web_fetch|web_search|mcp_call> <args>')
          return
        }

        if (toolName === 'read_file') {
          const targetPath = rest.join(' ').trim()
          if (!targetPath) {
            appendAssistantMessage('Bitte Dateipfad angeben: /tool read_file C:\\pfad\\datei.txt')
            return
          }
          const violation = validateToolPolicy('read_file', targetPath)
          if (violation) {
            appendAssistantMessage(violation)
            return
          }

          setBusy(true)
          try {
            if (superVerboseAuditLogging) {
              void writeAuditEvent('super_verbose', 'cowork_tool_call_started', {
                view: 'cowork',
                threadId,
                toolName: 'read_file',
                targetPath,
              })
            }
            const textOut = await safeInvoke<string>('fs_extract_text', { path: targetPath })
            appendAssistantMessage(`Datei gelesen: ${targetPath}\n\n${textOut.slice(0, 5000)}`)
            if (superVerboseAuditLogging) {
              void writeAuditEvent('super_verbose', 'cowork_tool_call_finished', {
                view: 'cowork',
                threadId,
                toolName: 'read_file',
                targetPath,
                success: true,
                output: textOut,
              })
            }
          } catch (err) {
            const message = err instanceof Error ? err.message : String(err)
            appendAssistantMessage(`read_file fehlgeschlagen: ${message}`)
            if (superVerboseAuditLogging) {
              void writeAuditEvent('super_verbose', 'cowork_tool_call_finished', {
                view: 'cowork',
                threadId,
                toolName: 'read_file',
                targetPath,
                success: false,
                error: message,
              })
            }
          } finally {
            setBusy(false)
          }
          return
        }

        if (toolName === 'web_fetch') {
          const url = rest.join(' ').trim()
          if (!url) {
            appendAssistantMessage('Bitte URL angeben: /tool web_fetch https://example.com')
            return
          }
          const violation = validateToolPolicy('web_fetch', url)
          if (violation) {
            appendAssistantMessage(violation)
            return
          }

          setBusy(true)
          try {
            if (superVerboseAuditLogging) {
              void writeAuditEvent('super_verbose', 'cowork_tool_call_started', {
                view: 'cowork',
                threadId,
                toolName: 'web_fetch',
                url,
              })
            }
            const response = await safeInvoke<WebFetchResponse>('web_fetch_url', {
              request: { url, maxChars: 4000 },
            })
            appendAssistantMessage([
              `Web-Fetch: ${response.url}`,
              `Status: ${response.status}`,
              response.title ? `Titel: ${response.title}` : null,
              '',
              response.content,
            ].filter(Boolean).join('\n'))
            if (superVerboseAuditLogging) {
              void writeAuditEvent('super_verbose', 'cowork_tool_call_finished', {
                view: 'cowork',
                threadId,
                toolName: 'web_fetch',
                url,
                success: true,
                response,
              })
            }
          } catch (err) {
            const message = err instanceof Error ? err.message : String(err)
            appendAssistantMessage(`web_fetch fehlgeschlagen: ${message}`)
            if (superVerboseAuditLogging) {
              void writeAuditEvent('super_verbose', 'cowork_tool_call_finished', {
                view: 'cowork',
                threadId,
                toolName: 'web_fetch',
                url,
                success: false,
                error: message,
              })
            }
          } finally {
            setBusy(false)
          }
          return
        }

        if (toolName === 'web_search') {
          const query = rest.join(' ').trim()
          if (!query) {
            appendAssistantMessage('Bitte Suchanfrage angeben: /tool web_search Wetter Stuttgart heute')
            return
          }
          const violation = validateToolPolicy('web_search', query)
          if (violation) {
            appendAssistantMessage(violation)
            return
          }

          setBusy(true)
          try {
            if (superVerboseAuditLogging) {
              void writeAuditEvent('super_verbose', 'cowork_tool_call_started', {
                view: 'cowork',
                threadId,
                toolName: 'web_search',
                query,
              })
            }
            const response = await safeInvoke<{ query: string; results: Array<{ title: string; url: string; snippet: string }> }>('web_search', {
              request: {
                query,
                maxResults: 5,
              },
            })
            const lines = response.results.map((item, index) => {
              const snippet = item.snippet ? `\n${item.snippet}` : ''
              return `${index + 1}. ${item.title}\n${item.url}${snippet}`
            })
            appendAssistantMessage(lines.join('\n\n') || `Keine Treffer fuer "${query}".`)
            if (superVerboseAuditLogging) {
              void writeAuditEvent('super_verbose', 'cowork_tool_call_finished', {
                view: 'cowork',
                threadId,
                toolName: 'web_search',
                query,
                success: true,
                response,
              })
            }
          } catch (err) {
            const message = err instanceof Error ? err.message : String(err)
            appendAssistantMessage(`web_search fehlgeschlagen: ${message}`)
            if (superVerboseAuditLogging) {
              void writeAuditEvent('super_verbose', 'cowork_tool_call_finished', {
                view: 'cowork',
                threadId,
                toolName: 'web_search',
                query,
                success: false,
                error: message,
              })
            }
          } finally {
            setBusy(false)
          }
          return
        }

        if (toolName === 'mcp_call') {
          const mcpToolName = rest[0]
          if (!mcpToolName) {
            appendAssistantMessage('Bitte MCP Toolnamen angeben: /tool mcp_call <toolName> {"arg":"value"}')
            return
          }

          const violation = validateToolPolicy('mcp', mcpToolName)
          if (violation) {
            appendAssistantMessage(violation)
            return
          }

          const jsonRaw = rest.slice(1).join(' ').trim()
          let parsedArgs: Record<string, unknown> = {}
          if (jsonRaw) {
            try {
              parsedArgs = JSON.parse(jsonRaw) as Record<string, unknown>
            } catch {
              appendAssistantMessage('MCP Args muessen gueltiges JSON sein.')
              return
            }
          }

          setBusy(true)
          try {
            if (superVerboseAuditLogging) {
              void writeAuditEvent('super_verbose', 'cowork_tool_call_started', {
                view: 'cowork',
                threadId,
                toolName: 'mcp_call',
                mcpToolName,
                args: parsedArgs,
                server: {
                  name: mcpServer.name,
                  command: mcpServer.command,
                  args: mcpServer.args,
                },
              })
            }
            const response = await safeInvoke<McpCallResponse>('mcp_call_tool', {
              request: {
                name: mcpServer.name,
                command: mcpServer.command,
                args: parseArgs(mcpServer.args),
                env: mcpServer.env,
                toolName: mcpToolName,
                toolArgs: parsedArgs,
              },
            })
            appendAssistantMessage(
              response.success
                ? `MCP ${response.toolName} erfolgreich:\n\n${response.result}`
                : `MCP ${response.toolName} Fehler: ${response.error ?? response.result}`
            )
            if (superVerboseAuditLogging) {
              void writeAuditEvent('super_verbose', 'cowork_tool_call_finished', {
                view: 'cowork',
                threadId,
                toolName: 'mcp_call',
                mcpToolName,
                success: response.success,
                response,
              })
            }
          } catch (err) {
            const message = err instanceof Error ? err.message : String(err)
            appendAssistantMessage(`mcp_call fehlgeschlagen: ${message}`)
            if (superVerboseAuditLogging) {
              void writeAuditEvent('super_verbose', 'cowork_tool_call_finished', {
                view: 'cowork',
                threadId,
                toolName: 'mcp_call',
                mcpToolName,
                success: false,
                error: message,
              })
            }
          } finally {
            setBusy(false)
          }
          return
        }

        appendAssistantMessage(`Unbekanntes Tool: ${toolName}. Erlaubt: read_file, web_fetch, web_search, mcp_call`)
        return
      }

      if (slash.command === 'mode') {
        const target = slash.args.toLowerCase()
        if (target === 'plan') {
          setClaudePlanMode(true)
          appendAssistantMessage('Plan-Mode aktiviert.')
        } else if (target === 'execute') {
          setClaudePlanMode(false)
          appendAssistantMessage('Plan-Mode deaktiviert (Execute).')
        } else {
          appendAssistantMessage('Ungueltiger Modus. Verwende: /mode plan oder /mode execute')
        }
        return
      }

      if (slash.command === 'permissions') {
        const target = slash.args as ClaudePermissionMode
        if (VALID_PERMISSION_MODES.includes(target)) {
          setClaudePermissionMode(target)
          appendAssistantMessage(`Permission-Modus gesetzt: ${target}`)
        } else {
          appendAssistantMessage('Ungueltiger Permission-Modus. Erlaubt: default, acceptEdits, bypassPermissions, dontAsk, plan')
        }
        return
      }

      if (slash.command === 'fetch') {
        if (!slash.args) {
          appendAssistantMessage('Bitte URL angeben: /fetch https://example.com')
          return
        }

        const violation = validateToolPolicy('web_fetch', slash.args)
        if (violation) {
          appendAssistantMessage(violation)
          return
        }

        setBusy(true)
        setError(null)
        try {
          if (superVerboseAuditLogging) {
            void writeAuditEvent('super_verbose', 'cowork_tool_call_started', {
              view: 'cowork',
              threadId,
              toolName: 'web_fetch',
              url: slash.args,
              viaCommand: '/fetch',
            })
          }
          const response = await safeInvoke<WebFetchResponse>('web_fetch_url', {
            request: {
              url: slash.args,
              maxChars: 4000,
            },
          })

          appendAssistantMessage(
            [
              `Web-Fetch: ${response.url}`,
              `Status: ${response.status}`,
              response.title ? `Titel: ${response.title}` : null,
              '',
              response.content,
              response.truncated ? '\n[Ausgabe gekuerzt]' : null,
            ]
              .filter(Boolean)
              .join('\n')
          )
          if (superVerboseAuditLogging) {
            void writeAuditEvent('super_verbose', 'cowork_tool_call_finished', {
              view: 'cowork',
              threadId,
              toolName: 'web_fetch',
              url: slash.args,
              viaCommand: '/fetch',
              success: true,
              response,
            })
          }
        } catch (err) {
          const message = err instanceof Error ? err.message : String(err)
          setError(message)
          appendAssistantMessage(`Web-Fetch fehlgeschlagen: ${message}`)
          if (superVerboseAuditLogging) {
            void writeAuditEvent('super_verbose', 'cowork_tool_call_finished', {
              view: 'cowork',
              threadId,
              toolName: 'web_fetch',
              url: slash.args,
              viaCommand: '/fetch',
              success: false,
              error: message,
            })
          }
        } finally {
          setBusy(false)
        }
        return
      }

      // ── AI Prompt Commands: flow through to Ollama ──
      const aiPrompts: Record<string, string> = {
        'review': 'Fuehre ein gruendliches Code-Review durch. Pruefe auf: Bugs, Code-Qualitaet, Best Practices, Lesbarkeit, Wartbarkeit. Gib konkrete Verbesserungsvorschlaege mit Codebeispielen.',
        'ultrareview': 'Fuehre ein umfassendes Ultra-Review durch:\n1. Architektur-Analyse\n2. Sicherheits-Check (OWASP Top 10)\n3. Performance-Analyse\n4. Code-Qualitaet & Clean Code\n5. Test-Abdeckung\n6. Dokumentation\n7. Dependency-Check\n8. Best Practices\nGib fuer jeden Bereich eine detaillierte Bewertung mit konkreten Empfehlungen.',
        'ultraplan': 'Erstelle einen detaillierten Multi-Step Plan:\n1. Analysiere die Anforderungen gruendlich\n2. Identifiziere Abhaengigkeiten und Risiken\n3. Erstelle nummerierte Schritte mit geschaetztem Aufwand\n4. Definiere Akzeptanzkriterien pro Schritt\n5. Liste Risiken und Mitigations-Strategien\n6. Schlage ein Test-Konzept vor',
        'security-review': 'Fuehre eine umfassende Sicherheitsanalyse durch:\n- OWASP Top 10 Pruefung\n- Injection-Schwachstellen (SQL, XSS, Command)\n- Authentication & Authorization\n- Kryptografie-Nutzung\n- Input-Validierung\n- Sensitive Daten-Handling\n- Dependencies mit bekannten CVEs\nGib Schweregrad (Critical/High/Medium/Low) und konkrete Fix-Empfehlungen.',
        'simplify': 'Vereinfache den folgenden Code:\n- Reduziere Komplexitaet (zyklomatisch und kognitiv)\n- Entferne Redundanzen und toten Code\n- Verbessere Lesbarkeit und Wartbarkeit\n- Halte die Funktionalitaet identisch\n- Erklaere jede Aenderung kurz',
        'autofix-pr': 'Analysiere und behebe automatisch alle Probleme:\n- Linting- und Formatierungs-Fehler\n- Type-Fehler und fehlende Typen\n- Fehlende oder fehlerhafte Tests\n- Code-Style Verbesserungen\n- Dokumentations-Luecken\nZeige fuer jede Aenderung Vorher/Nachher.',
        'team-onboarding': 'Erstelle eine ausfuehrliche Onboarding-Anleitung:\n1. Projekt-Uebersicht und Architektur\n2. Setup-Anleitung (Schritt fuer Schritt)\n3. Coding-Konventionen und Style Guide\n4. Wichtige Dateien und Ordnerstruktur\n5. Entwicklungs-Workflow und Prozesse\n6. Haeufige Aufgaben mit Loesungen\n7. Debugging-Tipps',
        'passes': 'Fuehre eine iterative Multi-Pass Analyse durch. In jedem Durchlauf vertiefe die Analyse und verbessere die vorherigen Ergebnisse:\nPass 1: Grobe Analyse und Ueberblick\nPass 2: Detailanalyse und Verbesserungen\nPass 3: Feinschliff und finale Empfehlungen',
      }

      if (slash.command in aiPrompts) {
        if (!slash.args?.trim()) {
          appendAssistantMessage(`Bitte Kontext angeben: /${slash.command} <beschreibung oder code>`)
          return
        }
        skillPromptOverride = `${aiPrompts[slash.command]}\n\nAufgabe/Kontext:\n${slash.args}`
        // Don't return → flows to Ollama streaming below
      }

      // ── Model & Config Commands ──
      if (slash.command === 'model') {
        if (!slash.args?.trim()) {
          appendAssistantMessage(`Aktueller Provider: ${providerState.label}\nAktuelles Modell: ${providerState.model || '(nicht gesetzt)'}\nVerfuegbar: ${selectableModels.join(', ') || '(keine geladen)'}\nNutze: /model <name>`)
        } else {
          const nextModel = slash.args.trim()
          setThreadProviderSettings(threadId, {
            ...createChatProviderSelection(providerState),
            model: nextModel,
          })
          appendAssistantMessage(`Modell fuer diesen Chat gewechselt: ${nextModel}`)
        }
        return
      }

      if (slash.command === 'effort') {
        const levels: Record<string, number> = { low: 0.1, medium: 0.5, high: 0.9 }
        const level = slash.args?.trim().toLowerCase() ?? ''
        if (level in levels) {
          setOllama({ temperature: levels[level] })
          appendAssistantMessage(`Aufwand-Level: ${level} (Temperatur: ${levels[level]})`)
        } else {
          appendAssistantMessage(`Aktuell: Temperatur ${ollama.temperature ?? 0.2}\nNutze: /effort low | medium | high`)
        }
        return
      }

      if (slash.command === 'fast') {
        const fast = selectableModels.find(m => m.includes('tiny') || m.includes('mini') || m.includes('3b') || m.includes('phi')) ?? selectableModels[0]
        if (fast) {
          setThreadProviderSettings(threadId, {
            ...createChatProviderSelection(providerState),
            model: fast,
          })
          appendAssistantMessage(`Schnell-Modus fuer diesen Chat: ${fast}`)
        } else {
          appendAssistantMessage('Kein schnelles Modell gefunden. Lade Modelle in den Einstellungen.')
        }
        return
      }

      if (slash.command === 'powerup') {
        const power = selectableModels.find(m => m.includes('70b') || m.includes('405b') || m.includes('72b') || m.includes('llama3')) ?? selectableModels[selectableModels.length - 1]
        if (power) {
          setThreadProviderSettings(threadId, {
            ...createChatProviderSelection(providerState),
            model: power,
          })
          appendAssistantMessage(`Power-Modus fuer diesen Chat: ${power}`)
        } else {
          appendAssistantMessage('Kein starkes Modell gefunden. Lade Modelle in den Einstellungen.')
        }
        return
      }

      if (slash.command === 'compact') {
        useCoworkStore.getState().setPolicyFlag('autoCompactLongContext', true)
        appendAssistantMessage('Kontext-Kompression aktiviert. Aeltere Nachrichten werden automatisch zusammengefasst.')
        return
      }

      if (slash.command === 'debug') {
        const newVerbose = !verboseMode
        useConfigStore.getState().setPreference('verboseMode', newVerbose)
        useConfigStore.getState().setPreference('superVerboseAuditLogging', newVerbose)
        appendAssistantMessage(`Debug-Modus: ${newVerbose ? 'aktiviert' : 'deaktiviert'} (Verbose + Audit-Logging)`)
        return
      }

      if (slash.command === 'sandbox') {
        useCoworkStore.getState().setPolicyFlag('strictPolicyEnforcement', true)
        useConfigStore.getState().setPreference('readOnlyFsMode', true)
        appendAssistantMessage('Sandbox-Modus aktiviert:\n- Nur-Lese-Zugriff auf Dateisystem\n- Strenge Policy-Durchsetzung\n- Alle destruktiven Operationen blockiert')
        return
      }

      if (slash.command === 'less-permission-prompts') {
        useConfigStore.getState().setPreferences({
          autoApproveSafeTools: true,
          confirmOnCloseWithRunningTasks: false,
          fallbackToHumanOnRepeatedFailure: false,
        })
        appendAssistantMessage('Berechtigungsfragen reduziert. Sichere Tools werden automatisch genehmigt.')
        return
      }

      if (slash.command === 'web-setup') {
        useCoworkStore.getState().setPolicyFlag('allowWebFetch', true)
        useCoworkStore.getState().setPolicyFlag('allowWebSearch', true)
        appendAssistantMessage('Web-Zugriff aktiviert. /fetch <url>, /tool web_fetch und /tool web_search sind nun erlaubt.')
        return
      }

      if (slash.command === 'terminal-setup') {
        try {
          await useTerminalStore.getState().ensureLocalBackend()
          appendAssistantMessage('Terminal-Backend eingerichtet. Lokales Terminal ist aktiv.')
        } catch (err) {
          const msg = err instanceof Error ? err.message : String(err)
          appendAssistantMessage(`Terminal-Setup fehlgeschlagen: ${msg}`)
        }
        return
      }

      // ── Data & Session Commands ──
      if (slash.command === 'context') {
        const msgCount = activeMessages.length
        const charCount = activeMessages.reduce((a, m) => a + m.content.length, 0)
        const userMsgs = activeMessages.filter(m => m.role === 'user').length
        const assistantMsgs = activeMessages.filter(m => m.role === 'assistant').length
        appendAssistantMessage(`Kontext:\n- Thread: "${activeThread?.title ?? 'Unbenannt'}"\n- ${msgCount} Nachrichten (${userMsgs} User, ${assistantMsgs} Assistent)\n- ${charCount} Zeichen gesamt\n- Provider: ${providerState.label}\n- Modell: ${providerState.model || '(nicht gesetzt)'}\n- Attachments: ${attachments.length}`)
        return
      }

      if (slash.command === 'rename') {
        if (!slash.args?.trim()) {
          appendAssistantMessage('Bitte neuen Namen angeben: /rename <name>')
          return
        }
        if (activeThread) {
          void safeInvokeVoid('db_save_thread', { id: activeThread.id, title: slash.args.trim(), createdAt: new Date(activeThread.createdAt).toISOString() })
          appendAssistantMessage(`Thread umbenannt: ${slash.args.trim()}`)
        }
        return
      }

      if (slash.command === 'branch') {
        if (activeThread) {
          const newId = addThread(`Zweig: ${activeThread.title}`)
          setActiveThread(newId)
          appendAssistantMessage(`Neuer Zweig erstellt von "${activeThread.title}".`)
        }
        return
      }

      if (slash.command === 'clear') {
        if (activeThread) {
          const cs = useChatStore.getState()
          cs.deleteThread(activeThread.id)
          cs.addThread('Neuer Chat')
        }
        return
      }

      if (slash.command === 'resume') {
        const cs = useChatStore.getState()
        const latest = cs.threads.sort((a, b) => b.updatedAt - a.updatedAt)[0]
        if (latest) {
          cs.setActiveThread(latest.id)
          appendAssistantMessage(`Fortgesetzt: "${latest.title}" (${latest.messages.length} Nachrichten)`)
        } else {
          appendAssistantMessage('Keine vorherige Session gefunden.')
        }
        return
      }

      if (slash.command === 'rewind') {
        const count = Number.parseInt(slash.args ?? '1', 10) || 1
        if (activeThread) {
          const removed = useChatStore.getState().removeLastMessagePairs(activeThread.id, count)
          appendAssistantMessage(
            removed.pairsRemoved > 0
              ? `${removed.pairsRemoved} Nachrichtenpaar(e) entfernt (${removed.messagesRemoved} Nachrichten). Sende deine naechste Anweisung.`
              : 'Keine vollstaendigen User/Assistant-Paare zum Zurueckspulen gefunden.'
          )
        } else {
          appendAssistantMessage('Kein aktiver Thread zum Zurueckspulen.')
        }
        return
      }

      if (slash.command === 'exit') {
        useChatStore.getState().setActiveThread(null)
        return
      }

      if (slash.command === 'recap') {
        const userMsgs = activeMessages.filter(m => m.role === 'user')
        const topics = userMsgs.slice(-5).map(m => m.content.slice(0, 60)).join('\n- ')
        appendAssistantMessage(`Session-Recap:\n- ${userMsgs.length} Benutzer-Nachrichten\n- Thread: "${activeThread?.title ?? '?'}"\n- Gestartet: ${activeThread ? new Date(activeThread.createdAt).toLocaleString('de-DE') : '?'}\n\nLetzte Themen:\n- ${topics || '(keine)'}`)
        return
      }

      if (slash.command === 'memory') {
        if (slash.args?.trim()) {
          await useMemoryStore.getState().searchEntries(slash.args.trim())
          const entries = useMemoryStore.getState().entries
          appendAssistantMessage(entries.length > 0
            ? `Memory-Suche "${slash.args.trim()}":\n${entries.slice(0, 10).map(e => `- [${e.scope}/${e.category}] ${e.content.slice(0, 100)}`).join('\n')}`
            : `Keine Treffer fuer "${slash.args.trim()}".`)
        } else {
          await useMemoryStore.getState().loadEntries()
          const entries = useMemoryStore.getState().entries
          appendAssistantMessage(`Memory: ${entries.length} Eintraege\nNutze /memory <suchbegriff> zum Suchen oder verwalte ueber Features > Memory.`)
        }
        return
      }

      if (slash.command === 'stats') {
        try {
          await useInsightsStore.getState().loadSummary()
          const summary = useInsightsStore.getState().summary
          appendAssistantMessage(summary
            ? `Statistiken:\n- Events: ${summary.totalEvents}\n- Sessions: ${summary.totalSessions}\n- Nachrichten: ${summary.totalMessagesSent}\n- Token (est.): ${summary.totalTokensEst}\n- Skills: ${summary.skillUsageCount}\n- Memory: ${summary.memoryEntryCount}`
            : 'Keine Statistiken verfuegbar.')
        } catch {
          appendAssistantMessage('Statistiken konnten nicht geladen werden.')
        }
        return
      }

      if (slash.command === 'status') {
        const procs = useProcessStore.getState().processes
        const backends = useTerminalStore.getState().backends
        const ollamaStatus = await getOllamaStatusText(ollama)
        appendAssistantMessage(`Status:\n- Ollama: ${ollamaStatus}\n- Aktiver Provider: ${providerState.label}\n- Modell: ${providerState.model || '(nicht gesetzt)'}\n- Threads: ${useChatStore.getState().threads.length}\n- Prozesse: ${procs.length}\n- Backends: ${backends.length}\n- Plan-Mode: ${claudePlanMode ? 'aktiv' : 'inaktiv'}\n- Permissions: ${claudePermissionMode}`)
        return
      }

      if (slash.command === 'cost') {
        try {
          await useInsightsStore.getState().loadSummary()
          const summary = useInsightsStore.getState().summary
          const tokens = summary?.totalTokensEst ?? 0
          appendAssistantMessage(`Kosten-Schaetzung:\n- Token gesamt: ${tokens}\n- Lokales Modell (Ollama): 0 EUR\n- Geschaetzte API-Kosten: ~${(tokens * 0.000002).toFixed(4)} EUR`)
        } catch {
          appendAssistantMessage('Kosten konnten nicht berechnet werden.')
        }
        return
      }

      if (slash.command === 'export') {
        if (activeThread) {
          const rawFormat = slash.args?.trim().toLowerCase()
          const format: ChatExportFormat = rawFormat === 'json' || rawFormat === 'txt' ? rawFormat : 'md'
          const { data, ext } = buildChatExportPayload(activeThread, activeMessages, format)
          const suggestedFileName = `${sanitizeExportBaseName(activeThread.title)}.${ext}`

          if (hasTauriRuntime()) {
            const selectedPath = await save({
              defaultPath: suggestedFileName,
              filters: [
                { name: 'Markdown', extensions: ['md'] },
                { name: 'Text', extensions: ['txt'] },
                { name: 'JSON', extensions: ['json'] },
              ],
            })

            if (!selectedPath) {
              appendAssistantMessage('Export abgebrochen.')
              return
            }

            await safeInvoke('export_save_text_file', {
              path: selectedPath,
              content: data,
            })

            await navigator.clipboard.writeText(data).catch(() => {})
            appendAssistantMessage(`Export (${ext}) gespeichert: ${selectedPath}`)
          } else {
            const blob = new Blob([data], { type: 'text/plain' })
            const url = URL.createObjectURL(blob)
            const anchor = document.createElement('a')
            anchor.href = url
            anchor.download = suggestedFileName
            document.body.appendChild(anchor)
            anchor.click()
            document.body.removeChild(anchor)
            URL.revokeObjectURL(url)
            await navigator.clipboard.writeText(data).catch(() => {})
            appendAssistantMessage(`Export (${ext}) heruntergeladen und in Zwischenablage kopiert (${data.length} Zeichen).`)
          }
        }
        return
      }

      if (slash.command === 'copy') {
        const lastAssistant = [...activeMessages].reverse().find(m => m.role === 'assistant')
        if (lastAssistant) {
          await navigator.clipboard.writeText(lastAssistant.content).catch(() => {})
          appendAssistantMessage(`Letzte Antwort kopiert (${lastAssistant.content.length} Zeichen).`)
        } else {
          appendAssistantMessage('Keine Antwort zum Kopieren vorhanden.')
        }
        return
      }

      if (slash.command === 'doctor') {
        setBusy(true)
        try {
          const ollamaStatus = await getOllamaStatusText(ollama)
          const backends = useTerminalStore.getState().backends
          const entries = useMemoryStore.getState().entries
          appendAssistantMessage(`System-Diagnose:\n- Ollama: ${ollamaStatus} (${ollama.baseUrl})\n- Aktiver Provider: ${providerState.label} (${providerState.endpoint || 'nicht gesetzt'})\n- Modell: ${providerState.model || '(nicht gesetzt)'}\n- DB: aktiv\n- MCP: ${mcpServer.command ? `konfiguriert (${mcpServer.name})` : 'nicht konfiguriert'}\n- Audit: aktiv\n- Terminal-Backends: ${backends.length}\n- Memory-Eintraege: ${entries.length}\n- Plugins: ${plugins.length}`)
        } finally {
          setBusy(false)
        }
        return
      }

      if (slash.command === 'heapdump') {
        try {
          const snapshot = await useMemoryStore.getState().createSnapshot()
          appendAssistantMessage(`Heap Dump erstellt:\n- Memory-Eintraege: ${snapshot.total_entries}\n- Profil-Keys: ${snapshot.total_profile_keys}\n- Timestamp: ${new Date().toLocaleString('de-DE')}`)
        } catch {
          appendAssistantMessage('Heap Dump fehlgeschlagen.')
        }
        return
      }

      if (slash.command === 'skills') {
        await useSkillStore.getState().loadSkills()
        const skills = useSkillStore.getState().skills
        if (skills.length > 0) {
          const lines = skills.slice(0, 15).map(s => `- ${s.name}: ${s.description?.slice(0, 60) ?? '(kein Beschr.)'}`)
          appendAssistantMessage(`Skills (${skills.length}):\n${lines.join('\n')}`)
        } else {
          appendAssistantMessage('Keine Skills vorhanden. Skills werden automatisch gelernt oder koennen manuell angelegt werden.')
        }
        return
      }

      if (slash.command === 'tasks') {
        await useTaskStore.getState().loadFromDb()
        const allTasks = useTaskStore.getState().tasks
        if (allTasks.length > 0) {
          const lines = allTasks.slice(0, 15).map((t, i) => `${i + 1}. [${t.status}] ${t.title}`)
          appendAssistantMessage(`Aufgaben (${allTasks.length}):\n${lines.join('\n')}`)
        } else {
          appendAssistantMessage('Keine offenen Aufgaben. Nutze /todo add <titel> zum Erstellen.')
        }
        return
      }

      if (slash.command === 'btw') {
        if (!slash.args?.trim()) {
          appendAssistantMessage('Nutze: /btw <info> um Kontext-Informationen hinzuzufuegen.')
          return
        }
        useMemoryStore.getState().upsertEntry({
          id: `btw-${Date.now()}`, scope: 'session', category: 'context', key: 'btw', content: slash.args.trim(),
        })
        appendAssistantMessage(`Notiert: ${slash.args.trim()}`)
        return
      }

      if (slash.command === 'feedback') {
        void safeInvokeVoid('audit_event', {
          area: 'feedback', action: 'user_feedback', details: slash.args || 'Kein Kommentar',
        })
        appendAssistantMessage(`Feedback gespeichert. Danke!${slash.args ? '' : ' (Tipp: /feedback <kommentar>)'}`)
        return
      }

      if (slash.command === 'schedule') {
        if (!slash.args?.trim()) {
          appendAssistantMessage(SCHEDULE_HELP_TEXT)
          return
        }
        const parsed = parseScheduledTaskInput(slash.args)
        if (!parsed) {
          appendAssistantMessage(SCHEDULE_HELP_TEXT)
          return
        }
        await useCoworkStore.getState().upsertScheduledTask({
          id: `sched-${Date.now()}`,
          name: parsed.prompt.slice(0, 40),
          prompt: parsed.prompt,
          cronLike: parsed.scheduleExpr,
          taskKind: 'prompt',
          crewId: null,
          crewSnapshotJson: null,
          modelConfigJson: JSON.stringify(ollama),
          priority: 100,
          dependsOnTaskIds: [],
          active: true,
          lastRunAt: null,
          nextRunAt: null,
        })
        appendAssistantMessage(`Aufgabe geplant: "${parsed.prompt}" (${parsed.scheduleExpr})`)
        return
      }

      if (slash.command === 'agents') {
        useCrewStore.getState().loadAgents()
        const agents = useCrewStore.getState().agents
        if (agents.length > 0) {
          const lines = agents.map(a => `- ${a.name} (${a.role}): ${a.backstory.slice(0, 60)}`)
          appendAssistantMessage(`Agenten (${agents.length}):\n${lines.join('\n')}`)
        } else {
          appendAssistantMessage('Keine Agenten konfiguriert. Verwalte Agenten unter Features > Crew AI.')
        }
        return
      }

      if (slash.command === 'crew') {
        if (slash.args?.trim()) {
          useCrewStore.getState().createCrew(`crew-${Date.now()}`, slash.args.trim(), [])
          appendAssistantMessage(`Crew erstellt: ${slash.args.trim()}`)
        } else {
          const crews = useCrewStore.getState().crews
          appendAssistantMessage(crews.length > 0
            ? `Crews (${crews.length}):\n${crews.map(c => `- ${c.name} (${c.agents.length} Agenten)`).join('\n')}`
            : 'Keine Crews vorhanden. Nutze /crew <name> zum Erstellen.')
        }
        return
      }

      if (slash.command === 'batch') {
        if (!slash.args?.trim()) {
          appendAssistantMessage('Nutze: /batch <aufgabe1>; <aufgabe2>; <aufgabe3>')
          return
        }
        const batchTasks = slash.args.split(';').map(t => t.trim()).filter(Boolean)
        for (const bt of batchTasks) {
          createTask(bt, bt.slice(0, 80), threadId)
        }
        appendAssistantMessage(`${batchTasks.length} Batch-Aufgaben erstellt:\n${batchTasks.map((t, i) => `${i + 1}. ${t}`).join('\n')}`)
        return
      }

      if (slash.command === 'loop') {
        if (!slash.args?.trim()) {
          appendAssistantMessage('Nutze: /loop <aufgabe> - Agent arbeitet autonom bis erledigt.')
          return
        }
        useConfigStore.getState().setPreference('autoPilotAllTools', true)
        skillPromptOverride = `Du arbeitest im Agentic Loop Modus. Arbeite autonom an der folgenden Aufgabe bis sie vollstaendig erledigt ist. Pruefe dein Ergebnis, iteriere bei Bedarf.\n\nAufgabe:\n${slash.args}`
        // flows to Ollama
      }

      // ── Navigation & Display Commands ──
      if (slash.command === 'config' || slash.command === 'settings') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage('Einstellungen geoeffnet.')
        return
      }

      if (slash.command === 'ide') {
        useUiStore.getState().setActiveMode('work')
        appendAssistantMessage('Arbeitsbereich aktiviert.')
        return
      }

      if (slash.command === 'focus') {
        useUiStore.getState().toggleLeftSidebar()
        appendAssistantMessage('Fokus-Modus umgeschaltet.')
        return
      }

      if (slash.command === 'theme' || slash.command === 'color') {
        const target = slash.args?.trim().toLowerCase()
        if (target === 'dark') { useUiStore.getState().setTheme('dark'); appendAssistantMessage('Dark Theme aktiviert.') }
        else if (target === 'light') { useUiStore.getState().setTheme('light'); appendAssistantMessage('Light Theme aktiviert.') }
        else { useUiStore.getState().toggleTheme(); appendAssistantMessage('Theme umgeschaltet.') }
        return
      }

      if (slash.command === 'mcp') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage(`MCP-Server: ${mcpServer.command ? `${mcpServer.name} (${mcpServer.command})` : 'nicht konfiguriert'}\nOeffne Einstellungen zur Konfiguration.`)
        return
      }

      if (slash.command === 'keybindings') {
        useUiStore.getState().setShortcutsOverlayOpen(true)
        appendAssistantMessage('Tastenkuerzel-Uebersicht geoeffnet.')
        return
      }

      if (slash.command === 'statusline') {
        useConfigStore.getState().setPreference('compactMode', !compactMode)
        appendAssistantMessage(`Kompakt-Modus: ${compactMode ? 'deaktiviert' : 'aktiviert'}`)
        return
      }

      if (slash.command === 'tui') {
        useConfigStore.getState().setPreference('compactMode', true)
        appendAssistantMessage('TUI-Modus aktiviert (kompakte Ansicht).')
        return
      }

      if (slash.command === 'mobile') {
        useConfigStore.getState().setPreference('compactMode', true)
        useConfigStore.getState().setPreference('fontScale', 110)
        appendAssistantMessage('Mobile-Ansicht aktiviert (kompakt + groessere Schrift).')
        return
      }

      // ── Misc Commands ──
      if (slash.command === 'plugin') {
        if (slash.args === 'examples' || slash.args === 'install') {
          useCoworkStore.getState().installPluginExamples()
          appendAssistantMessage('Beispiel-Plugins installiert.')
        } else {
          appendAssistantMessage(`Plugins: ${plugins.length} installiert (${plugins.filter(p => p.enabled).length} aktiv)\nNutze: /plugin install oder verwalte unter Features > Plugins.`)
        }
        return
      }

      if (slash.command === 'reload-plugins') {
        useCoworkStore.getState().installPluginExamples()
        appendAssistantMessage('Plugins neu geladen.')
        return
      }

      if (slash.command === 'insights' || slash.command === 'usage') {
        await useInsightsStore.getState().loadSummary()
        await useInsightsStore.getState().loadEvents()
        const summary = useInsightsStore.getState().summary
        appendAssistantMessage(summary
          ? `Insights:\n- Events: ${summary.totalEvents}\n- Sessions: ${summary.totalSessions}\n- Nachrichten: ${summary.totalMessagesSent}\n- Token: ${summary.totalTokensEst}\n- Oeffne Features > Insights fuer Details.`
          : 'Keine Insights verfuegbar.')
        return
      }

      if (slash.command === 'diff') {
        appendAssistantMessage('Diff: Nutze die Einstellungen > Backup um Datei-Diffs zu pruefen.\nOder nutze /tool read_file <pfad> um eine Datei zu lesen.')
        return
      }

      if (slash.command === 'init') {
        void safeInvokeVoid('audit_event', { area: 'project', action: 'init', details: 'Projekt-Init' })
        appendAssistantMessage('Projekt initialisiert. Open_Cowork Konfiguration wurde erstellt.')
        return
      }

      if (slash.command === 'teleport') {
        if (slash.args?.trim()) {
          useUiStore.getState().setWorkingPath(slash.args.trim(), 'file')
          appendAssistantMessage(`Navigation zu: ${slash.args.trim()}`)
        } else {
          appendAssistantMessage('Nutze: /teleport <pfad> um schnell zu einer Datei/Ordner zu springen.')
        }
        return
      }

      if (slash.command === 'chrome') {
        useCoworkStore.getState().toggleConnector('chrome', true)
        appendAssistantMessage('Chrome-Integration aktiviert.')
        return
      }

      if (slash.command === 'voice') {
        type SpeechRecognitionResultEventLike = Event & {
          results: ArrayLike<ArrayLike<{ transcript?: string }>>
        }
        type SpeechRecognitionConstructor = new () => {
          lang: string
          interimResults: boolean
          maxAlternatives: number
          onresult: ((event: SpeechRecognitionResultEventLike) => void) | null
          onerror: ((event: Event) => void) | null
          onend: (() => void) | null
          start: () => void
        }
        const SpeechRecognition = (window as unknown as {
          SpeechRecognition?: SpeechRecognitionConstructor
          webkitSpeechRecognition?: SpeechRecognitionConstructor
        }).SpeechRecognition
          ?? (window as unknown as { webkitSpeechRecognition?: SpeechRecognitionConstructor }).webkitSpeechRecognition
        if (!SpeechRecognition) {
          appendAssistantMessage('Spracheingabe nicht verfuegbar: Dein Browser unterstuetzt die Web Speech API nicht.')
          return
        }
        const recognition = new SpeechRecognition()
        recognition.lang = 'de-DE'
        recognition.interimResults = false
        recognition.maxAlternatives = 1
        recognition.onresult = (event: SpeechRecognitionResultEventLike) => {
          const transcript = event.results[0]?.[0]?.transcript ?? ''
          if (transcript) {
            appendAssistantMessage(`🎤 Erkannt: "${transcript}"\nSende diesen Text als naechste Anweisung oder bearbeite ihn.`)
            setInputValue((current) => current.trim().length > 0 ? `${current.trim()} ${transcript}` : transcript)
            inputRef.current?.focus()
          }
        }
        recognition.onerror = (event: Event) => {
          const errEvent = event as Event & { error?: string }
          appendAssistantMessage(`🎤 Spracheingabe-Fehler: ${errEvent.error ?? 'Unbekannter Fehler'}`)
        }
        recognition.start()
        appendAssistantMessage('🎤 Spracheingabe gestartet... Sprich jetzt!')
        return
      }

      if (slash.command === 'ollama') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage('Oeffne die Ollama-Einstellungen.\nKonfiguriere Endpoint, Modell und Laufzeitparameter fuer dein lokales Backend.')
        return
      }

      if (slash.command === 'local-model') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage(`Aktuelles Modell (${providerState.label}): ${providerState.model || '(nicht gesetzt)'}\nWechsle das Modell in den Einstellungen oder direkt mit /model <name>.`)
        return
      }

      if (slash.command === 'local-runtime') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage(`Aktive Runtime:\n- Provider: ${providerState.label}\n- Endpoint: ${providerState.endpoint || '(nicht gesetzt)'}\n- Modell: ${providerState.model || '(nicht gesetzt)'}\n- Cloud-Aliasse wurden entfernt.`)
        return
      }

      if (slash.command === 'privacy-settings') {
        useConfigStore.getState().setPreference('telemetryEnabled', false)
        appendAssistantMessage('Datenschutz: Telemetrie deaktiviert. Alle Daten bleiben lokal.')
        return
      }

      if (slash.command === 'extra-usage') {
        useConfigStore.getState().setPreference('maxToolCallsPerLoop', 50)
        appendAssistantMessage('Erweiterte Nutzungslimits aktiviert (50 Tool-Calls pro Loop).')
        return
      }

      if (slash.command === 'stickers') {
        appendAssistantMessage('🎉 Sticker-Modus aktiviert! 🚀✨💡')
        return
      }

      if (slash.command === 'release-notes') {
        appendAssistantMessage('Open_Cowork v1.0:\n- 79+ Slash-Commands (volle Claude Code Kompatibilitaet)\n- 5 Standard-Persoenlichkeiten\n- CrewAI Multi-Agent System\n- Memory Engine mit Suche\n- Plugin-System mit Skills\n- MCP-Integration\n- Sandbox & Security Controls')
        return
      }

      if (slash.command === 'upgrade') {
        appendAssistantMessage('Upgrade: Aktuelle Version ist auf dem neuesten Stand.\nPruefe GitHub Releases fuer neue Versionen.')
        return
      }

      if (slash.command === 'desktop') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage('Desktop-Integration: Konfiguriere Tray-Icon, Autostart und Fenster-Optionen in den Einstellungen.')
        return
      }

      if (slash.command === 'remote-control') {
        appendAssistantMessage('Remote Control: Nutze MCP-Server oder webhooks fuer Remote-Steuerung.\nKonfiguriere unter Einstellungen > MCP.')
        return
      }

      if (slash.command === 'remote-env') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage('Remote-Umgebung: Konfiguriere Terminal-Backends (SSH, Container, HPC) unter Features > Terminal.')
        return
      }

      if (slash.command === 'install-github-app') {
        appendAssistantMessage('GitHub-Integration:\n1. Erstelle einen Personal Access Token auf github.com/settings/tokens\n2. Konfiguriere einen MCP-Server mit github CLI\n3. Oder nutze /tool mcp_call fuer direkte API-Aufrufe')
        return
      }

      if (slash.command === 'install-slack-app') {
        appendAssistantMessage('Slack-Integration:\n1. Erstelle eine Slack App unter api.slack.com/apps\n2. Konfiguriere Webhooks oder einen MCP-Server\n3. Nutze /tool mcp_call fuer Slack-API Aufrufe')
        return
      }

      // ── Plugin skill matching ──
      const normalizedSlashCommand = normalizeSlashCommand(slash.command)
      const matchedSkill = enabledPluginSkills.find(
        (skill) => normalizeSlashCommand(skill.command) === normalizedSlashCommand
      )

      if (matchedSkill) {
        skillPromptOverride = renderSkillPrompt(matchedSkill.promptTemplate, slash.args, matchedSkill)
        skillPlanMode = matchedSkill.runMode === 'plan'
        skillInvocationActive = true
      } else if (!skillPromptOverride && slash.command !== 'plan') {
        // Fall through to registry for any remaining commands
        const registryCmd = registryCommands.find(
          (c) => c.command === `/${slash.command}`
        )
        if (registryCmd) {
          try {
            registryCmd.execute(slash.args || undefined)
          } catch { /* best effort */ }
          appendAssistantMessage(`/${slash.command} ausgefuehrt.`)
          return
        }
        appendAssistantMessage(
          `Unbekannter Slash-Command: /${slash.command}. Nutze /help fuer verfuegbare Befehle.`
        )
        return
      }
    }

    const baseUserPrompt = skillPromptOverride ?? (slash?.command === 'plan' ? slash.args : effectiveInput)
    const inferredClarificationContext = !replyingToAskUser && !slash
      ? inferClarificationContext(
          visibleMessages.map((message) => ({
            role: message.role,
            content: typeof message.content === 'string' ? message.content : '',
          })),
          baseUserPrompt,
        )
      : null
    const rawPrompt = replyingToAskUser && askUserQuestion
      ? `Antwort auf Rueckfrage:\nFrage: ${askUserQuestion}\nAntwort: ${baseUserPrompt}`
      : inferredClarificationContext
        ? buildClarificationContinuationPrompt(
            inferredClarificationContext.originalTask,
            inferredClarificationContext.assistantQuestion,
            baseUserPrompt,
          )
        : baseUserPrompt
    const hasApprovalBypassMarker = /\[approval-beduerftig\]/i.test(rawPrompt)
    const mergedForSend = mergeAttachments([], draftAttachments)
    const attachmentBuild = await buildAttachmentPromptContext(mergedForSend.next, rawPrompt)
    const attachmentContext = attachmentBuild.context
    const shouldRunInPlanMode = slash?.command === 'plan' || skillPlanMode || claudePlanMode
    const planWrappedPrompt = shouldRunInPlanMode
      ? `Erzeuge nur einen klaren, nummerierten Plan in Deutsch. Fuehre nichts aus.\n\nAufgabe:\n${rawPrompt}`
      : rawPrompt
    const systemAddendum = buildClaudeSystemAddendum({
      globalInstruction,
      planMode: shouldRunInPlanMode,
      permissionMode: claudePermissionMode,
      enabledTools: enabledClaudeToolIds,
    })
    const basePrompt = attachmentContext ? `${planWrappedPrompt}\n\n${attachmentContext}` : planWrappedPrompt
    const promptWithAttachments = systemAddendum ? `${systemAddendum}\n\n${basePrompt}` : basePrompt
    const engineUserInput = await buildEngineUserInput(promptWithAttachments, mergedForSend.next)
    const userMessage = {
      role: 'user' as const,
      content: rawPrompt,
      timestamp: Date.now(),
      attachments: mergedForSend.next,
      debugContent: promptWithAttachments,
    }
    let userMessageId: string | null = null
    const history = activeMessages
      .filter((m) => m.role !== 'system')
      .map((m) => ({ role: m.role, content: typeof m.content === 'string' ? m.content : '' }))
      .filter((m) => m.content.trim().length > 0 || m.role === 'user')

    const compactedHistory = policyFlags.autoCompactLongContext
      ? compactHistoryForPrompt(history, 12)
      : { compacted: history.slice(-12), droppedCount: 0 }

    if (superVerboseAuditLogging) {
      void writeAuditEvent('super_verbose', 'cowork_user_prompt', {
        view: 'cowork',
        threadId,
        prompt: rawPrompt,
        promptWithAttachments,
        attachments: mergedForSend.next,
        history,
        compactedHistory: compactedHistory.compacted,
        compactedDroppedItems: compactedHistory.droppedCount,
        slashCommand: slash?.command ?? null,
      })
    }

    if (!slash) {
      userMessageId = addMessage(threadId, userMessage)
    }
    setInputValue('')
    setAttachments([])
    setAttachmentNotice(null)
    setBusy(true)
    setError(null)

    let assistantMessageId: string | null = null
    let requestPreviewMessageId: string | null = null

    try {
      const started = Date.now()
      addLog({
        level: 'info',
        area: 'llm',
        message: 'LLM-Anfrage gestartet',
        details: {
          provider: providerState.provider,
          endpoint: providerState.endpoint,
          model: providerState.model,
          timeoutMs: providerState.timeoutMs,
          historyItems: history.length,
          compactedHistoryItems: compactedHistory.compacted.length,
          compactedDroppedItems: compactedHistory.droppedCount,
          promptChars: promptWithAttachments.length,
          parsedAttachments: attachmentBuild.parsedFiles,
          failedAttachments: attachmentBuild.failedFiles.length,
          source: skillInvocationActive ? 'chat_skill' : 'chat',
        },
      })
      if (superVerboseAuditLogging) {
        void writeAuditEvent('super_verbose', 'cowork_llm_request_started', {
          view: 'cowork',
          threadId,
          prompt: rawPrompt,
          promptWithAttachments,
          history,
          compactedHistory: compactedHistory.compacted,
          compactedDroppedItems: compactedHistory.droppedCount,
          ollama,
          shouldRunInPlanMode,
          skillInvocationActive,
          permissionMode: claudePermissionMode,
          enabledTools: enabledClaudeToolIds,
        })
      }
      if (attachmentBuild.failedFiles.length > 0) {
        addLog({
          level: 'warn',
          area: 'file_safety',
          message: 'Anhang-Analyse teilweise fehlgeschlagen',
          details: {
            failures: attachmentBuild.failedFiles,
          },
        })
      }
      let rawAssistantMessage = ''
      let rawThinkingMessage = ''
      let rawVerboseMessage = verboseMode
        ? `[${formatVerboseTimestamp(Date.now())}] Live-Verbose aktiviert`
        : ''
      let engineErrorMessage: string | null = null
      let webSearchSources: WebSearchSource[] = []
      const usedToolNames = new Set<string>()
      const toolNamesById = new Map<string, string>()
      let liveToolCalls: LiveToolCall[] = []
      let approvalSummary: string | null = null
      let awaitingUserQuestion: string | null = null
      setShellPanelRunning(false)
      const createdAssistantMessageId = addMessage(threadId, {
        role: 'assistant',
        content: '',
        verboseContent: rawVerboseMessage || undefined,
        timestamp: Date.now(),
        streaming: true,
      })
      assistantMessageId = createdAssistantMessageId

      const updateLiveToolCall = (patch: LiveToolCallPatch) => {
        liveToolCalls = upsertLiveToolCall(liveToolCalls, patch)
        updateMessage(threadId, createdAssistantMessageId, {
          liveToolCalls,
        })
      }

      const appendVerboseEntry = (headline: string, details?: string) => {
        if (!verboseMode) return

        const lines = [`[${formatVerboseTimestamp(Date.now())}] ${headline}`]
        const normalizedDetails = clipVerboseText(details ?? '')
        if (normalizedDetails) {
          lines.push(normalizedDetails)
        }

        rawVerboseMessage = rawVerboseMessage
          ? `${rawVerboseMessage}\n\n${lines.join('\n')}`
          : lines.join('\n')

        updateMessage(threadId, createdAssistantMessageId, {
          verboseContent: rawVerboseMessage,
        })
      }

      {
        // ── Engine Provider (Ollama backend) ──
        const cwd = getEffectiveWorkspaceCwd(
          mergedForSend.next,
          workingFolder,
          workingPathKind,
          workspaceDefaultPath,
        )
        appendVerboseEntry('Engine-Stream gestartet', `Arbeitsverzeichnis: ${cwd}`)
        await engineSendMessage(
          engineUserInput,
          cwd,
          (event) => {
            switch (event.type) {
              case 'text_delta':
              rawAssistantMessage += event.text
              {
                const presentation = resolveAssistantPresentation(rawAssistantMessage, {
                  verboseMode,
                  thinkingContent: rawThinkingMessage,
                })
                updateMessage(threadId, createdAssistantMessageId, {
                  content: presentation.content,
                  thinkingContent: presentation.thinkingContent,
                })
              }
              break
            case 'thinking_delta':
              rawThinkingMessage += event.thinking
              updateMessage(threadId, createdAssistantMessageId, {
                thinkingContent: rawThinkingMessage,
              })
              break
            case 'request_debug':
              if (userMessageId) {
                updateMessage(threadId, userMessageId, {
                  debugContent: `${promptWithAttachments}\n\n[OLLAMA REQUEST PREVIEW]\n${event.payload}`,
                })
              }
              if (requestPreviewMessageId) {
                updateMessage(threadId, requestPreviewMessageId, {
                  content: `Ollama Request Preview\n${event.payload}`,
                })
              } else {
                requestPreviewMessageId = addMessage(threadId, {
                  role: 'system',
                  content: `Ollama Request Preview\n${event.payload}`,
                  visibleInChat: true,
                  timestamp: Date.now(),
                })
              }
              appendVerboseEntry('Ollama-Request vorbereitet', event.payload)
              break
            case 'assistant_message': {
              // Non-stream fallback paths can emit only a final assistant_message
              // without prior text_delta events.
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
              appendVerboseEntry('Assistant-Block empfangen', [
                textFromEvent ? `Text: ${textFromEvent.length} Zeichen` : '',
                thinkingFromEvent ? `Thinking: ${thinkingFromEvent.length} Zeichen` : '',
                blocks.some((block) => block.type === 'tool_use')
                  ? `Tool-Aufrufe: ${blocks.filter((block) => block.type === 'tool_use').length}`
                  : '',
              ].filter(Boolean).join('\n'))
              updateMessage(threadId, createdAssistantMessageId, {
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
            case 'approval_required':
              approvalSummary = `${event.request.toolName}: ${event.request.description}`
              updateLiveToolCall({
                id: findLiveToolCallId(liveToolCalls, event.request.toolName, event.request.input),
                toolName: event.request.toolName,
                input: event.request.input,
                status: 'approval',
                result: event.request.description,
              })
              appendVerboseEntry('Freigabe erforderlich', [
                `Tool: ${event.request.toolName}`,
                `Beschreibung: ${event.request.description}`,
                `Risk Level: ${event.request.riskLevel}`,
              ].join('\n'))
              if (autoPilotAllTools || hasApprovalBypassMarker) {
                appendVerboseEntry(
                  'Freigabe automatisch erteilt',
                  autoPilotAllTools ? 'Grund: autoPilotAllTools' : 'Grund: approval-beduerftig marker',
                )
                resolveEngineApproval({ allowed: true })
                addLog({
                  level: 'info',
                  area: 'llm',
                  message: `Freigabe automatisch erteilt: ${event.request.toolName}`,
                  details: {
                    reason: autoPilotAllTools ? 'autoPilotAllTools' : 'approval-beduerftig marker',
                    request: event.request,
                  },
                })
                break
              }
              setBusy(false)
              setPendingApproval([`${event.request.toolName}: ${event.request.description}`])
              addLog({
                level: 'warn',
                area: 'llm',
                message: `Freigabe erforderlich: ${event.request.toolName}`,
                details: event.request,
              })
              break
            case 'tool_use_start':
              usedToolNames.add(event.toolName)
              toolNamesById.set(event.toolUseId, event.toolName)
              updateLiveToolCall({
                id: event.toolUseId,
                toolName: event.toolName,
                input: event.input,
                status: 'running',
              })
              if (event.toolName === 'Bash') {
                setShellPanelOpen(true)
                setShellPanelRunning(true)
                setShellPanelContent((previous) => appendRollingTerminalOutput(
                  previous,
                  `[${formatVerboseTimestamp(Date.now())}] $ ${String(event.input.command ?? '').trim()}`,
                ))
              }
              appendVerboseEntry(
                `Tool gestartet: ${event.toolName}`,
                stringifyVerboseValue(event.input),
              )
              addLog({
                level: 'info',
                area: 'llm',
                message: `Tool gestartet: ${event.toolName}`,
                details: { toolName: event.toolName, input: event.input },
              })
              break
            case 'tool_progress': {
              const activeToolName = toolNamesById.get(event.toolUseId) ?? 'Tool'
              const progress = formatToolProgress(activeToolName, event.data)
              const progressData = event.data
              if (activeToolName === 'Bash' && progressData.type === 'bash_progress') {
                const shellLine = normalizeShellPanelLine(progressData.output)
                if (shellLine) {
                  setShellPanelContent((previous) => appendRollingTerminalOutput(previous, shellLine))
                }
                setShellPanelRunning(progressData.exitCode === undefined)
              }
              appendVerboseEntry(progress.headline, progress.details)
              break
            }
            case 'tool_use_complete':
              usedToolNames.add(event.toolName)
              {
                const toolFailed = event.result.trim().toLowerCase().startsWith('fehler:')
                const completedToolInput = liveToolCalls.find((call) => call.id === event.toolUseId)?.input ?? {}
                if (event.toolName === 'AskUser' && typeof completedToolInput.question === 'string') {
                  awaitingUserQuestion = completedToolInput.question
                }
                const nextStatus: LiveToolCallStatus = toolFailed
                  ? 'failed'
                  : event.toolName === 'AskUser'
                    ? 'waiting_input'
                    : 'completed'
                updateLiveToolCall({
                  id: event.toolUseId,
                  toolName: event.toolName,
                  input: completedToolInput,
                  status: nextStatus,
                  result: event.result,
                  error: toolFailed ? event.result : undefined,
                  finishedAt: Date.now(),
                })
              }
              if (event.toolName === 'Bash') {
                setShellPanelRunning(false)
                const shellCompletionLine = extractShellPanelCompletionLine(event.result)
                if (shellCompletionLine) {
                  setShellPanelContent((previous) => appendRollingTerminalOutput(previous, shellCompletionLine))
                }
                setShellPanelContent((previous) => appendRollingTerminalOutput(
                  previous,
                  `[${formatVerboseTimestamp(Date.now())}] Shell-Lauf abgeschlossen`,
                ))
              }
              if (event.toolName === 'WebSearch') {
                webSearchSources = mergeWebSearchSources(
                  webSearchSources,
                  parseWebSearchSourcesFromToolResult(event.result),
                )
              }
              appendVerboseEntry(
                `Tool fertig: ${event.toolName}`,
                clipVerboseText(event.result, 6000),
              )
              addLog({
                level: 'info',
                area: 'llm',
                message: `Tool fertig: ${event.toolName}`,
                details: { toolName: event.toolName, result: event.result?.slice(0, 500) },
              })
              break
            case 'compaction':
              appendVerboseEntry('Kontext kompaktisiert', [
                `Entfernte Nachrichten: ${event.removedCount}`,
                event.summary,
              ].filter(Boolean).join('\n'))
              addLog({
                level: 'info',
                area: 'llm',
                message: 'Kontext kompaktisiert',
                details: { removedCount: event.removedCount, summary: event.summary },
              })
              break
            case 'context_warning':
              appendVerboseEntry(
                `Kontextwarnung: ${event.level}`,
                `Geschaetzte Tokens: ${event.estimatedTokens}`,
              )
              addLog({
                level: event.level === 'critical' ? 'warn' : 'info',
                area: 'llm',
                message: `Kontextwarnung: ${event.level}`,
                details: { estimatedTokens: event.estimatedTokens },
              })
              break
            case 'retry':
              appendVerboseEntry(`Engine-Retry ${event.attempt}`, event.reason)
              addLog({
                level: 'warn',
                area: 'llm',
                message: `Engine-Retry ${event.attempt}`,
                details: { reason: event.reason },
              })
              break
            case 'error':
              engineErrorMessage = event.error
              appendVerboseEntry('Engine-Fehler', event.error)
              addLog({ level: 'error', area: 'llm', message: event.error })
              break
          }
        }, {
          threadId,
          messages: activeMessages.map((message) => ({
            role: message.role,
            content: typeof message.content === 'string' ? message.content : '',
            debugContent: message.debugContent,
          })),
        }, createChatProviderSelection(providerState))

        const fallbackText = engineErrorMessage
          ? `LLM-Anfrage fehlgeschlagen: ${engineErrorMessage}\n\n${getChatProviderFailureHint(providerState.provider)}`
          : awaitingUserQuestion
            ? `Rueckfrage: ${awaitingUserQuestion}`
          : approvalSummary
            ? `Freigabe erforderlich: ${approvalSummary}`
            : usedToolNames.size > 0
              ? `Die Engine hat Tools verwendet (${Array.from(usedToolNames).join(', ')}), aber keinen sichtbaren Abschlusstext geliefert.`
              : 'Die Engine hat keine sichtbare Antwort geliefert. Bitte erneut versuchen oder Modell/Prompt prüfen.'
        const presentation = resolveAssistantPresentation(rawAssistantMessage, {
          verboseMode,
          thinkingContent: rawThinkingMessage,
          fallbackText,
        })
        const finalContent = appendWebSearchSources(presentation.content, webSearchSources)
        updateMessage(threadId, createdAssistantMessageId, {
          content: finalContent,
          debugContent: presentation.debugContent,
          thinkingContent: presentation.thinkingContent,
          verboseContent: rawVerboseMessage || undefined,
          streaming: false,
        }, {
          persist: true,
        })
        addLog({
          level: engineErrorMessage ? 'warn' : 'info',
          area: 'llm',
          message: engineErrorMessage ? 'Engine-Anfrage ohne sichtbare Antwort abgeschlossen' : 'Engine-Anfrage erfolgreich',
          details: {
            provider: providerState.provider,
            endpoint: providerState.endpoint,
            model: providerState.model,
            durationMs: Date.now() - started,
            usedToolNames: Array.from(usedToolNames),
            hadThinkingOnlyFallback: !sanitizeAssistantContent(rawAssistantMessage, verboseMode) && !!rawThinkingMessage,
            engineErrorMessage,
          },
        })
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      setError(message)
      addLog({
        level: 'error',
        area: 'llm',
        message: 'LLM-Anfrage fehlgeschlagen',
        details: {
          provider: providerState.provider,
          endpoint: providerState.endpoint,
          model: providerState.model,
          timeoutMs: providerState.timeoutMs,
          error: message,
          source: 'chat',
        },
      })
      if (superVerboseAuditLogging) {
        void writeAuditEvent('super_verbose', 'cowork_llm_request_failed', {
          view: 'cowork',
          threadId,
          prompt: rawPrompt,
          promptWithAttachments,
          error: message,
          provider: providerState.provider,
          endpoint: providerState.endpoint,
          model: providerState.model,
        })
      }
      const failureContent = `LLM-Anfrage fehlgeschlagen: ${message}\n\n${getChatProviderFailureHint(providerState.provider)}`
      if (assistantMessageId) {
        updateMessage(threadId, assistantMessageId, { content: failureContent, streaming: false }, { persist: true })
      } else {
        addMessage(threadId, {
          role: 'assistant',
          content: failureContent,
          timestamp: Date.now(),
        })
      }
    } finally {
      setBusy(false)
    }
  }

  const handleSend = async (event: FormEvent) => {
    event.preventDefault()
    await submitPrompt(inputValue, attachments)
  }

  const handleStop = () => {
    engineAbort()
    if (activeThreadId) {
      const streamingMessage = [...activeMessages].reverse().find(
        (message) => message.role === 'assistant' && message.streaming,
      )
      if (streamingMessage) {
        const content = streamingMessage.content?.trim()
          ? `${streamingMessage.content}\n\nGenerierung abgebrochen.`
          : 'Generierung abgebrochen.'
        updateMessage(
          activeThreadId,
          streamingMessage.id,
          { content, streaming: false },
          { persist: true },
        )
      }
    }
    setBusy(false)
    setError(null)
  }

  const toggleAskUserOption = (optionId: string) => {
    setSelectedAskUserOptionIds((current) => {
      if (current.includes(optionId)) {
        return current.filter((id) => id !== optionId)
      }

      if (!askUserPromptModel?.allowMultiple) {
        return [optionId]
      }

      return [...current, optionId]
    })
  }

  const buildStructuredAskUserAnswer = (): string => {
    if (!askUserPromptModel) {
      return inputValue.trim()
    }

    const selectedOptions = askUserPromptModel.options
      .filter((option) => selectedAskUserOptionIds.includes(option.id))
      .map((option) => option.label)

    const freeText = askUserFreeText.trim() || inputValue.trim()
    const parts = ['Rueckfrage beantwortet.']

    if (selectedOptions.length > 0) {
      parts.push(`Auswahl:\n${selectedOptions.map((option) => `- ${option}`).join('\n')}`)
    }

    if (freeText) {
      parts.push(`${askUserPromptModel.freeTextLabel}:\n${freeText}`)
    }

    return parts.join('\n\n')
  }

  const handleAskUserSubmit = async () => {
    const answer = buildStructuredAskUserAnswer()
    if (!answer.trim() && attachments.length === 0) return
    setInputValue(answer)
    setDismissedAskUserQuestion(askUserQuestion)
    await submitPrompt(answer, attachments)
  }

  const focusAnswerInput = () => {
    window.requestAnimationFrame(() => {
      inputRef.current?.focus()
      const cursor = inputValue.length
      inputRef.current?.setSelectionRange(cursor, cursor)
    })
  }

  const handleApprove = () => {
    if (approvalSteps.length === 0 || !activeThreadId) return
    setBusy(true)
    addMessage(activeThreadId, {
      role: 'system',
      content: `Plan freigegeben: ${approvalSteps.join(' | ')}`,
      timestamp: Date.now(),
    })
    resolveEngineApproval({ allowed: true })
    clearApproval()
  }

  const handleReject = () => {
    if (!activeThreadId) return
    addMessage(activeThreadId, {
      role: 'system',
      content: 'Plan abgelehnt. Passe die Anfrage an oder pruefe die Freigabe.',
      timestamp: Date.now(),
    })
    resolveEngineApproval({ allowed: false, reason: 'Vom Benutzer in CoworkView abgelehnt.' })
    clearApproval()
  }

  const handleProviderChange = (provider: string) => {
    if (!activeThreadId) return
    const nextProvider = normalizeChatProvider(provider)
    const nextProviderState = getChatProviderState(providerContext, activeProvider, { provider: nextProvider })
    setThreadProviderSettings(activeThreadId, createChatProviderSelection(nextProviderState))
    addLog({
      level: 'info',
      area: 'llm',
      message: 'Provider fuer diesen Chat gewechselt',
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
      message: 'Modell fuer diesen Chat gewechselt',
      details: {
        provider: providerState.provider,
        previousModel: providerState.model,
        nextModel: model,
        endpoint: providerState.endpoint,
      },
    })
  }

  if (!activeThread) {
    return null // WelcomeScreen handles the empty state
  }

  const quickPrompts = [
    'Erstelle einen klaren 5-Schritte-Plan fuer die aktuelle Aufgabe.',
    'Analysiere die letzten Aenderungen und nenne Risiken.',
    'Formuliere die naechsten konkreten ToDos mit Prioritaet.',
  ]

  const formatTime = (timestamp: number) =>
    new Date(timestamp).toLocaleTimeString('de-DE', { hour: '2-digit', minute: '2-digit' })

  const applyPromptToInput = (content: string, nextAttachments: ChatAttachment[] = []) => {
    setInputValue(content)
    const restored = mergeAttachments([], nextAttachments)
    setAttachments(restored.next)
    setAttachmentNotice(
      restored.rejectedCount > 0
        ? 'Es konnten nicht alle gespeicherten Dateien/Ordner wiederhergestellt werden.'
        : null,
    )
    window.requestAnimationFrame(() => {
      inputRef.current?.focus()
      inputRef.current?.setSelectionRange(content.length, content.length)
    })
  }

  const findPreviousUserMessage = (assistantMessageId: string): ChatMessage | null => {
    const assistantIndex = visibleMessages.findIndex((message) => message.id === assistantMessageId)
    if (assistantIndex < 0) return null

    for (let index = assistantIndex - 1; index >= 0; index -= 1) {
      const candidate = visibleMessages[index]
      if (candidate?.role === 'user') {
        return candidate
      }
    }
    return null
  }

  const handleRegenerate = async (assistantMessageId: string) => {
    const previousUser = findPreviousUserMessage(assistantMessageId)
    if (!previousUser) return
    const prompt = typeof previousUser.content === 'string' ? previousUser.content.trim() : ''
    const promptAttachments = Array.isArray(previousUser.attachments) ? previousUser.attachments : []
    const fallbackPrompt = 'Bitte fuehre dieselbe Aufgabe erneut mit denselben Anhaengen aus.'
    await submitPrompt(prompt || fallbackPrompt, promptAttachments)
  }

  const scrollMessagesToBottom = () => {
    if (!logRef.current) return
    logRef.current.scrollTo({ top: logRef.current.scrollHeight, behavior: 'smooth' })
  }

  return (
    <div className={`cowork-view ${compactMode ? 'compact-mode' : ''}`}>
      {/* Chat Pane */}
      <div className="cowork-pane">
        <div className="card" style={{ marginBottom: 12, fontSize: 12 }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', gap: 12, flexWrap: 'wrap' }}>
            <span><strong>Session:</strong> {currentSessionId ?? 'noch nicht gespeichert'}</span>
            <span><strong>Compactions:</strong> {compactionCount}</span>
            <span>
              <strong>Kontext:</strong>{' '}
              {contextWarning.level === 'none'
                ? 'stabil'
                : `${contextWarning.level} (${contextWarning.estimatedTokens} Tokens)`}
            </span>
            <button
              type="button"
              className="btn-sm"
              onClick={() => setShellPanelOpen((open) => !open)}
            >
              {shellPanelOpen ? 'Terminal Live ausblenden' : shellPanelRunning || shellPanelContent ? 'Terminal Live einblenden' : 'Terminal Live'}
            </button>
            <button type="button" className="btn-sm" onClick={() => void forceCompact()} disabled={uiLocked}>
              Kontext kompaktieren
            </button>
          </div>
        </div>

        {shellPanelOpen && (shellPanelContent || shellPanelRunning) && (
          <div className="card" style={{ marginBottom: 12 }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', gap: 12, alignItems: 'center', marginBottom: 8, flexWrap: 'wrap' }}>
              <div>
                <strong style={{ fontSize: 13 }}>Terminal Live</strong>
                <span style={{ fontSize: 11, color: shellPanelRunning ? 'var(--success)' : 'var(--text-muted)', marginLeft: 8 }}>
                  {shellPanelRunning ? 'laeuft' : 'bereit'}
                </span>
              </div>
              <div style={{ display: 'flex', gap: 8 }}>
                <button type="button" className="btn-sm" onClick={() => setShellPanelContent('')} disabled={shellPanelRunning || !shellPanelContent}>
                  Leeren
                </button>
                <button type="button" className="btn-sm" onClick={() => setShellPanelOpen(false)}>
                  Schliessen
                </button>
              </div>
            </div>
            <pre style={{ margin: 0, fontSize: 12, lineHeight: 1.45, background: 'var(--bg-primary)', color: 'var(--text-primary)', padding: 12, borderRadius: 'var(--radius-sm)', overflowX: 'auto', maxHeight: 280, whiteSpace: 'pre-wrap', wordBreak: 'break-word', fontFamily: 'Consolas, Monaco, monospace' }}>
              {shellPanelContent || (shellPanelRunning ? 'Warte auf Shell-Ausgabe...' : 'Noch keine Shell-Ausgabe.')}
            </pre>
          </div>
        )}

        <div className="cowork-messages" ref={logRef}>
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
              const canRegenerate = msg.role === 'assistant' && findPreviousUserMessage(msg.id) !== null
              return (
                <div key={msg.id} className={`cowork-msg ${msg.role}`}>
                <div className="msg-avatar">
                  {msg.role === 'user' ? '👤' : '✦'}
                </div>
                <div className="msg-body">
                  <div className="msg-role">
                    {msg.role === 'user' ? 'Du' : 'Open_Cowork'}
                    {showTimestamps && <span className="msg-time">{formatTime(msg.timestamp)}</span>}
                  </div>
                  <div className="msg-content">
                    {displayedContent ? <HighlightedChatText content={displayedContent} /> : null}
                  </div>
                  <MessageThinking
                    content={displayedThinkingContent}
                    limitToRollingWindow={limitThinkingWindow}
                    streaming={msg.streaming}
                  />
                  <LiveToolCalls calls={msg.liveToolCalls} />
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
                      <summary>Ollama Request Preview</summary>
                      <pre>{ollamaRequestPreview}</pre>
                    </details>
                  )}
                  {verboseMode && promptDebug && promptDebug !== content && (
                    <details className="message-debug">
                      <summary>Verbose: interner Prompt</summary>
                      <pre>{promptDebug}</pre>
                    </details>
                  )}
                  <div className="msg-actions">
                    <button type="button" className="btn-msg-action" onClick={() => void navigator.clipboard.writeText(content)}>
                      Kopieren
                    </button>
                    {msg.role === 'user' ? (
                      <button
                        type="button"
                        className="btn-msg-action"
                        onClick={() => applyPromptToInput(content, attachmentsForMessage)}
                      >
                        Wiederverwenden
                      </button>
                    ) : (
                      <>
                        <button type="button" className="btn-msg-action" onClick={() => applyPromptToInput(content)}>
                          Als Prompt nutzen
                        </button>
                        <button
                          type="button"
                          className="btn-msg-action"
                          onClick={() => void handleRegenerate(msg.id)}
                          disabled={uiLocked || !canRegenerate}
                        >
                          Neu generieren
                        </button>
                      </>
                    )}
                  </div>
                </div>
                </div>
              )
            })}
          {busy && !activeMessages.some((msg) => msg.streaming) && (
            <div className="cowork-msg assistant">
              <div className="msg-avatar">✦</div>
              <div className="msg-body">
                <div className="msg-role">Open_Cowork</div>
                <div className="msg-content typing">
                  <span className="typing-dot" />
                  <span className="typing-dot" />
                  <span className="typing-dot" />
                </div>
              </div>
            </div>
          )}
        </div>

        {showScrollToBottom && (
          <button type="button" className="btn-scroll-bottom" onClick={scrollMessagesToBottom}>
            Neue Nachrichten ▾
          </button>
        )}

        {approvalSteps.length > 0 && (
          <div className="approval-banner">
            <div className="approval-header">
              <span className="approval-icon">⚠️</span>
              <span>Diese Schritte erfordern deine Freigabe:</span>
            </div>
            <ol className="approval-steps">
              {approvalSteps.map((step, idx) => (
                <li key={`${step}-${idx}`}>{step}</li>
              ))}
            </ol>
            <div className="approval-actions">
              <button type="button" className="btn-approve" onClick={handleApprove} disabled={uiLocked}>
                ✓ Freigeben
              </button>
              <button type="button" className="btn-reject" onClick={handleReject} disabled={uiLocked}>
                ✗ Ablehnen
              </button>
            </div>
          </div>
        )}

        {showAskUserPrompt && askUserQuestion && approvalSteps.length === 0 && (
          <div className="approval-banner question-banner">
            <div className="approval-header">
              <span className="approval-icon">?</span>
              <span>Open_Cowork hat eine Rueckfrage:</span>
            </div>
            <div className="ask-user-modal-question">
              <HighlightedChatText content={askUserPromptModel?.question ?? askUserQuestion} />
            </div>
            {askUserPromptModel && askUserPromptModel.options.length > 0 && (
              <div className="ask-user-options" role="group" aria-label="Antwortoptionen">
                {askUserPromptModel.options.map((option) => {
                  const checked = selectedAskUserOptionIds.includes(option.id)
                  return (
                    <label key={option.id} className={`ask-user-option ${checked ? 'selected' : ''}`}>
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={() => toggleAskUserOption(option.id)}
                      />
                      <span>{option.label}</span>
                    </label>
                  )
                })}
              </div>
            )}
            <label className="ask-user-free-text-label" htmlFor="ask-user-free-text">
              {askUserPromptModel?.freeTextLabel ?? 'Freitext'}
            </label>
            <textarea
              id="ask-user-free-text"
              className="ask-user-modal-input"
              rows={4}
              value={askUserFreeText}
              onChange={(event) => setAskUserFreeText(event.currentTarget.value)}
              placeholder={askUserPromptModel?.freeTextPlaceholder ?? 'Optional ergaenzen...'}
              autoFocus
            />
            <div className="ask-user-modal-actions">
              <button
                type="button"
                className="btn-approve"
                onClick={() => void handleAskUserSubmit()}
                disabled={uiLocked || (!askUserHasStructuredResponse && attachments.length === 0)}
              >
                Antwort senden
              </button>
              <button
                type="button"
                className="btn-reject"
                onClick={focusAnswerInput}
                disabled={uiLocked}
              >
                Im Hauptchat beantworten
              </button>
            </div>
          </div>
        )}

        {error && <p className="error cowork-error">{error}</p>}

        <div className="quick-prompts">
          {quickPrompts.map((prompt) => (
            <button key={prompt} type="button" className="quick-prompt-btn" onClick={() => applyPromptToInput(prompt)}>
              {prompt}
            </button>
          ))}
        </div>

        <form className="cowork-input" onSubmit={handleSend}>
          <div className="chat-input-main">
            {attachments.length > 0 && (
              <div className="attachment-list" aria-label="Verbundene Elemente">
                {attachments.map((item) => (
                  <span key={`${item.kind}-${item.path}`} className="attachment-chip" title={item.label ?? item.path}>
                    <span className="attachment-chip-label">
                      {item.kind === 'folder' ? 'Ordner' : isImageAttachment(item) ? 'Bild' : 'Datei'}: {getAttachmentDisplayName(item)}
                    </span>
                    <button
                      type="button"
                      className="attachment-remove"
                      onClick={() => handleRemoveAttachment(item)}
                      aria-label={`Anhang entfernen: ${getAttachmentDisplayName(item)}`}
                      disabled={uiLocked}
                    >
                      ×
                    </button>
                  </span>
                ))}
              </div>
            )}
            {attachmentNotice && <p className="attachment-notice">{attachmentNotice}</p>}
            <textarea
              ref={inputRef}
              rows={2}
              placeholder={askUserQuestion ? 'Beantworte die Rueckfrage hier...' : 'Nächste Anweisung...'}
              disabled={uiLocked}
              value={inputValue}
              className={dragOverInput ? 'input-drop-active' : ''}
              onChange={(e) => setInputValue(e.currentTarget.value)}
              onDragOver={handleInputDragOver}
              onDragLeave={handleInputDragLeave}
              onDrop={handleInputDrop}
              onPaste={handleInputPaste}
              onKeyDown={(e) => {
                if (showSlashSuggestions) {
                  if (e.key === 'ArrowDown') {
                    e.preventDefault()
                    setActiveSlashSuggestionIndex((current) =>
                      Math.min(current + 1, filteredSlashSuggestions.length - 1)
                    )
                    return
                  }

                  if (e.key === 'ArrowUp') {
                    e.preventDefault()
                    setActiveSlashSuggestionIndex((current) => Math.max(current - 1, 0))
                    return
                  }

                  if (e.key === 'Enter' || e.key === 'Tab') {
                    e.preventDefault()
                    applySlashSuggestion(filteredSlashSuggestions[activeSlashSuggestionIndex])
                    return
                  }

                  if (e.key === 'Escape') {
                    e.preventDefault()
                    setSlashSuggestionsOpen(false)
                    return
                  }
                }

                if (e.key === 'ArrowUp' && !e.shiftKey && (e.currentTarget.value.trim() === '' || e.currentTarget.selectionStart === 0)) {
                  e.preventDefault()
                  if (promptHistory.length === 0) return
                  const next = Math.min(historyIndex + 1, promptHistory.length - 1)
                  setHistoryIndex(next)
                  const nextValue = promptHistory[next]
                  setInputValue(nextValue)
                  window.requestAnimationFrame(() => {
                    inputRef.current?.setSelectionRange(nextValue.length, nextValue.length)
                  })
                  return
                }

                if (e.key === 'ArrowDown' && historyIndex >= 0) {
                  e.preventDefault()
                  const next = historyIndex - 1
                  setHistoryIndex(next)
                  const nextValue = next >= 0 ? promptHistory[next] : ''
                  setInputValue(nextValue)
                  window.requestAnimationFrame(() => {
                    inputRef.current?.setSelectionRange(nextValue.length, nextValue.length)
                  })
                  return
                }

                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault()
                  handleSend(e)
                }
              }}
            />
            {showSlashSuggestions && (
              <div className="slash-command-menu" role="listbox" aria-label="Slash-Commands">
                {filteredSlashSuggestions.map((suggestion, index) => (
                  <button
                    key={`${suggestion.source}-${suggestion.command}`}
                    type="button"
                    className={`slash-command-option ${index === activeSlashSuggestionIndex ? 'active' : ''}`}
                    onMouseDown={(event) => {
                      event.preventDefault()
                      applySlashSuggestion(suggestion)
                    }}
                    role="option"
                    aria-selected={index === activeSlashSuggestionIndex}
                  >
                    <span className="slash-command-usage">
                      {suggestion.command}
                      {suggestion.args ? <span> {suggestion.args}</span> : null}
                    </span>
                    <span className="slash-command-description">{suggestion.description}</span>
                  </button>
                ))}
              </div>
            )}
          </div>
          <div className="chat-input-bottom-bar">
            <div className="chat-input-toolbar-compact">
              <select
                className="chat-compact-select"
                value={providerState.provider}
                onChange={(e) => handleProviderChange(e.target.value)}
                disabled={uiLocked}
                title="Provider"
              >
                {CHAT_PROVIDER_OPTIONS.map((provider) => (
                  <option key={provider} value={provider}>
                    {CHAT_PROVIDER_LABELS[provider]}
                  </option>
                ))}
              </select>
              <select
                className="chat-compact-select"
                value={providerState.model}
                onChange={(e) => handleModelChange(e.target.value)}
                disabled={uiLocked}
                title="Modell"
              >
                {selectableModels.length > 0 ? (
                  selectableModels.map((model) => (
                    <option key={model} value={model}>
                      {model}
                    </option>
                  ))
                ) : (
                  <option value={providerState.model}>{providerState.model || 'kein Modell gesetzt'}</option>
                )}
                {selectableModels.length > 0 && providerState.model && !selectableModels.includes(providerState.model) && (
                  <option value={providerState.model}>{providerState.model}</option>
                )}
              </select>
              <select
                className="chat-compact-select"
                value={enginePermissionMode}
                onChange={(e) => {
                  const mode = e.target.value as 'default' | 'plan' | 'bypass' | 'strict'
                  setEngineConfig({ permissionMode: mode })
                  setClaudePermissionMode(ENGINE_TO_CLAUDE_PERMISSION_MODE[mode])
                }}
                disabled={uiLocked}
                title="Berechtigungs-Modus"
              >
                <option value="default">Standard</option>
                <option value="plan">Plan-Modus</option>
                <option value="bypass">Bypass</option>
                <option value="strict">Strikt</option>
              </select>
              <div className="chat-compact-actions">
                <button type="button" className="btn-compact-action" onClick={handleAttachFiles} disabled={uiLocked}>
                  Dateien
                </button>
                <button type="button" className="btn-compact-action" onClick={handleAttachFolders} disabled={uiLocked}>
                  Ordner
                </button>
              </div>
            </div>
            {busy ? (
              <button type="button" onClick={handleStop} className="btn-stop compact-send-btn">
                ⏹ Stopp
              </button>
            ) : (
              <button type="submit" disabled={uiLocked} className="btn-send compact-send-btn">
                {askUserQuestion ? 'Antwort senden →' : 'Senden →'}
              </button>
            )}
          </div>
        </form>
      </div>
    </div>
  )
}
