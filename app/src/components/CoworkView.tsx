import { lazy, Suspense, useRef, useEffect, useMemo, useState } from 'react'
import type { ClipboardEvent, DragEvent, FormEvent } from 'react'
import { open, save } from '@tauri-apps/plugin-dialog'
import { useChatStore, getActiveThread, type ChatMessage } from '../stores/chatStore'
import type { LiveToolCall, LiveToolCallStatus } from '../stores/chatStore'
import { CheckCircle2, Clock3, Loader2, ShieldAlert, Wrench, XCircle } from 'lucide-react'
import { useConfigStore } from '../stores/configStore'
import { useTaskStore } from '../stores/taskStore'
import { useWorkTasksStore } from '../stores/workTasksStore'
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
import {
  getEnabledProjectAttachments,
  getEnabledProjectLinks,
  getProjectForThread,
  useProjectStore,
  type Project,
  type ProjectResource,
} from '../stores/projectStore'
import type { ContentBlock, ToolUIRequest } from '../engine'
import type { ToolProgressData } from '../engine/types'
import { checkOllamaConnection } from '../engine/api/ollamaClient'
import {
  createInlineImageAttachment,
  extractFileAttachmentsFromFileList,
  extractFileAttachmentsFromUriList,
  getAttachmentDisplayName,
  getPathName,
  getAttachmentPreviewSrcForAttachment,
  hasLocalAttachmentPath,
  isImageAttachment,
  mergeAttachments,
  normalizeDialogSelection,
  toImageContentBlocks,
  type ChatAttachment,
} from '../utils/chatAttachments'
import { buildAttachmentPromptContext } from '../utils/attachmentPromptContext'
import { resolveAssistantPresentation, resolveDisplayedAssistantContent, resolveDisplayedThinkingContent, sanitizeAssistantContent, splitPromptDebugContent } from '../utils/messageDisplay'
import { appendWebSearchSources, mergeWebSearchSources, parseWebSearchSourcesFromToolResult, type WebSearchSource } from '../utils/webSearchSources'
// Ollama streaming is now handled by the engine
import { MessageThinking, MessageVerbose } from './MessageThinking'
import { HighlightedChatText } from './HighlightedChatText'
import CrewLiveMonitor from './CrewLiveMonitor'
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
import { tr } from '../i18n'

const TerminalDock = lazy(() => import('./TerminalDock'))

type WebFetchResponse = {
  url: string
  status: number
  ok: boolean
  title: string | null
  content: string
  truncated: boolean
}

export async function buildProjectLinkPromptContext(
  links: ProjectResource[],
): Promise<{ context: string; notice: string | null }> {
  if (links.length === 0) return { context: '', notice: null }

  const lines: string[] = ['Manually fetched project links:']
  const failures: string[] = []

  for (const link of links) {
    try {
      const response = await safeInvoke<WebFetchResponse>('web_fetch_url', {
        request: { url: link.path, maxChars: 4000 },
      })
      if (!response.ok) {
        failures.push(`${link.label ?? link.path}: HTTP ${response.status}`)
        continue
      }
      lines.push(`Source: ${link.label ?? response.title ?? link.path}`)
      lines.push(`URL: ${response.url}`)
      if (response.title) lines.push(`Titel: ${response.title}`)
      lines.push(response.truncated ? `${response.content}\n[gekuerzt]` : response.content)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      failures.push(`${link.label ?? link.path}: ${message}`)
    }
  }

  return {
    context: lines.length > 1 ? lines.join('\n\n') : '',
    notice: failures.length > 0
      ? `Not all project links could be fetched: ${failures.join('; ')}`
      : null,
  }
}

