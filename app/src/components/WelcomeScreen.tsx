import { useRef, useState, useEffect } from 'react'
import type { FormEvent } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { useConfigStore } from '../stores/configStore'
import { useUiStore } from '../stores/uiStore'
import { useChatStore } from '../stores/chatStore'
import { useTaskStore } from '../stores/taskStore'
import { useLogStore } from '../stores/logStore'
import { useCoworkStore, type ClaudePermissionMode } from '../stores/coworkStore'
import type { WorkingPathKind } from '../stores/uiStore'
import type { TaskStep } from '../stores/taskStore'
import {
  getPathName,
  mergeAttachments,
  normalizeDialogSelection,
  type ChatAttachment,
} from '../utils/chatAttachments'
import { buildAttachmentPromptContext } from '../utils/attachmentPromptContext'
import { resolveAssistantPresentation } from '../utils/messageDisplay'
import { appendWebSearchSources, mergeWebSearchSources, parseWebSearchSourcesFromToolResult, type WebSearchSource } from '../utils/webSearchSources'
import { writeAuditEvent } from '../utils/audit'
import { safeInvoke } from '../utils/safeInvoke'
import { useEngineStore } from '../stores/engineStore'

type EnginePermissionMode = 'default' | 'plan' | 'bypass' | 'strict'

const CLAUDE_TO_ENGINE_PERMISSION_MODE: Record<ClaudePermissionMode, EnginePermissionMode> = {
  default: 'default',
  acceptEdits: 'strict',
  bypassPermissions: 'bypass',
  dontAsk: 'bypass',
  plan: 'plan',
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

function getEffectiveWelcomeCwd(
  attachments: ChatAttachment[],
  fallbackPath: string | null,
): string {
  for (const attachment of attachments) {
    const normalized = attachment.path.trim()
    if (normalized && attachment.kind === 'folder') {
      return normalized
    }
  }

  for (const attachment of attachments) {
    const normalized = attachment.path.trim()
    if (normalized && attachment.kind === 'file') {
      return getParentDirectory(normalized)
    }
  }

  return fallbackPath?.trim() || '.'
}

const QUICK_ACTIONS = [
  { icon: '📄', title: 'Datei erstellen', prompt: 'Erstelle eine neue Datei' },
  { icon: '📊', title: 'Daten analysieren', prompt: 'Analysiere Daten' },
  { icon: '🎨', title: 'Prototyp bauen', prompt: 'Baue einen Prototyp' },
  { icon: '📂', title: 'Dateien organisieren', prompt: 'Organisiere meine Dateien' },
  { icon: '📝', title: 'Meeting vorbereiten', prompt: 'Bereite ein Meeting vor' },
  { icon: '✉️', title: 'Nachricht entwerfen', prompt: 'Entwirf eine Nachricht' },
]

export default function WelcomeScreen() {
  const inputRef = useRef<HTMLTextAreaElement>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [useFolder, setUseFolder] = useState(false)
  const [attachments, setAttachments] = useState<ChatAttachment[]>([])
  const [attachmentNotice, setAttachmentNotice] = useState<string | null>(null)

  const ollama = useConfigStore((s) => s.ollama)
  const verboseMode = useConfigStore((s) => s.preferences.verboseMode)
  const superVerboseAuditLogging = useConfigStore((s) => s.preferences.superVerboseAuditLogging)
  const autoPilotAllTools = useConfigStore((s) => s.preferences.autoPilotAllTools)
  const availableModels = useConfigStore((s) => s.availableModels)
  const setOllama = useConfigStore((s) => s.setOllama)
  const setAvailableModels = useConfigStore((s) => s.setAvailableModels)
  const selectableModels = Array.isArray(availableModels) ? availableModels : []
  const claudePermissionMode = useCoworkStore((s) => s.claudePermissionMode)
  const engineSendMessage = useEngineStore((s) => s.sendMessage)
  const setEngineConfig = useEngineStore((s) => s.setConfig)
  const resolveEngineApproval = useEngineStore((s) => s.resolveApproval)

  const { workingFolder, workingPathKind, setWorkingPath } = useUiStore()
  const {
    addThread,
    setActiveThread,
    addMessage,
    updateMessage,
    setPendingApproval,
    clearApproval,
    setBusy: setChatBusy,
    setError: setChatError,
  } = useChatStore()
  const { createTask, setTaskSteps, updateTaskStatus } = useTaskStore()
  const addLog = useLogStore((s) => s.addLog)

  useEffect(() => {
    setEngineConfig({ permissionMode: CLAUDE_TO_ENGINE_PERMISSION_MODE[claudePermissionMode] })
  }, [claudePermissionMode, setEngineConfig])

  useEffect(() => {
    const fetchModels = async () => {
      try {
        const health = await safeInvoke<{
          ok: boolean
          models: string[]
        }>('ollama_health_check', { config: ollama }, { ok: false, models: [] })
        const models = Array.isArray(health.models) ? health.models : []
        if (health.ok && models.length > 0) {
          setAvailableModels(models)
        }
      } catch {
        // Ollama not available
      }
    }
    fetchModels()
  }, [ollama.baseUrl, setAvailableModels, ollama])

  const handlePathSelect = async (kind: WorkingPathKind) => {
    try {
      const selected = await open({
        directory: kind === 'folder',
        multiple: false,
      })
      if (typeof selected === 'string') {
        setWorkingPath(selected, kind)
        setUseFolder(true)
      }
    } catch {
      const label = kind === 'folder' ? 'Ordnerpfad' : 'Dateipfad'
      const path = window.prompt(`${label} eingeben:`)
      if (path) {
        setWorkingPath(path, kind)
        setUseFolder(true)
      }
    }
  }

  const selectedPathName = workingFolder
    ? workingFolder.split(/[\\/]/).filter(Boolean).pop() ?? workingFolder
    : null

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

  const handleSubmit = async (event: FormEvent, quickPrompt?: string) => {
    event.preventDefault()
    const text = quickPrompt ?? inputRef.current?.value?.trim()
    if (!text || busy) return

    setBusy(true)
    setChatBusy(true)
    setError(null)
    setChatError(null)
    clearApproval()

    const pathAttachment: ChatAttachment[] = useFolder && workingFolder
      ? [{ path: workingFolder, kind: workingPathKind === 'file' ? 'file' : 'folder' }]
      : []
    const mergedForSend = mergeAttachments(pathAttachment, attachments)
    const attachmentBuild = await buildAttachmentPromptContext(mergedForSend.next, text)
    const attachmentContext = attachmentBuild.context
    const promptWithAttachments = attachmentContext ? `${text}\n\n${attachmentContext}` : text

    // Create a new thread for this task
    const threadId = addThread(text.slice(0, 50))
    setActiveThread(threadId)

    const userMessage = {
      role: 'user' as const,
      content: text,
      timestamp: Date.now(),
      attachments: mergedForSend.next,
      debugContent: promptWithAttachments,
    }
    if (superVerboseAuditLogging) {
      void writeAuditEvent('super_verbose', 'welcome_user_prompt', {
        view: 'welcome',
        threadId,
        prompt: text,
        promptWithAttachments,
        attachments: mergedForSend.next,
      })
    }
    addMessage(threadId, userMessage)

    if (inputRef.current) inputRef.current.value = ''
    setAttachments([])
    setAttachmentNotice(null)
    const effectiveTimeoutMs = Math.max(ollama.timeoutMs, 600000)
    if (effectiveTimeoutMs !== ollama.timeoutMs) {
      setOllama({ timeoutMs: effectiveTimeoutMs })
    }

    let assistantMessageId: string | null = null

    try {
      const started = Date.now()
      addLog({
        level: 'info',
        area: 'llm',
        message: 'LLM-Anfrage gestartet',
        details: {
          endpoint: ollama.baseUrl,
          model: ollama.model,
          timeoutMs: effectiveTimeoutMs,
          promptChars: promptWithAttachments.length,
          parsedAttachments: attachmentBuild.parsedFiles,
          failedAttachments: attachmentBuild.failedFiles.length,
          source: 'welcome',
        },
      })
      if (superVerboseAuditLogging) {
        void writeAuditEvent('super_verbose', 'welcome_llm_request_started', {
          view: 'welcome',
          threadId,
          prompt: text,
          promptWithAttachments,
          ollama,
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
      let engineErrorMessage = ''
      let approvalSummary = ''
      let approvalTaskCreated = false
      let webSearchSources: WebSearchSource[] = []
      const usedToolNames = new Set<string>()
      const createdAssistantMessageId = addMessage(threadId, {
        role: 'assistant',
        content: '',
        timestamp: Date.now(),
        streaming: true,
      })
      assistantMessageId = createdAssistantMessageId

      const cwd = getEffectiveWelcomeCwd(mergedForSend.next, workingFolder)

      await engineSendMessage(promptWithAttachments, cwd, (event) => {
        switch (event.type) {
          case 'text_delta': {
            rawAssistantMessage += event.text
            const presentation = resolveAssistantPresentation(rawAssistantMessage, {
              verboseMode,
              thinkingContent: rawThinkingMessage,
            })
            updateMessage(threadId, createdAssistantMessageId, {
              content: presentation.content,
              thinkingContent: presentation.thinkingContent,
            })
            break
          }
          case 'thinking_delta':
            rawThinkingMessage += event.thinking
            updateMessage(threadId, createdAssistantMessageId, {
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

            if (!rawAssistantMessage && textFromEvent) {
              rawAssistantMessage = textFromEvent
            }
            if (!rawThinkingMessage && thinkingFromEvent) {
              rawThinkingMessage = thinkingFromEvent
            }

            const presentation = resolveAssistantPresentation(rawAssistantMessage, {
              verboseMode,
              thinkingContent: rawThinkingMessage,
            })
            updateMessage(threadId, createdAssistantMessageId, {
              content: presentation.content,
              thinkingContent: presentation.thinkingContent,
            })
            break
          }
          case 'approval_required': {
            approvalSummary = `${event.request.toolName}: ${event.request.description}`
            if (autoPilotAllTools) {
              resolveEngineApproval({ allowed: true, reason: 'autoPilotAllTools' })
              addLog({
                level: 'info',
                area: 'llm',
                message: `Freigabe automatisch erteilt: ${event.request.toolName}`,
                details: {
                  reason: 'autoPilotAllTools',
                  request: event.request,
                },
              })
              break
            }

            setPendingApproval([approvalSummary])
            if (!approvalTaskCreated) {
              const taskId = createTask(text, text.slice(0, 60), threadId)
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
              message: `Freigabe erforderlich: ${event.request.toolName}`,
              details: event.request,
            })
            break
          }
          case 'tool_use_start':
            usedToolNames.add(event.toolName)
            addLog({
              level: 'info',
              area: 'llm',
              message: `Tool gestartet: ${event.toolName}`,
              details: { toolName: event.toolName, input: event.input },
            })
            break
          case 'tool_use_complete':
            usedToolNames.add(event.toolName)
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
        threadId,
        messages: [],
      })

      const fallbackText = engineErrorMessage
        ? `LLM-Anfrage fehlgeschlagen: ${engineErrorMessage}\n\nPruefe unter Einstellungen den Ollama-Endpoint, das Modell und den Timeout.`
        : approvalSummary
          ? `Freigabe erforderlich: ${approvalSummary}`
          : usedToolNames.size > 0
            ? `Die Engine hat Tools verwendet (${Array.from(usedToolNames).join(', ')}), aber keinen sichtbaren Abschlusstext geliefert.`
            : 'Das Modell hat keine sichtbare Antwort geliefert. Bitte erneut versuchen oder Modell/Prompt pruefen.'
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
        streaming: false,
      }, {
        persist: true,
      })
      addLog({
        level: 'info',
        area: 'llm',
        message: 'LLM-Anfrage erfolgreich',
        details: {
          endpoint: ollama.baseUrl,
          model: ollama.model,
          cwd,
          durationMs: Date.now() - started,
          responseChars: rawAssistantMessage.length,
          usedTools: Array.from(usedToolNames),
        },
      })
      if (superVerboseAuditLogging) {
        void writeAuditEvent('super_verbose', 'welcome_llm_request_finished', {
          view: 'welcome',
          threadId,
          prompt: text,
          promptWithAttachments,
          assistantRawResponse: rawAssistantMessage,
          assistantVisibleResponse: presentation.content,
          endpoint: ollama.baseUrl,
          model: ollama.model,
          approvalSummary,
          cwd,
          usedTools: Array.from(usedToolNames),
        })
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      setError(message)
      setChatError(message)
      addLog({
        level: 'error',
        area: 'llm',
        message: 'LLM-Anfrage fehlgeschlagen',
        details: {
          endpoint: ollama.baseUrl,
          model: ollama.model,
          timeoutMs: effectiveTimeoutMs,
          error: message,
          source: 'welcome',
        },
      })
      if (superVerboseAuditLogging) {
        void writeAuditEvent('super_verbose', 'welcome_llm_request_failed', {
          view: 'welcome',
          threadId,
          prompt: text,
          promptWithAttachments,
          error: message,
          ollama,
        })
      }
      const failureContent = `LLM-Anfrage fehlgeschlagen: ${message}\n\nPrüfe unter Einstellungen den Ollama-Endpoint, das Modell und den Timeout.`
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
      setChatBusy(false)
    }
  }

  return (
    <div className="welcome-screen">
      <div className="welcome-content">
        <div className="welcome-hero">
          <div className="welcome-icon">✦</div>
          <h1 className="welcome-heading">
            Was sollen wir heute erledigen?
          </h1>
          <p className="welcome-subheading">
            Open_Cowork kann Aufgaben planen, ausführen und Dateien verwalten — alles lokal auf deinem Rechner.
          </p>
        </div>

        <div className="quick-actions-grid">
          {QUICK_ACTIONS.map((action) => (
            <button
              key={action.title}
              type="button"
              className="quick-action-card"
              onClick={(e) => handleSubmit(e, action.prompt)}
              disabled={busy}
            >
              <span className="quick-action-icon">{action.icon}</span>
              <span className="quick-action-title">{action.title}</span>
            </button>
          ))}
        </div>

        <form className="welcome-input-bar" onSubmit={handleSubmit}>
          <div className="input-wrapper">
            <textarea
              ref={inputRef}
              rows={2}
              placeholder="Wie kann ich dir heute helfen?"
              disabled={busy}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault()
                  handleSubmit(e)
                }
              }}
            />
          </div>

          <div className="input-bar-footer">
            <div className="input-bar-left">
              <label className="folder-toggle">
                <input
                  type="checkbox"
                  checked={useFolder}
                  onChange={(e) => {
                    setUseFolder(e.target.checked)
                    if (!e.target.checked) setWorkingPath(null)
                  }}
                />
                <span>Lokalen Pfad nutzen</span>
              </label>
              {useFolder && (
                <div className="path-actions">
                  <button
                    type="button"
                    className="btn-folder-select"
                    onClick={() => handlePathSelect('folder')}
                  >
                    Ordner wählen
                  </button>
                  <button
                    type="button"
                    className="btn-folder-select"
                    onClick={() => handlePathSelect('file')}
                  >
                    Datei wählen
                  </button>
                  {selectedPathName && (
                    <span className="selected-path" title={workingFolder ?? undefined}>
                      {workingPathKind === 'file' ? 'Datei' : 'Ordner'}: {selectedPathName}
                    </span>
                  )}
                </div>
              )}
              <div className="attachment-actions">
                <button
                  type="button"
                  className="btn-attach"
                  onClick={handleAttachFiles}
                  disabled={busy}
                >
                  Dateien
                </button>
                <button
                  type="button"
                  className="btn-attach"
                  onClick={handleAttachFolders}
                  disabled={busy}
                >
                  Ordner
                </button>
              </div>
            </div>

            <div className="input-bar-right">
              <select
                className="model-selector"
                value={ollama.model}
                onChange={(e) => setOllama({ model: e.target.value })}
              >
                {selectableModels.length > 0 ? (
                  selectableModels.map((m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ))
                ) : (
                  <option value={ollama.model}>{ollama.model}</option>
                )}
              </select>

              <button type="submit" className="btn-go" disabled={busy}>
                {busy ? 'Läuft...' : "Los geht's →"}
              </button>
            </div>
          </div>

          {attachments.length > 0 && (
            <div className="attachment-list" aria-label="Verbundene Elemente">
              {attachments.map((item) => (
                <span key={`${item.kind}-${item.path}`} className="attachment-chip" title={item.path}>
                  <span className="attachment-chip-label">
                    {item.kind === 'folder' ? 'Ordner' : 'Datei'}: {getPathName(item.path)}
                  </span>
                  <button
                    type="button"
                    className="attachment-remove"
                    onClick={() => handleRemoveAttachment(item)}
                    aria-label={`Anhang entfernen: ${item.path}`}
                    disabled={busy}
                  >
                    ×
                  </button>
                </span>
              ))}
            </div>
          )}
          {attachmentNotice && <p className="attachment-notice">{attachmentNotice}</p>}
        </form>

        {error && <p className="error welcome-error">{error}</p>}

        <div className="connectors-section">
          <p className="connectors-title">Verbinde deine Tools mit Open_Cowork</p>
          <div className="connector-icons">
            <span className="connector-badge" title="MCP Server">🔌 MCP</span>
            <span className="connector-badge" title="Dateisystem">📂 Dateien</span>
            <span className="connector-badge" title="Ollama">🤖 Ollama</span>
          </div>
        </div>
      </div>
    </div>
  )
}