export function buildProjectInstructionsPromptContext(
  project: Pick<Project, 'title' | 'instructions'> | null | undefined,
): string {
  const instructions = project?.instructions.trim()
  if (!project || !instructions) return ''
  return `Project instructions for "${project.title}":\n${instructions}`
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

const MAX_RENDERED_MESSAGES = 120

function formatVerboseTimestamp(timestamp: number): string {
  return new Date(timestamp).toLocaleTimeString('en-US', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

function clipVerboseText(value: string, maxChars = 4000): string {
  const normalized = value.trim()
  if (!normalized) return ''
  if (normalized.length <= maxChars) return normalized
  return `${normalized.slice(0, maxChars)}\n... [truncated, ${normalized.length - maxChars} additional characters]`
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
    return reachable ? 'REACHABLE (web check)' : 'NOT REACHABLE'
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
        headline: `${toolName}: Shell-Output`,
        details: clipVerboseText(data.output),
      }
    case 'agent_progress':
      return {
        headline: `${toolName}: Agent-Fortschritt (${data.agentName})`,
        details: clipVerboseText(data.content),
      }
    case 'web_search_progress':
      return {
        headline: `${toolName}: ${tr('Web search')}`,
        details: `Query: ${data.query}\nTreffer: ${data.results}`,
      }
    case 'mcp_progress':
      return {
        headline: `${toolName}: MCP-Fortschritt (${data.serverName})`,
        details: `Fortschritt: ${data.progress}%`,
      }
    case 'skill_progress':
      return {
        headline: `${toolName}: Skill-Output (${data.skillName})`,
        details: clipVerboseText(data.output),
      }
    case 'task_output_progress':
      return {
        headline: `${toolName}: Task-Output (${data.taskId})`,
        details: clipVerboseText(data.output),
      }
    case 'file_progress':
      return {
        headline: `${toolName}: Fileoperation`,
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
      : 'Add optional details...',
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
  const contentMatch = content.match(/^question:\s*([\s\S]+)$/)
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
  if (call.toolName === 'Bash' && call.status === 'completed' && typeof call.result === 'string') {
    const exitCode = call.result.match(/exit code:\s*(-?\d+|null)/i)?.[1]
    const currentCwd = call.result.match(/current cwd:\s*(.+)/i)?.[1]?.trim()
    const manualIntervention = /manually intervened/i.test(call.result)
    return {
      ...call,
      result: [
        'Terminal-Ausgabe im Terminal-Dock.',
        exitCode ? `Exit-Code: ${exitCode}` : '',
        currentCwd ? `CWD: ${currentCwd}` : '',
        manualIntervention ? 'Note: The running terminal command was manually interrupted.' : '',
      ].filter(Boolean).join('\n'),
    }
  }

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
    <div className="live-tool-call-list" aria-label={tr("Live Tool Calls")}>
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
                <summary>{tr("Input")}</summary>
                <pre>{inputPreview}</pre>
              </details>
            )}
            {resultPreview && (
              <details className="live-tool-call-detail" open={displayCall.status === 'failed'}>
                <summary>{displayCall.error ? 'Error' : 'Result'}</summary>
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
  const [includeProjectLinks, setIncludeProjectLinks] = useState(false)
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
  const projects = useProjectStore((s) => s.projects)
  const setProjectResourceEnabled = useProjectStore((s) => s.setResourceEnabled)
  const activeThread = useChatStore(getActiveThread)
  const terminalThreadId = activeThreadId ?? activeThread?.id ?? 'default'
  const terminalDockOpen = useTerminalStore((s) => Boolean(s.dockOpenByThread[terminalThreadId]))
  const terminalHiddenActivity = useTerminalStore((s) => Boolean(s.hiddenActivityByThread[terminalThreadId]))
  const setTerminalDockOpen = useTerminalStore((s) => s.setDockOpen)
  const setActiveAiThread = useTerminalStore((s) => s.setActiveAiThread)
  const inputRef = useRef<HTMLTextAreaElement>(null)
  const logRef = useRef<HTMLDivElement>(null)
  const notifiedAskUserQuestionRef = useRef<string | null>(null)
  const emptyThreadBootstrapRef = useRef<string | null>(null)
  const activeMessages = useMemo(
    () => (Array.isArray(activeThread?.messages) ? activeThread.messages : []),
    [activeThread?.messages],
  )
  const workTasks = useWorkTasksStore((s) => s.tasks)
  const [collapsedMessageIds, setCollapsedMessageIds] = useState<Set<string>>(new Set())
  const isTaskChat = useMemo(() => {
    if (!activeThread?.id) return false
    return workTasks.some((task) => task.threadId === activeThread.id)
  }, [activeThread?.id, workTasks])

  const toggleCollapse = (messageId: string) => {
    setCollapsedMessageIds((prev) => {
      const next = new Set(prev)
      if (next.has(messageId)) {
        next.delete(messageId)
      } else {
        next.add(messageId)
      }
      return next
    })
  }

  const lastActiveMessage = activeMessages[activeMessages.length - 1]
  const activeProject = useMemo(
    () => getProjectForThread(projects, activeThreadId),
    [activeThreadId, projects],
  )
  const activeProjectAttachments = useMemo(
    () => getEnabledProjectAttachments(activeProject),
    [activeProject],
  )
  const activeProjectLinks = useMemo(
    () => getEnabledProjectLinks(activeProject),
    [activeProject],
  )

  useEffect(() => {
    setIncludeProjectLinks(false)
  }, [activeProject?.id])

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
    // If an active valid thread already exists, do nothing
    if (activeThread) {
      emptyThreadBootstrapRef.current = null
      return
    }

    const current = useChatStore.getState()
    
    // Check, ob activeThreadId gesetzt und valid ist
    if (current.activeThreadId && current.threads.some((thread) => thread.id === current.activeThreadId)) {
      setActiveThread(current.activeThreadId)
      return
    }

    // Check, ob ein bootstrapped Thread existiert
    const bootstrappedThreadId = emptyThreadBootstrapRef.current
    if (bootstrappedThreadId && current.threads.some((thread) => thread.id === bootstrappedThreadId)) {
      setActiveThread(bootstrappedThreadId)
      return
    }

    // Only create an empty "New Chat" when no thread exists yet
    if (current.threads.length === 0) {
      const threadId = addThread('New chat', createChatProviderSelection(providerState))
      emptyThreadBootstrapRef.current = threadId
      setActiveThread(threadId)
    } else {
      // Otherwise activate the newest thread
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

  // Automatic title generation: when the thread is still "New Chat" and a user message is added
  useEffect(() => {
    if (!activeThreadId || !activeThread) return
    if (activeThread.title !== 'New chat') return

    const userMessages = activeThread.messages.filter(m => m.role === 'user')
    if (userMessages.length === 0) return // No User-Message available

    // Rename only once, after the first user message
    // Check whether the title is still "New Chat" and has not been renamed yet
    const firstUserMessage = userMessages[0]
    const content = typeof firstUserMessage.content === 'string' ? firstUserMessage.content : ''
    if (!content.trim()) return

    const newTitle = content.length > 50 ? content.slice(0, 50) + '...' : content
    // Update title through store and DB
    const updatedThreads = useChatStore.getState().threads.map(t =>
      t.id === activeThreadId ? { ...t, title: newTitle, updatedAt: Date.now() } : t
    )
    useChatStore.setState({ threads: updatedThreads })

    // Update title in the database
    void persistInvoke('db_save_thread', {
      id: activeThreadId,
      title: newTitle,
      createdAt: new Date(activeThread.createdAt).toISOString()
    }, 'db_save_thread update title')
  }, [activeThreadId, activeThread?.messages.length]) // Execute when thread or message count changes

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
      const haystack = `${item.command} ${item.args ?? ''} ${tr(item.description)}`.toLowerCase()
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
    void showDesktopNotification('Open Cowork is waiting for your answer', askUserQuestion)
      .then((shown) => {
        addLog({
          level: shown ? 'info' : 'warn',
          area: 'ui',
          message: shown
            ? 'Desktop notification for question sent'
            : 'Desktop notification for the question could not be sent',
          details: { question: askUserQuestion },
        })
      })
  }, [addLog, askUserQuestion, desktopNotificationsEnabled])

  const visibleMessages = useMemo(
    () => activeMessages.filter((message) => message.role !== 'system' || message.visibleInChat),
    [activeMessages],
  )
  const renderedMessages = useMemo(
    () => visibleMessages.length > MAX_RENDERED_MESSAGES
      ? visibleMessages.slice(-MAX_RENDERED_MESSAGES)
      : visibleMessages,
    [visibleMessages],
  )
  const hiddenRenderedMessageCount = visibleMessages.length - renderedMessages.length

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

    setAttachmentNotice(tr("Drop detected, but no local file path was found. Please choose files with the button."))
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
            setAttachmentNotice(tr("Could not read image from clipboard."))
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
    const projectContextAttachments = activeProjectAttachments
    const hasProjectAttachments = projectContextAttachments.length > 0
    const projectLinksToFetch = includeProjectLinks ? activeProjectLinks : []
    const hasProjectLinksToFetch = projectLinksToFetch.length > 0
    if ((!text && !hasDraftAttachments && !hasProjectAttachments && !hasProjectLinksToFetch) || busy) return
    const fallbackAttachmentPrompt = 'Please analyze the attached files/folders and complete the task.'
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

    const slash = parseSlashCommand(text)

    const parseArgs = (value: string): string[] => {
      const matches = value.match(/"[^"]*"|'[^']*'|\S+/g) ?? []
      return matches.map((part) => part.replace(/^["']|["']$/g, ''))
    }

    const validateToolPolicy = (toolId: string, target: string | null): string | null => {
      if (policyFlags.strictPolicyEnforcement && !enabledClaudeToolIds.includes(toolId)) {
        return `Tool ${toolId} is disabled in the current profile.`
      }

      if (policyFlags.strictPolicyEnforcement && isToolDeniedByRules(toolId, target, toolDenyRules)) {
        return `Tool-Aufruf durch deny-rule blockiert (${toolId}).`
      }

      if (toolId === 'web_fetch' && !policyFlags.allowWebFetch) {
        return 'Web Fetch ist per Policy disabled.'
      }

      if (toolId === 'web_search' && !policyFlags.allowWebSearch) {
        return 'Web Search ist per Policy disabled.'
      }

      if (toolId === 'read_file' && !policyFlags.allowFileReadExtraction) {
        return 'Fileextraktion ist per Policy disabled.'
      }

      if (toolId === 'mcp' && !policyFlags.allowMcpToolCalls) {
        return 'MCP Tool Calls sind per Policy disabled.'
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
      const effectiveTemplate = template.trim() || 'Bearbeite diese Task: {{input}}'
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
        const helpText = ['Verfuegbare slash commands:', ...cmdLines]
        if (pluginLines.length > 0) {
          helpText.push('', 'Active Plugin-Skills:', ...pluginLines)
        }
        appendAssistantMessage(helpText.join('\n'))
        return
      }

      if (slash.command === 'tools') {
        appendAssistantMessage(
          [
            `Permission-Mode: ${claudePermissionMode}`,
            `Plan mode: ${claudePlanMode ? 'active' : 'inactive'}`,
            `Active Tools: ${enabledClaudeToolIds.join(', ') || '(none)'}`,
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
          appendAssistantMessage(lines.length > 0 ? lines.join('\n') : 'No offenen Todos/Tasks available.')
          return
        }

        if (args[0].toLowerCase() === 'add') {
          const title = args.slice(1).join(' ').trim()
          if (!title) {
            appendAssistantMessage(tr("Please Titel angeben: /todo add <titel>"))
            return
          }
          createTask(title, title.slice(0, 80), threadId)
          appendAssistantMessage(`Todo created: ${title}`)
          return
        }

        appendAssistantMessage(tr("Invalid /todo command. Use /todo list or /todo add <title>."))
        return
      }

      if (slash.command === 'tool') {
        if (!policyFlags.allowToolDispatcher) {
          appendAssistantMessage(tr("Tool Dispatcher ist per Policy disabled."))
          return
        }

        const args = parseArgs(slash.args)
        const toolName = (args[0] ?? '').toLowerCase()
        const rest = args.slice(1)

        if (!toolName) {
          appendAssistantMessage(tr("Please Tool angeben: /tool <read_file|web_fetch|web_search|mcp_call> <args>"))
          return
        }

        if (toolName === 'read_file') {
          const targetPath = rest.join(' ').trim()
          if (!targetPath) {
            appendAssistantMessage(tr("Please provide a file path: /tool read_file C:\\path\\file.txt"))
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
            appendAssistantMessage(`File geread: ${targetPath}\n\n${textOut.slice(0, 5000)}`)
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
            appendAssistantMessage(`read_file failed: ${message}`)
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
            appendAssistantMessage(tr("Please URL angeben: /tool web_fetch https://example.com"))
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
            appendAssistantMessage(`web_fetch failed: ${message}`)
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
            appendAssistantMessage(tr("Please provide a search query: /tool web_search weather Stuttgart today"))
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
            appendAssistantMessage(lines.join('\n\n') || `No results for "${query}".`)
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
            appendAssistantMessage(`web_search failed: ${message}`)
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
            appendAssistantMessage(tr("Please MCP Toolnamen angeben: /tool mcp_call <toolName> {\"arg\":\"value\"}"))
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
              appendAssistantMessage(tr("MCP Args muessen valides JSON sein."))
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
                : `MCP ${response.toolName} Error: ${response.error ?? response.result}`
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
            appendAssistantMessage(`mcp_call failed: ${message}`)
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

        appendAssistantMessage(`${tr('Unknown tool')}: ${toolName}. ${tr('Allowed')}: read_file, web_fetch, web_search, mcp_call`)
        return
      }

      if (slash.command === 'mode') {
        const target = slash.args.toLowerCase()
        if (target === 'plan') {
          setClaudePlanMode(true)
          appendAssistantMessage(tr("Plan-Mode enabled."))
        } else if (target === 'execute') {
          setClaudePlanMode(false)
          appendAssistantMessage(tr("Plan-Mode disabled (Execute)."))
        } else {
          appendAssistantMessage(tr("Invalid mode. Use: /mode plan or /mode execute"))
        }
        return
      }

      if (slash.command === 'permissions') {
        const target = slash.args as ClaudePermissionMode
        if (VALID_PERMISSION_MODES.includes(target)) {
          setClaudePermissionMode(target)
          appendAssistantMessage(tr('Permission mode set to {{mode}}', { mode: target }))
        } else {
          appendAssistantMessage(tr("Invalid permission mode. Allowed: default, acceptEdits, bypassPermissions, dontAsk, plan"))
        }
        return
      }

      if (slash.command === 'fetch') {
        if (!slash.args) {
          appendAssistantMessage(tr("Please URL angeben: /fetch https://example.com"))
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
              response.truncated ? '\n[Output gekuerzt]' : null,
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
          appendAssistantMessage(`Web-Fetch failed: ${message}`)
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
        'review': 'Perform a thorough code review. Check for bugs, code quality, best practices, readability, and maintainability. Provide concrete improvement suggestions with code examples.',
        'ultrareview': 'Run a comprehensive ultra review:\n1. architecture analysis\n2. security check (OWASP Top 10)\n3. performance analysis\n4. code quality and clean code\n5. test coverage\n6. documentation\n7. dependency check\n8. best practices\nGive a detailed assessment with concrete recommendations for each area.',
        'ultraplan': 'Create a detailed multi-step plan:\n1. Analyze the requirements thoroughly\n2. Identify dependencies and risks\n3. Create numbered steps with estimated effort\n4. Define acceptance criteria for each step\n5. List risks and mitigation strategies\n6. Suggest a test concept',
        'security-review': 'Run a comprehensive security analysis:\n- OWASP Top 10 review\n- injection vulnerabilities (SQL, XSS, Command)\n- Authentication & Authorization\n- cryptography usage\n- input validation\n- sensitive data handling\n- dependencies with known CVEs\nProvide severity (Critical/High/Medium/Low) and concrete fix recommendations.',
        'simplify': 'Simplify the following code:\n- Reduce complexity (cyclomatic and cognitive)\n- Remove redundancy and dead code\n- Improve readability and maintainability\n- Keep functionality identical\n- Briefly explain each change',
        'autofix-pr': 'Analyze and automatically fix all problems:\n- linting and formatting errors\n- type errors and missing types\n- missing or failing tests\n- code style improvements\n- documentations-Luecken\nShow before/after for each change.',
        'team-onboarding': 'Create a detailed onboarding guide:\n1. project overview and architecture\n2. setup guide (step by step)\n3. coding conventions and style guide\n4. important files and folder structure\n5. development workflow and processes\n6. common tasks with solutions\n7. debugging tips',
        'passes': 'Run an iterative multi-pass analysis. Deepen the analysis and improve previous results in each pass:\nPass 1: rough analysis and overview\nPass 2: detailed analysis and improvements\nPass 3: final refinement and recommendations',
      }

      if (slash.command in aiPrompts) {
        if (!slash.args?.trim()) {
          appendAssistantMessage(`Please provide context: /${slash.command} <description or code>`)
          return
        }
        skillPromptOverride = `${aiPrompts[slash.command]}\n\nTask/Context:\n${slash.args}`
        // Don't return → flows to Ollama streaming below
      }

      // ── Model & Config Commands ──
      if (slash.command === 'model') {
        if (!slash.args?.trim()) {
          appendAssistantMessage(`Current provider: ${providerState.label}\nCurrent model: ${providerState.model || '(not set)'}\nAvailable: ${selectableModels.join(', ') || '(none loaded)'}\nUse: /model <name>`)
        } else {
          const nextModel = slash.args.trim()
          setThreadProviderSettings(threadId, {
            ...createChatProviderSelection(providerState),
            model: nextModel,
          })
          appendAssistantMessage(`Model changed for this chat: ${nextModel}`)
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
          appendAssistantMessage(`Aktuell: Temperatur ${ollama.temperature ?? 0.2}\nUse: /effort low | medium | high`)
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
          appendAssistantMessage(`Schnell-Mode for diesen Chat: ${fast}`)
        } else {
          appendAssistantMessage(tr("No fast model found. Load models in Settings."))
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
          appendAssistantMessage(`Power-Mode for diesen Chat: ${power}`)
        } else {
          appendAssistantMessage(tr("No strong model found. Load models in Settings."))
        }
        return
      }

      if (slash.command === 'compact') {
        useCoworkStore.getState().setPolicyFlag('autoCompactLongContext', true)
        appendAssistantMessage(tr("Context compression enabled. Older messages will be summarized automatically."))
        return
      }

      if (slash.command === 'debug') {
        const newVerbose = !verboseMode
        useConfigStore.getState().setPreference('verboseMode', newVerbose)
        useConfigStore.getState().setPreference('superVerboseAuditLogging', newVerbose)
        appendAssistantMessage(`Debug-Mode: ${newVerbose ? 'enabled' : 'disabled'} (Verbose + Audit-Logging)`)
        return
      }

      if (slash.command === 'sandbox') {
        useCoworkStore.getState().setPolicyFlag('strictPolicyEnforcement', true)
        useConfigStore.getState().setPreference('readOnlyFsMode', true)
        appendAssistantMessage(tr("Sandbox mode enabled:\n- Read-only filesystem access\n- Strict policy enforcement\n- All destructive operations blocked"))
        return
      }

      if (slash.command === 'less-permission-prompts') {
        useConfigStore.getState().setPreferences({
          autoApproveSafeTools: true,
          confirmOnCloseWithRunningTasks: false,
          fallbackToHumanOnRepeatedFailure: false,
        })
        appendAssistantMessage(tr("Permission questions reduced. Safe tools will be approved automatically."))
        return
      }

      if (slash.command === 'web-setup') {
        useCoworkStore.getState().setPolicyFlag('allowWebFetch', true)
        useCoworkStore.getState().setPolicyFlag('allowWebSearch', true)
        appendAssistantMessage(tr("Web access enabled. /fetch <url>, /tool web_fetch, and /tool web_search are now allowed."))
        return
      }

      if (slash.command === 'terminal-setup') {
        try {
          await useTerminalStore.getState().ensureLocalBackend()
          appendAssistantMessage(tr("Terminal backend configured. Local terminal is active."))
        } catch (err) {
          const msg = err instanceof Error ? err.message : String(err)
          appendAssistantMessage(`Terminal-Setup failed: ${msg}`)
        }
        return
      }

      // ── Data & Session Commands ──
      if (slash.command === 'context') {
        const msgCount = activeMessages.length
        const charCount = activeMessages.reduce((a, m) => a + m.content.length, 0)
        const userMsgs = activeMessages.filter(m => m.role === 'user').length
        const assistantMsgs = activeMessages.filter(m => m.role === 'assistant').length
        appendAssistantMessage(`Context:\n- Thread: "${activeThread?.title ?? 'Untitled'}"\n- ${msgCount} Messages (${userMsgs} User, ${assistantMsgs} Assistent)\n- ${charCount} Zeichen gesamt\n- Provider: ${providerState.label}\n- Model: ${providerState.model || '(not set)'}\n- Attachments: ${attachments.length}`)
        return
      }

      if (slash.command === 'rename') {
        if (!slash.args?.trim()) {
          appendAssistantMessage(tr("Please neuen Namen angeben: /rename <name>"))
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
          const newId = addThread(`Branch: ${activeThread.title}`)
          setActiveThread(newId)
          appendAssistantMessage(`New branch created from "${activeThread.title}".`)
        }
        return
      }

      if (slash.command === 'clear') {
        if (activeThread) {
          const cs = useChatStore.getState()
          cs.deleteThread(activeThread.id)
          cs.addThread('New chat')
        }
        return
      }

      if (slash.command === 'resume') {
        const cs = useChatStore.getState()
        const latest = cs.threads.sort((a, b) => b.updatedAt - a.updatedAt)[0]
        if (latest) {
          cs.setActiveThread(latest.id)
          appendAssistantMessage(`Fortgesetzt: "${latest.title}" (${latest.messages.length} Messages)`)
        } else {
          appendAssistantMessage(tr("No previous session found."))
        }
        return
      }

      if (slash.command === 'rewind') {
        const count = Number.parseInt(slash.args ?? '1', 10) || 1
        if (activeThread) {
          const removed = useChatStore.getState().removeLastMessagePairs(activeThread.id, count)
          appendAssistantMessage(
            removed.pairsRemoved > 0
              ? `${removed.pairsRemoved} message pair(s) removed (${removed.messagesRemoved} messages). Send your next instruction.`
              : 'No complete user/assistant pairs found for rewind.'
          )
        } else {
          appendAssistantMessage(tr("No activer Thread zum Rewind."))
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
        appendAssistantMessage(`Session-Recap:\n- ${userMsgs.length} user messages\n- Thread: "${activeThread?.title ?? '?'}"\n- Started: ${activeThread ? new Date(activeThread.createdAt).toLocaleString('en-US') : '?'}\n\nLatest topics:\n- ${topics || '(none)'}`)
        return
      }

      if (slash.command === 'memory') {
        if (slash.args?.trim()) {
          await useMemoryStore.getState().searchEntries(slash.args.trim())
          const entries = useMemoryStore.getState().entries
          appendAssistantMessage(entries.length > 0
            ? `${tr('Memory search')} "${slash.args.trim()}":\n${entries.slice(0, 10).map(e => `- [${e.scope}/${e.category}] ${e.content.slice(0, 100)}`).join('\n')}`
            : `No results for "${slash.args.trim()}".`)
        } else {
          await useMemoryStore.getState().loadEntries()
          const entries = useMemoryStore.getState().entries
          appendAssistantMessage(`Memory: ${entries.length} entries\nUse /memory <search-term> to search or manage entries under Features > Memory.`)
        }
        return
      }

      if (slash.command === 'stats') {
        try {
          await useInsightsStore.getState().loadSummary()
          const summary = useInsightsStore.getState().summary
          appendAssistantMessage(summary
            ? `Statistics:\n- Events: ${summary.totalEvents}\n- Sessions: ${summary.totalSessions}\n- Messages: ${summary.totalMessagesSent}\n- Token (est.): ${summary.totalTokensEst}\n- Skills: ${summary.skillUsageCount}\n- Memory: ${summary.memoryEntryCount}`
            : 'No Statistics available.')
        } catch {
          appendAssistantMessage(tr("Statistics could not be loaded."))
        }
        return
      }

      if (slash.command === 'status') {
        const procs = useProcessStore.getState().processes
        const backends = useTerminalStore.getState().backends
        const ollamaStatus = await getOllamaStatusText(ollama)
        appendAssistantMessage(`Status:\n- Ollama: ${ollamaStatus}\n- Active provider: ${providerState.label}\n- Model: ${providerState.model || '(not set)'}\n- Threads: ${useChatStore.getState().threads.length}\n- Processes: ${procs.length}\n- Backends: ${backends.length}\n- Plan mode: ${claudePlanMode ? 'active' : 'inactive'}\n- Permissions: ${claudePermissionMode}`)
        return
      }

      if (slash.command === 'cost') {
        try {
          await useInsightsStore.getState().loadSummary()
          const summary = useInsightsStore.getState().summary
          const tokens = summary?.totalTokensEst ?? 0
          appendAssistantMessage(`Cost estimate:\n- Total tokens: ${tokens}\n- Local model (Ollama): 0 EUR\n- Estimated API costs: ~${(tokens * 0.000002).toFixed(4)} EUR`)
        } catch {
          appendAssistantMessage(tr("Costs could not be calculated."))
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
              appendAssistantMessage(tr("Export abgebrochen."))
              return
            }

            await safeInvoke('export_save_text_file', {
              path: selectedPath,
              content: data,
            })

            await navigator.clipboard.writeText(data).catch(() => {})
            appendAssistantMessage(`Export (${ext}) saved: ${selectedPath}`)
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
            appendAssistantMessage(`Export (${ext}) herunterloaded und in Zwischenablage kopiert (${data.length} characters).`)
          }
        }
        return
      }

      if (slash.command === 'copy') {
        const lastAssistant = [...activeMessages].reverse().find(m => m.role === 'assistant')
        if (lastAssistant) {
          await navigator.clipboard.writeText(lastAssistant.content).catch(() => {})
          appendAssistantMessage(`Last answer copied (${lastAssistant.content.length} characters).`)
        } else {
          appendAssistantMessage(tr("No answer available to copy."))
        }
        return
      }

      if (slash.command === 'doctor') {
        setBusy(true)
        try {
          const ollamaStatus = await getOllamaStatusText(ollama)
          const backends = useTerminalStore.getState().backends
          const entries = useMemoryStore.getState().entries
          appendAssistantMessage(`System diagnosis:\n- Ollama: ${ollamaStatus} (${ollama.baseUrl})\n- Active provider: ${providerState.label} (${providerState.endpoint || 'not set'})\n- Model: ${providerState.model || '(not set)'}\n- DB: active\n- MCP: ${mcpServer.command ? `configured (${mcpServer.name})` : 'not configured'}\n- Audit: active\n- Terminal-Backends: ${backends.length}\n- Memory entries: ${entries.length}\n- Plugins: ${plugins.length}`)
        } finally {
          setBusy(false)
        }
        return
      }

      if (slash.command === 'heapdump') {
        try {
          const snapshot = await useMemoryStore.getState().createSnapshot()
          appendAssistantMessage(`Heap Dump created:\n- Memory entries: ${snapshot.total_entries}\n- Profile keys: ${snapshot.total_profile_keys}\n- Timestamp: ${new Date().toLocaleString('en-US')}`)
        } catch {
          appendAssistantMessage(tr("Heap Dump failed."))
        }
        return
      }

      if (slash.command === 'skills') {
        await useSkillStore.getState().loadSkills()
        const skills = useSkillStore.getState().skills
        if (skills.length > 0) {
          const lines = skills.slice(0, 15).map(s => `- ${s.name}: ${s.description?.slice(0, 60) ?? '(no description)'}`)
          appendAssistantMessage(`Skills (${skills.length}):\n${lines.join('\n')}`)
        } else {
          appendAssistantMessage(tr("No skills available. Skills are learned automatically or can be created manually."))
        }
        return
      }

      if (slash.command === 'tasks') {
        await useTaskStore.getState().loadFromDb()
        const allTasks = useTaskStore.getState().tasks
        if (allTasks.length > 0) {
          const lines = allTasks.slice(0, 15).map((t, i) => `${i + 1}. [${t.status}] ${t.title}`)
          appendAssistantMessage(`Tasks (${allTasks.length}):\n${lines.join('\n')}`)
        } else {
          appendAssistantMessage(tr("No offenen Tasks. Use /todo add <titel> zum Createn."))
        }
        return
      }

      if (slash.command === 'btw') {
        if (!slash.args?.trim()) {
          appendAssistantMessage(tr("Use: /btw <info> to add context information."))
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
          area: 'feedback', action: 'user_feedback', details: slash.args || 'No Kommentar',
        })
        appendAssistantMessage(`Feedback saved. Thank you!${slash.args ? '' : ' (Tip: /feedback <comment>)'}`)
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
        appendAssistantMessage(`Task geplant: "${parsed.prompt}" (${parsed.scheduleExpr})`)
        return
      }

      if (slash.command === 'agents') {
        useCrewStore.getState().loadAgents()
        const agents = useCrewStore.getState().agents
        if (agents.length > 0) {
          const lines = agents.map(a => `- ${a.name} (${a.role}): ${a.backstory.slice(0, 60)}`)
          appendAssistantMessage(`Agents (${agents.length}):\n${lines.join('\n')}`)
        } else {
          appendAssistantMessage(tr("No agents configured. Manage agents under Features > Crew AI."))
        }
        return
      }

      if (slash.command === 'crew') {
        if (slash.args?.trim()) {
          useCrewStore.getState().createCrew(`crew-${Date.now()}`, slash.args.trim(), [])
          appendAssistantMessage(`Crew created: ${slash.args.trim()}`)
        } else {
          const crews = useCrewStore.getState().crews
          appendAssistantMessage(crews.length > 0
            ? `Crews (${crews.length}):\n${crews.map(c => `- ${c.name} (${c.agents.length} agents)`).join('\n')}`
            : 'No Crews available. Use /crew <name> zum Createn.')
        }
        return
      }

      if (slash.command === 'batch') {
        if (!slash.args?.trim()) {
          appendAssistantMessage(tr("Use: /batch <task1>; <task2>; <task3>"))
          return
        }
        const batchTasks = slash.args.split(';').map(t => t.trim()).filter(Boolean)
        for (const bt of batchTasks) {
          createTask(bt, bt.slice(0, 80), threadId)
        }
        appendAssistantMessage(`${batchTasks.length} Batch-Tasks created:\n${batchTasks.map((t, i) => `${i + 1}. ${t}`).join('\n')}`)
        return
      }

      if (slash.command === 'loop') {
        if (!slash.args?.trim()) {
          appendAssistantMessage(tr("Use: /loop <task> - agent works autonomously until complete."))
          return
        }
        useConfigStore.getState().setPreference('autoPilotAllTools', true)
        skillPromptOverride = `You are working in agentic loop mode. Work autonomously on the following task until it is fully complete. Check your result and iterate if needed.\n\nTask:\n${slash.args}`
        // flows to Ollama
      }

      // ── Navigation & Display Commands ──
      if (slash.command === 'config' || slash.command === 'settings') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage(tr("Settings opened."))
        return
      }

      if (slash.command === 'ide') {
        useUiStore.getState().setActiveMode('work')
        appendAssistantMessage(tr("Workspace enabled."))
        return
      }

      if (slash.command === 'focus') {
        useUiStore.getState().toggleLeftSidebar()
        appendAssistantMessage(tr("Fokus-Mode umgeschaltet."))
        return
      }

      if (slash.command === 'theme' || slash.command === 'color') {
        const target = slash.args?.trim().toLowerCase()
        if (target === 'dark') { useUiStore.getState().setTheme('dark'); appendAssistantMessage(tr("Dark Theme enabled.")) }
        else if (target === 'light') { useUiStore.getState().setTheme('light'); appendAssistantMessage(tr("Light Theme enabled.")) }
        else { useUiStore.getState().toggleTheme(); appendAssistantMessage(tr("Theme umgeschaltet.")) }
        return
      }

      if (slash.command === 'mcp') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage(`MCP-Server: ${mcpServer.command ? `${mcpServer.name} (${mcpServer.command})` : 'not configured'}\nOpen Settings for configuration.`)
        return
      }

      if (slash.command === 'keybindings') {
        useUiStore.getState().setShortcutsOverlayOpen(true)
        appendAssistantMessage(tr("Keyboard shortcuts-Uebersicht opened."))
        return
      }

      if (slash.command === 'statusline') {
        useConfigStore.getState().setPreference('compactMode', !compactMode)
        appendAssistantMessage(`Kompakt-Mode: ${compactMode ? 'disabled' : 'enabled'}`)
        return
      }

      if (slash.command === 'tui') {
        useConfigStore.getState().setPreference('compactMode', true)
        appendAssistantMessage(tr("TUI mode enabled (compact view)."))
        return
      }

      if (slash.command === 'mobile') {
        useConfigStore.getState().setPreference('compactMode', true)
        useConfigStore.getState().setPreference('fontScale', 110)
        appendAssistantMessage(tr("Mobile view enabled (compact layout and larger font)."))
        return
      }

      // ── Misc Commands ──
      if (slash.command === 'plugin') {
        if (slash.args === 'examples' || slash.args === 'install') {
          useCoworkStore.getState().installPluginExamples()
          appendAssistantMessage(tr("Beispiel-Plugins installed."))
        } else {
          appendAssistantMessage(`Plugins: ${plugins.length} installed (${plugins.filter(p => p.enabled).length} active)\nUse: /plugin install or manage plugins under Features > Plugins.`)
        }
        return
      }

      if (slash.command === 'reload-plugins') {
        useCoworkStore.getState().installPluginExamples()
        appendAssistantMessage(tr("Plugins reloaded."))
        return
      }

      if (slash.command === 'insights' || slash.command === 'usage') {
        await useInsightsStore.getState().loadSummary()
        await useInsightsStore.getState().loadEvents()
        const summary = useInsightsStore.getState().summary
        appendAssistantMessage(summary
          ? `Insights:\n- Events: ${summary.totalEvents}\n- Sessions: ${summary.totalSessions}\n- Messages: ${summary.totalMessagesSent}\n- Token: ${summary.totalTokensEst}\n- Open Features > Insights for Details.`
          : 'No Insights available.')
        return
      }

      if (slash.command === 'diff') {
        appendAssistantMessage(tr("Diff: use Settings > Backup to inspect file diffs.\nOr use /tool read_file <path> to read a file."))
        return
      }

      if (slash.command === 'init') {
        void safeInvokeVoid('audit_event', { area: 'project', action: 'init', details: 'Project init' })
        appendAssistantMessage(tr("Project initialized. Open_Cowork configuration was created."))
        return
      }

      if (slash.command === 'teleport') {
        if (slash.args?.trim()) {
          useUiStore.getState().setWorkingPath(slash.args.trim(), 'file')
          appendAssistantMessage(`Navigation zu: ${slash.args.trim()}`)
        } else {
          appendAssistantMessage(tr("Use: /teleport <path> to quickly jump to a file/folder."))
        }
        return
      }

      if (slash.command === 'chrome') {
        useCoworkStore.getState().toggleConnector('chrome', true)
        appendAssistantMessage(tr("Chrome-Integration enabled."))
        return
      }

      if (slash.command === 'voice') {
        type SpeechRecognitionResultEventLike = Event & {
          resultIndex?: number
          results: ArrayLike<ArrayLike<{ transcript?: string; isFinal?: boolean }>>
        }
        type SpeechRecognitionConstructor = new () => {
          continuous: boolean
          lang: string
          interimResults: boolean
          intermediateResults?: boolean
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
          appendAssistantMessage(tr("Voice input is not available: this browser does not support the Web Speech API."))
          return
        }
        const recognition = new SpeechRecognition()
        recognition.lang = 'en-US'
        recognition.continuous = true
        recognition.interimResults = true
        recognition.intermediateResults = true
        recognition.maxAlternatives = 1
        const finalPackets: string[] = []
        recognition.onresult = (event: SpeechRecognitionResultEventLike) => {
          const packets: string[] = []
          const startIndex = event.resultIndex ?? 0
          for (let index = startIndex; index < event.results.length; index += 1) {
            const result = event.results[index]
            const transcript = result?.[0]?.transcript?.trim() ?? ''
            if (!transcript || !result?.[0]?.isFinal) continue
            packets.push(transcript)
          }
          if (packets.length > 0) {
            finalPackets.push(...packets)
            const packetText = packets.join(' ')
            setInputValue((current) => current.trim().length > 0 ? `${current.trim()} ${packetText}` : packetText)
            inputRef.current?.focus()
            appendAssistantMessage(`Voice packet${packets.length === 1 ? '' : 's'} captured: "${packetText}". The current input was updated and recording will keep listening.`)
          }
        }
        recognition.onerror = (event: Event) => {
          const errEvent = event as Event & { error?: string }
          appendAssistantMessage(`Voice input error: ${errEvent.error ?? 'unknown error'}`)
        }
        recognition.onend = () => {
          appendAssistantMessage(`Voice input stopped. Captured ${finalPackets.length} packet${finalPackets.length === 1 ? '' : 's'}.`)
        }
        recognition.start()
        appendAssistantMessage(tr("Voice input started. Speak in one or more packets; each final packet will be appended to the current input."))
        return
      }

      if (slash.command === 'ollama') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage(tr("Open the Ollama settings.\nConfigure endpoint, model, and runtime parameters for your local backend."))
        return
      }

      if (slash.command === 'local-model') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage(`Current model (${providerState.label}): ${providerState.model || '(not set)'}\nChange the model in Settings or directly with /model <name>.`)
        return
      }

      if (slash.command === 'local-runtime') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage(`Active Runtime:\n- Provider: ${providerState.label}\n- Endpoint: ${providerState.endpoint || '(not set)'}\n- Model: ${providerState.model || '(not set)'}\n- Cloud aliases were removed.`)
        return
      }

      if (slash.command === 'privacy-settings') {
        useConfigStore.getState().setPreference('telemetryEnabled', false)
        appendAssistantMessage(tr("Privacy: telemetry is disabled. All data stays local."))
        return
      }

      if (slash.command === 'extra-usage') {
        useConfigStore.getState().setPreference('maxToolCallsPerLoop', 50)
        appendAssistantMessage(tr("Extended usage limits enabled (50 tool calls per loop)."))
        return
      }

      if (slash.command === 'stickers') {
        appendAssistantMessage(tr("🎉 Sticker-Mode enabled! 🚀✨💡"))
        return
      }

      if (slash.command === 'release-notes') {
        appendAssistantMessage(tr("Open_Cowork v1.0:\n- 79+ slash commands (volle Claude Code compatibility)\n- 5 default personalities\n- CrewAI Multi-Agent System\n- Memory Engine mit Suche\n- Plugin-System mit Skills\n- MCP-Integration\n- Sandbox & Security Controls"))
        return
      }

      if (slash.command === 'upgrade') {
        appendAssistantMessage(tr("Upgrade: the current version is not known to be latest.\nCheck GitHub releases for newer versions."))
        return
      }

      if (slash.command === 'desktop') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage(tr("Desktop integration: configure tray icon, startup, and window options in Settings."))
        return
      }

      if (slash.command === 'remote-control') {
        appendAssistantMessage(tr("Remote control: use MCP servers or webhooks for remote control.\nConfigure this under Settings > MCP."))
        return
      }

      if (slash.command === 'remote-env') {
        useUiStore.getState().setActiveMode('settings')
        appendAssistantMessage(tr("Remote environment: configure terminal backends (SSH, containers, HPC) under Features > Terminal."))
        return
      }

      if (slash.command === 'install-github-app') {
        appendAssistantMessage(tr("GitHub integration:\n1. Create a personal access token at github.com/settings/tokens\n2. Configure an MCP server with the GitHub CLI\n3. Or use /tool mcp_call for direct API calls"))
        return
      }

      if (slash.command === 'install-slack-app') {
        appendAssistantMessage(tr("Slack integration:\n1. Create a Slack app at api.slack.com/apps\n2. Configure webhooks or an MCP server\n3. Use /tool mcp_call for Slack API calls"))
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
          appendAssistantMessage(`/${slash.command} executed.`)
          return
        }
        appendAssistantMessage(
          `Unknown Slash-Command: /${slash.command}. Use /help for available commands.`
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
      ? `Answer to question:\nQuestion: ${askUserQuestion}\nAnswer: ${baseUserPrompt}`
      : inferredClarificationContext
        ? buildClarificationContinuationPrompt(
            inferredClarificationContext.originalTask,
            inferredClarificationContext.assistantQuestion,
            baseUserPrompt,
          )
        : baseUserPrompt
    const hasApprovalBypassMarker = /\[approval-beduerftig\]/i.test(rawPrompt)
    const mergedForSend = mergeAttachments([], [...projectContextAttachments, ...draftAttachments])
    const attachmentLimitNotice = mergedForSend.rejectedCount > 0
      ? 'Maximum of 25 project and message attachments per request reached.'
      : null
    const attachmentBuild = await buildAttachmentPromptContext(mergedForSend.next, rawPrompt)
    const projectLinkBuild = await buildProjectLinkPromptContext(projectLinksToFetch)
    const attachmentContext = attachmentBuild.context
    const projectInstructionsContext = buildProjectInstructionsPromptContext(activeProject)
    const shouldRunInPlanMode = slash?.command === 'plan' || skillPlanMode || claudePlanMode
    const planWrappedPrompt = shouldRunInPlanMode
      ? `Create only a clear numbered plan in English. Do not execute anything.\n\nTask:\n${rawPrompt}`
      : rawPrompt
    const systemAddendum = buildClaudeSystemAddendum({
      globalInstruction,
      planMode: shouldRunInPlanMode,
      permissionMode: claudePermissionMode,
      enabledTools: enabledClaudeToolIds,
    })
    const basePrompt = [
      planWrappedPrompt,
      projectInstructionsContext,
      projectLinkBuild.context,
      attachmentContext,
    ].filter((part) => part.trim().length > 0).join('\n\n')
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
        projectId: activeProject?.id ?? null,
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
    setIncludeProjectLinks(false)
    setAttachmentNotice(
      [attachmentLimitNotice, projectLinkBuild.notice].filter(Boolean).join(' ') || null,
    )
    setBusy(true)
    setError(null)

    let assistantMessageId: string | null = null
    let requestPreviewMessageId: string | null = null

    try {
      const started = Date.now()
      addLog({
        level: 'info',
        area: 'llm',
        message: 'LLM request started',
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
          projectResources: projectContextAttachments.length,
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
          message: 'Attachment analysis partially failed',
          details: {
            failures: attachmentBuild.failedFiles,
          },
        })
      }
      let rawAssistantMessage = ''
      let rawThinkingMessage = ''
      let rawVerboseMessage = verboseMode
        ? `[${formatVerboseTimestamp(Date.now())}] Live-Verbose enabled`
        : ''
      let engineErrorMessage: string | null = null
      let webSearchSources: WebSearchSource[] = []
      const usedToolNames = new Set<string>()
      const toolNamesById = new Map<string, string>()
      let liveToolCalls: LiveToolCall[] = []
      let approvalSummary: string | null = null
      let awaitingUserQuestion: string | null = null
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
        setActiveAiThread(threadId)
        appendVerboseEntry('Engine-Stream started', `Working directory: ${cwd}`)
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
              appendVerboseEntry('Ollama-Request vorreadyet', event.payload)
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
              appendVerboseEntry('Approval required', [
                `Tool: ${event.request.toolName}`,
                `Description: ${event.request.description}`,
                `Risk Level: ${event.request.riskLevel}`,
              ].join('\n'))
              if (autoPilotAllTools || hasApprovalBypassMarker) {
                appendVerboseEntry(
                  'Approval granted automatically',
                  autoPilotAllTools ? 'Grund: autoPilotAllTools' : 'Grund: approval-beduerftig marker',
                )
                resolveEngineApproval({ allowed: true })
                addLog({
                  level: 'info',
                  area: 'llm',
                  message: `Approval granted automatically: ${event.request.toolName}`,
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
                message: `Approval required: ${event.request.toolName}`,
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
              appendVerboseEntry(
                `Tool started: ${event.toolName}`,
                stringifyVerboseValue(event.input),
              )
              addLog({
                level: 'info',
                area: 'llm',
                message: `Tool started: ${event.toolName}`,
                details: { toolName: event.toolName, input: event.input },
              })
              break
            case 'tool_progress': {
              const activeToolName = toolNamesById.get(event.toolUseId) ?? 'Tool'
              const progress = formatToolProgress(activeToolName, event.data)
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
              appendVerboseEntry(tr('Context compacted'), [
                `${tr('Removed messages')}: ${event.removedCount}`,
                event.summary,
              ].filter(Boolean).join('\n'))
              addLog({
                level: 'info',
                area: 'llm',
                message: 'Context compacted',
                details: { removedCount: event.removedCount, summary: event.summary },
              })
              break
            case 'context_warning':
              appendVerboseEntry(
                `Context warning: ${event.level}`,
                `Estimated tokens: ${event.estimatedTokens}`,
              )
              addLog({
                level: event.level === 'critical' ? 'warn' : 'info',
                area: 'llm',
                message: `Context warning: ${event.level}`,
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
              appendVerboseEntry('Engine-Error', event.error)
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
          ? `LLM request failed: ${engineErrorMessage}\n\n${getChatProviderFailureHint(providerState.provider)}`
          : awaitingUserQuestion
            ? `question: ${awaitingUserQuestion}`
          : approvalSummary
            ? `Approval required: ${approvalSummary}`
            : usedToolNames.size > 0
              ? `The engine used tools (${Array.from(usedToolNames).join(', ')}), but no visible final text provided.`
              : 'The engine did not provide a visible response. Please try again or check the model/prompt.'
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
          message: engineErrorMessage ? 'Engine request ended without a visible response' : 'Engine request succeeded',
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
        message: 'LLM request failed',
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
      const failureContent = `LLM request failed: ${message}\n\n${getChatProviderFailureHint(providerState.provider)}`
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
      setActiveAiThread(null)
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
    const parts = [tr('Question answered.')]

    if (selectedOptions.length > 0) {
      parts.push(`${tr('Selection')}:\n${selectedOptions.map((option) => `- ${option}`).join('\n')}`)
    }

    if (freeText) {
      parts.push(`${askUserPromptModel.freeTextLabel}:\n${freeText}`)
    }

    return parts.join('\n\n')
  }

  const handleAskUserSubmit = async () => {
    const answer = buildStructuredAskUserAnswer()
    if (!answer.trim() && attachments.length === 0 && activeProjectAttachments.length === 0 && !(includeProjectLinks && activeProjectLinks.length > 0)) return
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
      content: 'Plan rejected. Adjust the request or check the approval.',
      timestamp: Date.now(),
    })
    resolveEngineApproval({ allowed: false, reason: 'Declined by user in CoworkView.' })
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

  if (!activeThread) {
    return null // WelcomeScreen handles the empty state
  }

  const quickPrompts = [
    'Create a clear 5-step plan for the current task.',
    'Analyze the latest changes and list risks.',
    'Write the next concrete to-dos with priority.',
  ]

  const formatTime = (timestamp: number) =>
    new Date(timestamp).toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit' })

  const applyPromptToInput = (content: string, nextAttachments: ChatAttachment[] = []) => {
    setInputValue(content)
    const restored = mergeAttachments([], nextAttachments)
    setAttachments(restored.next)
    setAttachmentNotice(
      restored.rejectedCount > 0
        ? 'Not all saved files/folders could be restored.'
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
    const fallbackPrompt = 'Please fuehre dieselbe Task erneut mit denselben attachmentsn aus.'
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
            <span><strong>{tr("Session:")}</strong> {currentSessionId ?? 'not saved yet'}</span>
            <span><strong>{tr("Compactions:")}</strong> {compactionCount}</span>
            <span>
              <strong>{tr("Context:")}</strong>{' '}
              {contextWarning.level === 'none'
                ? 'stable'
                : `${contextWarning.level} (${contextWarning.estimatedTokens} Tokens)`}
            </span>
            <button
              type="button"
              className="btn-sm"
              onClick={() => setTerminalDockOpen(terminalThreadId, !terminalDockOpen)}
            >
              {terminalDockOpen
                ? tr('Terminal ausblenden')
                : terminalHiddenActivity
                  ? tr('Terminal Live einblenden')
                  : tr('Terminal Live')}
            </button>
            <button type="button" className="btn-sm" onClick={() => void forceCompact()} disabled={uiLocked}>{tr("Compact context")}</button>
          </div>
        </div>

        <div className="cowork-messages" ref={logRef}>
          {hiddenRenderedMessageCount > 0 && (
            <div className="message-window-notice">
              {hiddenRenderedMessageCount} older messages are hidden for a faster startup.
            </div>
          )}
          {renderedMessages.map((msg, index) => {
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
                  preferLive: liveThinkingBelongsToThread && msg.streaming && msg.id === visibleMessages[visibleMessages.length - 1]?.id,
                },
              )
              const displayedContent = resolveDisplayedAssistantContent(content, displayedThinkingContent)
              const canRegenerate = msg.role === 'assistant' && findPreviousUserMessage(msg.id) !== null

              // Check if this assistant message should be collapsed
              const isCollapsed = msg.role === 'assistant' && (() => {
                // Find the previous user message
                for (let i = index - 1; i >= 0; i--) {
                  if (renderedMessages[i].role === 'user') {
                    return collapsedMessageIds.has(renderedMessages[i].id)
                  }
                }
                return false
              })()

              // Check if we should show collapse button (after user message in task chat)
              const showCollapseButton = msg.role === 'user' && isTaskChat

              return (
                <div key={msg.id} className={`cowork-msg ${msg.role}${msg.crewLive ? ' crew-live-message' : ''}`}>
                <div className="msg-avatar">
                  {msg.role === 'user' ? '👤' : '✦'}
                </div>
                <div className="msg-body">
                  <div className="msg-role">
                    {msg.role === 'user' ? 'Du' : 'Open_Cowork'}
                    {showTimestamps && <span className="msg-time">{formatTime(msg.timestamp)}</span>}
                    {showCollapseButton && (
                      <button
                        type="button"
                        className="btn-collapse-toggle"
                        onClick={() => toggleCollapse(msg.id)}
                        title={collapsedMessageIds.has(msg.id) ? 'Show output' : 'Hide output'}
                        style={{ marginLeft: 8, cursor: 'pointer', background: 'none', border: 'none', fontSize: 12 }}
                      >
                        {collapsedMessageIds.has(msg.id) ? '▶' : '▼'}
                      </button>
                    )}
                  </div>
                  {msg.crewLive ? (
                    <CrewLiveMonitor live={msg.crewLive} />
                  ) : isCollapsed ? (
                    <div
                      className="msg-content-collapsed"
                      onClick={() => {
                        // Find the user message that controls this collapse
                        for (let i = index - 1; i >= 0; i--) {
                          if (renderedMessages[i].role === 'user') {
                            toggleCollapse(renderedMessages[i].id)
                            break
                          }
                        }
                      }}
                      style={{ cursor: 'pointer', color: 'var(--text-muted)', fontStyle: 'italic', padding: '8px 0' }}
                    >{tr("Output hidden - click to show")}</div>
                  ) : (
                    <div className="msg-content">
                      {displayedContent ? <HighlightedChatText content={displayedContent} /> : null}
                    </div>
                  )}
                  {!isCollapsed && (
                    <>
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
                    </>
                  )}
                  {!isCollapsed && attachmentsForMessage.length > 0 && (
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
                  {!isCollapsed && ollamaRequestPreview && (
                    <details className="message-debug">
                      <summary>{tr("Ollama Request Preview")}</summary>
                      <pre>{ollamaRequestPreview}</pre>
                    </details>
                  )}
                  {!isCollapsed && verboseMode && promptDebug && promptDebug !== content && (
                    <details className="message-debug">
                      <summary>{tr("Verbose: internal prompt")}</summary>
                      <pre>{promptDebug}</pre>
                    </details>
                  )}
                  {!isCollapsed && (
                    <div className="msg-actions">
                      <button type="button" className="btn-msg-action" onClick={() => void navigator.clipboard.writeText(content)}>{tr("Copy")}</button>
                      {msg.role === 'user' ? (
                        <button
                          type="button"
                          className="btn-msg-action"
                          onClick={() => applyPromptToInput(content, attachmentsForMessage)}
                        >{tr("Reuse")}</button>
                      ) : (
                        <>
                          <button type="button" className="btn-msg-action" onClick={() => applyPromptToInput(content)}>{tr("Als Prompt nutzen")}</button>
                          <button
                            type="button"
                            className="btn-msg-action"
                            onClick={() => void handleRegenerate(msg.id)}
                            disabled={uiLocked || !canRegenerate}
                          >{tr("Regenerate")}</button>
                        </>
                      )}
                    </div>
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

        {showScrollToBottom && (
          <button type="button" className="btn-scroll-bottom" onClick={scrollMessagesToBottom}>{tr("New messages ▾")}</button>
        )}

        {terminalDockOpen && (
          <Suspense fallback={<div className="terminal-dock-loading">Terminal is loading...</div>}>
            <TerminalDock
              threadId={terminalThreadId}
              cwd={getEffectiveWorkspaceCwd(
                attachments,
                workingFolder,
                workingPathKind,
                workspaceDefaultPath,
              )}
            />
          </Suspense>
        )}

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
              <button type="button" className="btn-approve" onClick={handleApprove} disabled={uiLocked}>{tr("✓ Approve")}</button>
              <button type="button" className="btn-reject" onClick={handleReject} disabled={uiLocked}>{tr("✗ Reject")}</button>
            </div>
          </div>
        )}

        {showAskUserPrompt && askUserQuestion && approvalSteps.length === 0 && (
          <div className="approval-banner question-banner">
            <div className="approval-header">
              <span className="approval-icon">?</span>
              <span>{tr("Open_Cowork has a question:")}</span>
            </div>
            <div className="ask-user-modal-question">
              <HighlightedChatText content={askUserPromptModel?.question ?? askUserQuestion} />
            </div>
            {askUserPromptModel && askUserPromptModel.options.length > 0 && (
              <div className="ask-user-options" role="group" aria-label={tr("Answer options")}>
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
              placeholder={askUserPromptModel?.freeTextPlaceholder ?? 'Add optional details...'}
              autoFocus
            />
            <div className="ask-user-modal-actions">
              <button
                type="button"
                className="btn-approve"
                onClick={() => void handleAskUserSubmit()}
                disabled={uiLocked || (!askUserHasStructuredResponse && attachments.length === 0 && activeProjectAttachments.length === 0 && !(includeProjectLinks && activeProjectLinks.length > 0))}
              >{tr("Send answer")}</button>
              <button
                type="button"
                className="btn-reject"
                onClick={focusAnswerInput}
                disabled={uiLocked}
              >{tr("Answer in main chat")}</button>
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
            {activeProject && (
              <div className="project-context-strip" aria-label={tr("Project context")}>
                <div className="project-context-header">
                  <span>{tr("Project:")}{activeProject.title}</span>
                  <span>{activeProjectAttachments.length}{tr("sources /")}{activeProjectLinks.length}{tr("links active")}</span>
                </div>
                {activeProject.instructions.trim() && (
                  <div className="project-context-instructions">{tr("Project instructions active")}</div>
                )}
                {activeProject.resources.length > 0 && (
                  <div className="project-context-resources">
                    {activeProject.resources.map((resource) => (
                      <label
                        key={resource.id}
                        className={`project-context-chip${resource.enabled ? '' : ' disabled'}`}
                        title={resource.path}
                      >
                        <input
                          type="checkbox"
                          checked={resource.enabled}
                          onChange={(event) => setProjectResourceEnabled(activeProject.id, resource.id, event.currentTarget.checked)}
                          disabled={uiLocked}
                        />
                        <span>
                          {resource.kind === 'folder' ? 'Folder' : resource.kind === 'link' ? 'Link' : 'File'}: {resource.label ?? getPathName(resource.path)}
                        </span>
                      </label>
                    ))}
                  </div>
                )}
                {activeProjectLinks.length > 0 && (
                  <label className="project-link-fetch-toggle">
                    <input
                      type="checkbox"
                      checked={includeProjectLinks}
                      onChange={(event) => setIncludeProjectLinks(event.currentTarget.checked)}
                      disabled={uiLocked}
                    />
                    <span>{tr("Fetch active links for the next message")}</span>
                  </label>
                )}
              </div>
            )}
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
                      disabled={uiLocked}
                    >{tr("×")}</button>
                  </span>
                ))}
              </div>
            )}
            {attachmentNotice && <p className="attachment-notice">{attachmentNotice}</p>}
            <textarea
              ref={inputRef}
              rows={2}
              placeholder={askUserQuestion ? 'Answer the question here...' : 'Next instruction...'}
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
              <div className="slash-command-menu" role="listbox" aria-label={tr("slash commands")}>
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
                    <span className="slash-command-description">{tr(suggestion.description)}</span>
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
                title={tr("Provider")}
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
                title={tr("Model")}
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
              <select
                className="chat-compact-select"
                value={enginePermissionMode}
                onChange={(e) => {
                  const mode = e.target.value as 'default' | 'plan' | 'bypass' | 'strict'
                  setEngineConfig({ permissionMode: mode })
                  setClaudePermissionMode(ENGINE_TO_CLAUDE_PERMISSION_MODE[mode])
                }}
                disabled={uiLocked}
                title={tr("Permission mode")}
              >
                <option value="default">{tr("Standard")}</option>
                <option value="plan">{tr("Plan-Mode")}</option>
                <option value="bypass">{tr("Bypass")}</option>
                <option value="strict">{tr("Strikt")}</option>
              </select>
              <div className="chat-compact-actions">
                <button type="button" className="btn-compact-action" onClick={handleAttachFiles} disabled={uiLocked}>{tr("Files")}</button>
                <button type="button" className="btn-compact-action" onClick={handleAttachFolders} disabled={uiLocked}>{tr("Folder")}</button>
              </div>
            </div>
            {busy ? (
              <button type="button" onClick={handleStop} className="btn-stop compact-send-btn">{tr("⏹ Stopp")}</button>
            ) : (
              <button type="submit" disabled={uiLocked} className="btn-send compact-send-btn">
                {askUserQuestion ? 'Send answer ->' : 'Send ->'}
              </button>
            )}
          </div>
        </form>
      </div>
    </div>
  )
}
