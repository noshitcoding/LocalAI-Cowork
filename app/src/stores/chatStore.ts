import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { hydrateStoredMessage, serializeChatMessageForStorage } from '../utils/chatMessages'
import type { ChatAttachment } from '../utils/chatAttachments'
import { normalizeChatProviderSelection, type ChatProviderSelection } from '../utils/chatProvider'
import type { PermissionMode } from '../engine/types/tool'
import { useProjectStore } from './projectStore'

export type ChatMessage = {
  id: string
  role: 'user' | 'assistant' | 'system'
  content: string
  timestamp: number
  attachments?: ChatAttachment[]
  visibleInChat?: boolean
  debugContent?: string
  thinkingContent?: string
  verboseContent?: string
  liveToolCalls?: LiveToolCall[]
  crewLive?: CrewLiveState
  streaming?: boolean
}

export type LiveToolCallStatus = 'requested' | 'running' | 'completed' | 'failed' | 'approval' | 'waiting_input'

export type CrewLiveStatus = 'running' | 'completed' | 'failed' | 'canceled'

export type CrewLiveEntryCategory =
  | 'status'
  | 'context'
  | 'agent'
  | 'thinking'
  | 'handoff'
  | 'delegation'
  | 'tool'
  | 'mcp'
  | 'task'
  | 'result'
  | 'output'
  | 'error'

export type CrewLiveSeverity = 'info' | 'warning' | 'error'

export type CrewLiveEntry = {
  id: string
  timestamp: number
  agentId: string
  rawAgentId?: string | null
  taskId: string
  action: string
  category: CrewLiveEntryCategory
  title: string
  detail: string
  agentName?: string | null
  sourceAgent?: string | null
  targetAgent?: string | null
  rawTargetAgentId?: string | null
  provider?: string | null
  model?: string | null
  taskTitle?: string | null
  phase?: string | null
  summary?: string | null
  severity?: CrewLiveSeverity | null
  providerReasoning?: string | null
}

export type CrewLiveState = {
  streamId: string
  title: string
  status: CrewLiveStatus
  entries: CrewLiveEntry[]
  agentColors: Record<string, string>
  updatedAt: number
}

export type AskQuestionOption = {
  label: string
  value?: string
  description?: string
}

export type LiveToolCall = {
  id: string
  toolName: string
  input: Record<string, unknown>
  status: LiveToolCallStatus
  result?: string
  error?: string
  startedAt: number
  finishedAt?: number
  options?: AskQuestionOption[]
  allowMultiple?: boolean
  allowFreeformInput?: boolean
  freeTextLabel?: string
  freeTextPlaceholder?: string
}

export type PermissionConfig = {
  mode: PermissionMode
  allowedDirectories: string[]
}

export type ChatThread = {
  id: string
  title: string
  messages: ChatMessage[]
  createdAt: number
  updatedAt: number
  providerSettings?: ChatProviderSelection
  permissionConfig?: PermissionConfig
  runner?: 'crew' | 'model'
  crewId?: string | null
}

type ChatState = {
  threads: ChatThread[]
  activeThreadId: string | null
  pendingApproval: string[]
  busy: boolean
  error: string | null
  loadFromDb: () => Promise<void>
  addThread: (title: string, providerSettings?: ChatProviderSelection, permissionConfig?: PermissionConfig, runner?: 'crew' | 'model', crewId?: string | null) => string
  ensureThread: (id: string, title: string, providerSettings?: ChatProviderSelection, permissionConfig?: PermissionConfig, runner?: 'crew' | 'model', crewId?: string | null) => { id: string; created: boolean }
  hydrateThread: (thread: ChatThread) => void
  ensureThreadLoaded: (id: string) => Promise<void>
  reloadThreadMessages: (id: string) => Promise<void>
  setActiveThread: (id: string | null) => Promise<void>
  setThreadProviderSettings: (threadId: string, providerSettings?: ChatProviderSelection) => void
  setThreadPermissionConfig: (threadId: string, permissionConfig?: PermissionConfig) => void
  setThreadRunner: (threadId: string, runner: 'crew' | 'model', crewId?: string | null) => void
  addMessage: (threadId: string, message: Omit<ChatMessage, 'id'>) => string
  updateMessage: (
    threadId: string,
    messageId: string,
    patch: Partial<Pick<ChatMessage, 'content' | 'debugContent' | 'thinkingContent' | 'verboseContent' | 'liveToolCalls' | 'crewLive' | 'streaming'>>,
    options?: { persist?: boolean },
  ) => void
  setPendingApproval: (steps: string[]) => void
  clearApproval: () => void
  setBusy: (busy: boolean) => void
  setError: (error: string | null) => void
  deleteThread: (id: string) => void
  removeLastMessagePairs: (threadId: string, pairCount: number) => { pairsRemoved: number; messagesRemoved: number }
}

type DbMessage = { id: string; role: string; content: string; timestamp: number }

const loadedThreadMessages = new Set<string>()
const databaseBackedThreadIds = new Set<string>()
const loadingThreadMessages = new Map<string, Promise<void>>()
const pendingMessagePersists = new Map<string, { threadId: string; message: ChatMessage }>()
const messagePersistTimers = new Map<string, ReturnType<typeof setTimeout>>()
const STREAM_PERSIST_INTERVAL_MS = 750

function generateId(): string {
  return `${Date.now()}-${Math.random().toString(36).slice(2, 9)}`
}

function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && Boolean((window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
}

export async function persistInvoke(command: string, args: Record<string, unknown>, context: string): Promise<void> {
  if (!isTauriRuntime()) {
    return
  }
  try {
    await invoke(command, args)
  } catch (error) {
    console.error(`[chatStore] ${context} failed`, error)
  }
}

function parseTimestamp(value: string | undefined): number {
  const parsed = value ? new Date(value).getTime() : NaN
  return Number.isFinite(parsed) ? parsed : Date.now()
}

function serializeThreadProviderSettings(providerSettings?: ChatProviderSelection): string | null {
  const normalized = normalizeChatProviderSelection(providerSettings)
  return normalized ? JSON.stringify(normalized) : null
}

function parseThreadProviderSettings(raw: string | null | undefined): ChatProviderSelection | undefined {
  if (!raw?.trim()) {
    return undefined
  }

  try {
    return normalizeChatProviderSelection(JSON.parse(raw))
  } catch {
    return undefined
  }
}

function serializePermissionConfig(config?: PermissionConfig): string | null {
  if (!config) return null
  return JSON.stringify(config)
}

function parsePermissionConfig(raw: string | null | undefined): PermissionConfig | undefined {
  if (!raw?.trim()) {
    return undefined
  }

  try {
    const parsed = JSON.parse(raw)
    return {
      mode: parsed.mode || 'default',
      allowedDirectories: parsed.allowedDirectories || [],
    }
  } catch {
    return undefined
  }
}

async function loadThreadMessagesFromDb(threadId: string): Promise<ChatMessage[]> {
  const dbMsgs = await invoke<DbMessage[]>('db_list_messages', { threadId })
  return (Array.isArray(dbMsgs) ? dbMsgs : []).map((message) => hydrateStoredMessage(message))
}

function persistMessageUpdate(threadId: string, message: ChatMessage): void {
  void persistInvoke('db_update_message_content', {
    id: message.id,
    content: serializeChatMessageForStorage(message),
  }, `db_update_message_content ${threadId}`)
}

function scheduleMessagePersist(threadId: string, message: ChatMessage): void {
  if (!isTauriRuntime()) return
  pendingMessagePersists.set(message.id, { threadId, message })
  if (messagePersistTimers.has(message.id)) return

  const timer = setTimeout(() => {
    messagePersistTimers.delete(message.id)
    const pending = pendingMessagePersists.get(message.id)
    pendingMessagePersists.delete(message.id)
    if (pending) {
      persistMessageUpdate(pending.threadId, pending.message)
    }
  }, STREAM_PERSIST_INTERVAL_MS)
  messagePersistTimers.set(message.id, timer)
}

function flushMessagePersist(threadId: string, message: ChatMessage): void {
  const timer = messagePersistTimers.get(message.id)
  if (timer) clearTimeout(timer)
  messagePersistTimers.delete(message.id)
  pendingMessagePersists.delete(message.id)
  persistMessageUpdate(threadId, message)
}

export const useChatStore = create<ChatState>()((set, get) => ({
  threads: [],
  activeThreadId: null,
  pendingApproval: [],
  busy: false,
  error: null,

  loadFromDb: async () => {
    try {
      type DbThread = {
        id: string
        title: string
        created_at?: string
        createdAt?: string
        updated_at?: string
        updatedAt?: string
        provider_settings_json?: string | null
        providerSettingsJson?: string | null
        permission_config_json?: string | null
        permissionConfigJson?: string | null
        runner?: string | null
        crew_id?: string | null
        crewId?: string | null
      }
      const dbThreads = await invoke<DbThread[]>('db_list_threads')
      dbThreads.forEach((thread) => databaseBackedThreadIds.add(thread.id))
      const currentActiveThreadId = get().activeThreadId
      const sortedDbThreads = [...dbThreads].sort((a, b) => {
        const aTime = parseTimestamp(a.updated_at ?? a.updatedAt)
        const bTime = parseTimestamp(b.updated_at ?? b.updatedAt)
        return bTime - aTime
      })
      const initialActiveThreadId = currentActiveThreadId && dbThreads.some((thread) => thread.id === currentActiveThreadId)
        ? currentActiveThreadId
        : sortedDbThreads[0]?.id ?? null
      const threads: ChatThread[] = []
      for (const dt of dbThreads) {
        const messages = dt.id === initialActiveThreadId
          ? await loadThreadMessagesFromDb(dt.id)
          : []
        if (dt.id === initialActiveThreadId) {
          loadedThreadMessages.add(dt.id)
        }
        threads.push({
          id: dt.id,
          title: dt.title,
          messages,
          createdAt: parseTimestamp(dt.created_at ?? dt.createdAt),
          updatedAt: parseTimestamp(dt.updated_at ?? dt.updatedAt),
          providerSettings: parseThreadProviderSettings(dt.provider_settings_json ?? dt.providerSettingsJson),
          permissionConfig: parsePermissionConfig(dt.permission_config_json || dt.permissionConfigJson || '{}'),
          runner: dt.runner === 'crew' ? 'crew' : 'model',
          crewId: dt.runner === 'crew' ? (dt.crew_id ?? dt.crewId ?? null) : null,
        })
      }
      const hydratedThreads = threads.map((thread) => ({
        ...thread,
        messages: Array.isArray(thread.messages) ? thread.messages : [],
      }))
      const hydratedThreadIds = new Set(hydratedThreads.map((thread) => thread.id))
      
      // Find the newest thread (sorted by updatedAt)
      set((state) => ({
        threads: [
          ...state.threads.filter((thread) => !hydratedThreadIds.has(thread.id)),
          ...hydratedThreads,
        ],
        // Setze activeThreadId auf den neuesten Thread, falls none aktiv ist
        activeThreadId: state.activeThreadId && hydratedThreads.some((thread) => thread.id === state.activeThreadId)
          ? state.activeThreadId
          : initialActiveThreadId ?? state.activeThreadId,
      }))
      
      // Remove empty threads after loading
        // cleanupEmptyThreads is called through set()
      set((state) => {
        // Find all empty "New chat" threads (system message only)
        const emptyThreadIds = state.threads
          .filter(t => 
            t.title === 'New chat' && 
            t.messages.length <= 1 && 
            t.messages.every(m => m.role === 'system')
          )
          .map(t => t.id)
        // Keep only the newest empty thread, delete the rest
        if (emptyThreadIds.length <= 1) return state
        
        const sortedEmptyThreads = state.threads
          .filter(t => emptyThreadIds.includes(t.id))
          .sort((a, b) => b.updatedAt - a.updatedAt)
        // Keep the newest one, delete the rest
        const keepId = sortedEmptyThreads[0]?.id
        const deleteIds = sortedEmptyThreads.slice(1).map(t => t.id)
        // Delete from the database
        for (const id of deleteIds) {
          void persistInvoke('db_delete_thread', { id }, 'db_delete_thread cleanup')
        }
        
        return {
          threads: state.threads.filter(t => !deleteIds.includes(t.id)),
          activeThreadId: deleteIds.includes(state.activeThreadId as string) 
            ? (keepId ?? null) 
            : state.activeThreadId
        }
      })
    } catch {
      // DB not available (e.g. in tests) - keep in-memory state
    }
  },

  addThread: (title: string, providerSettings?: ChatProviderSelection, permissionConfig?: PermissionConfig, runner?: 'crew' | 'model', crewId?: string | null) => {
    const id = generateId()
    const now = Date.now()
    const normalizedProviderSettings = normalizeChatProviderSelection(providerSettings)
    const systemMsg: ChatMessage = {
      id: generateId(),
      role: 'system',
      content: 'LocalAI Cowork is ready. Send a task to start planning and execution in chat mode.',
      timestamp: now,
    }
    const thread: ChatThread = {
      id,
      title,
      messages: [systemMsg],
      createdAt: now,
      updatedAt: now,
      providerSettings: normalizedProviderSettings,
      permissionConfig,
      runner,
      crewId,
    }
    loadedThreadMessages.add(id)
    set((state) => ({
      threads: [thread, ...state.threads],
      activeThreadId: id,
    }))
    const isoNow = new Date(now).toISOString()
    void persistInvoke('db_save_thread', {
      id,
      title,
      createdAt: isoNow,
      providerSettingsJson: serializeThreadProviderSettings(normalizedProviderSettings),
      permissionConfigJson: serializePermissionConfig(permissionConfig),
      runner: runner ?? 'model',
      crewId: runner === 'crew' ? crewId ?? null : null,
    }, 'db_save_thread').then(() => databaseBackedThreadIds.add(id))
    void persistInvoke('db_save_message', {
      id: systemMsg.id,
      threadId: id,
      role: systemMsg.role,
      content: serializeChatMessageForStorage(systemMsg),
      timestamp: systemMsg.timestamp,
    }, 'db_save_message system')
    
    // Bereinige leere Threads nach dem Createn eines neuen
    set((state) => {
      // Find all empty "New chat" threads (system message only)
      const emptyThreadIds = state.threads
        .filter(t => 
          t.title === 'New chat' && 
          t.messages.length <= 1 && 
          t.messages.every(m => m.role === 'system')
        )
        .map(t => t.id)
        // Keep only the newest empty thread, delete the rest
      if (emptyThreadIds.length <= 1) return state
      
      const sortedEmptyThreads = state.threads
        .filter(t => emptyThreadIds.includes(t.id))
        .sort((a, b) => b.updatedAt - a.updatedAt)
        // Keep the newest one, delete the rest
      const keepId = sortedEmptyThreads[0]?.id
      const deleteIds = sortedEmptyThreads.slice(1).map(t => t.id)
        // Delete from the database
      for (const deleteId of deleteIds) {
        void persistInvoke('db_delete_thread', { id: deleteId }, 'db_delete_thread cleanup')
      }
      
      return {
        threads: state.threads.filter(t => !deleteIds.includes(t.id)),
        activeThreadId: deleteIds.includes(state.activeThreadId as string) 
          ? (keepId ?? null) 
          : state.activeThreadId
      }
    })
    
    return id
  },

  ensureThread: (id, title, providerSettings, permissionConfig, runner, crewId) => {
    const normalizedId = id.trim()
    const existing = get().threads.find((thread) => thread.id === normalizedId)
    if (existing) {
      return { id: existing.id, created: false }
    }

    const now = Date.now()
    const normalizedProviderSettings = normalizeChatProviderSelection(providerSettings)
    const systemMsg: ChatMessage = {
      id: generateId(),
      role: 'system',
      content: 'LocalAI Cowork is ready. Send a task to start planning and execution in chat mode.',
      timestamp: now,
    }
    const thread: ChatThread = {
      id: normalizedId,
      title,
      messages: [systemMsg],
      createdAt: now,
      updatedAt: now,
      providerSettings: normalizedProviderSettings,
      permissionConfig,
      runner,
      crewId,
    }

    loadedThreadMessages.add(normalizedId)
    set((state) => ({
      threads: [thread, ...state.threads],
      activeThreadId: normalizedId,
    }))

    const isoNow = new Date(now).toISOString()
    void persistInvoke('db_save_thread', {
      id: normalizedId,
      title,
      createdAt: isoNow,
      providerSettingsJson: serializeThreadProviderSettings(normalizedProviderSettings),
      permissionConfigJson: serializePermissionConfig(permissionConfig),
      runner: runner ?? 'model',
      crewId: runner === 'crew' ? crewId ?? null : null,
    }, 'db_save_thread restored task chat')
      .then(() => databaseBackedThreadIds.add(normalizedId))
    void persistInvoke('db_save_message', {
      id: systemMsg.id,
      threadId: normalizedId,
      role: systemMsg.role,
      content: serializeChatMessageForStorage(systemMsg),
      timestamp: systemMsg.timestamp,
    }, 'db_save_message restored task chat system')

    return { id: normalizedId, created: true }
  },

  hydrateThread: (thread) => {
    const normalized: ChatThread = {
      ...thread,
      messages: Array.isArray(thread.messages) ? thread.messages : [],
      updatedAt: thread.updatedAt || Date.now(),
      providerSettings: normalizeChatProviderSelection(thread.providerSettings),
      runner: thread.runner === 'crew' || thread.runner === 'model' ? thread.runner : undefined,
      crewId: thread.crewId ?? undefined,
    }
    loadedThreadMessages.add(normalized.id)
    set((state) => {
      const remaining = state.threads.filter((item) => item.id !== normalized.id)
      return {
        threads: [normalized, ...remaining],
        activeThreadId: normalized.id,
      }
    })
  },

  ensureThreadLoaded: async (id) => {
    if (!id || loadedThreadMessages.has(id)) return
    if (!isTauriRuntime()) {
      loadedThreadMessages.add(id)
      return
    }

    const existingLoad = loadingThreadMessages.get(id)
    if (existingLoad) {
      await existingLoad
      return
    }

    const load = loadThreadMessagesFromDb(id)
      .then((messages) => {
        loadedThreadMessages.add(id)
        set((state) => ({
          threads: state.threads.map((thread) => (
            thread.id === id
              ? { ...thread, messages }
              : thread
          )),
        }))
      })
      .catch((error) => {
        console.warn('[chatStore] db_list_messages failed', error)
        throw error
      })
      .finally(() => {
        loadingThreadMessages.delete(id)
      })

    loadingThreadMessages.set(id, load)
    await load
  },

  reloadThreadMessages: async (id) => {
    if (!id || !isTauriRuntime() || !databaseBackedThreadIds.has(id)) {
      await get().ensureThreadLoaded(id)
      return
    }

    const existingLoad = loadingThreadMessages.get(id)
    if (existingLoad) await existingLoad

    const messages = await loadThreadMessagesFromDb(id)
    loadedThreadMessages.add(id)
    set((state) => ({
      threads: state.threads.map((thread) => (
        thread.id === id
          ? { ...thread, messages }
          : thread
      )),
    }))
  },

  setActiveThread: async (id) => {
    set({ activeThreadId: id })
    if (!id) return
    await get().ensureThreadLoaded(id)
  },

  setThreadProviderSettings: (threadId, providerSettings) => {
    const normalized = normalizeChatProviderSelection(providerSettings)
    set((state) => ({
      threads: state.threads.map((thread) => (
        thread.id === threadId
          ? { ...thread, providerSettings: normalized, updatedAt: Date.now() }
          : thread
      )),
    }))
    void persistInvoke('db_update_thread_provider_settings', {
      id: threadId,
      providerSettingsJson: serializeThreadProviderSettings(normalized),
    }, 'db_update_thread_provider_settings')
  },

  setThreadPermissionConfig: (threadId: string, permissionConfig?: PermissionConfig) => {
    const serialized = serializePermissionConfig(permissionConfig)
    set((state) => ({
      threads: state.threads.map((thread) => (
        thread.id === threadId
          ? { ...thread, permissionConfig, updatedAt: Date.now() }
          : thread
      )),
    }))
    void persistInvoke('db_update_thread_permission_config', {
      id: threadId,
      permissionConfigJson: serialized,
    }, 'db_update_thread_permission_config')
  },

  setThreadRunner: (threadId, runner, crewId) => {
    const normalizedCrewId = runner === 'crew' && crewId?.trim() ? crewId.trim() : null
    set((state) => ({
      threads: state.threads.map((thread) => (
        thread.id === threadId
          ? { ...thread, runner, crewId: normalizedCrewId, updatedAt: Date.now() }
          : thread
      )),
    }))
    void persistInvoke('db_update_thread_runner', {
      id: threadId,
      runner,
      crewId: normalizedCrewId,
    }, 'db_update_thread_runner')
  },

  addMessage: (threadId, message) => {
    const msgId = generateId()
    const full: ChatMessage = {
      ...message,
      id: msgId,
      content: typeof message.content === 'string' ? message.content : '',
      attachments: Array.isArray(message.attachments) ? message.attachments : undefined,
    }
    set((state) => ({
      threads: state.threads.map((t) =>
        t.id === threadId
          ? { ...t, messages: [...t.messages, full], updatedAt: Date.now() }
          : t
      ),
    }))
    void persistInvoke('db_save_message', {
      id: msgId,
      threadId,
      role: message.role,
      content: serializeChatMessageForStorage(full),
      timestamp: message.timestamp,
    }, 'db_save_message addMessage')
    return msgId
  },

  updateMessage: (threadId, messageId, patch, options) => {
    let messageToPersist: ChatMessage | null = null

    set((state) => ({
      threads: state.threads.map((t) =>
        t.id === threadId
          ? {
              ...t,
              messages: t.messages.map((m) => {
                if (m.id !== messageId) return m
                const nextMessage = { ...m, ...patch }
                if (options?.persist) {
                  messageToPersist = nextMessage
                }
                return nextMessage
              }),
              updatedAt: Date.now(),
            }
          : t
      ),
    }))

    if (messageToPersist) {
      flushMessagePersist(threadId, messageToPersist)
    } else {
      const currentMessage = get().threads
        .find((thread) => thread.id === threadId)
        ?.messages.find((message) => message.id === messageId)
      if (currentMessage) {
        scheduleMessagePersist(threadId, currentMessage)
      }
    }
  },

  setPendingApproval: (steps) => set({ pendingApproval: steps }),
  clearApproval: () => set({ pendingApproval: [] }),
  setBusy: (busy) => set({ busy }),
  setError: (error) => set({ error }),

  deleteThread: (id) => {
    loadedThreadMessages.delete(id)
    databaseBackedThreadIds.delete(id)
    loadingThreadMessages.delete(id)
    set((state) => ({
      threads: state.threads.filter((t) => t.id !== id),
      activeThreadId: state.activeThreadId === id ? null : state.activeThreadId,
    }))
    useProjectStore.getState().detachThreadFromAll(id)
    void persistInvoke('db_delete_thread', { id }, 'db_delete_thread')
  },

  removeLastMessagePairs: (threadId, pairCount) => {
    let pairsRemoved = 0
    let messagesRemoved = 0
    let removedIds: string[] = []

    set((state) => ({
      threads: state.threads.map((t) => {
        if (t.id !== threadId) return t

        const idsToRemove = new Set<string>()
        let cursor = t.messages.length - 1

        while (cursor >= 0 && pairsRemoved < pairCount) {
          while (cursor >= 0 && t.messages[cursor]?.role === 'system') {
            cursor--
          }

          if (cursor < 0) break
          if (t.messages[cursor]?.role !== 'assistant') {
            cursor--
            continue
          }

          const assistantMessage = t.messages[cursor]
          cursor--

          while (cursor >= 0 && t.messages[cursor]?.role === 'system') {
            cursor--
          }

          if (cursor < 0 || t.messages[cursor]?.role !== 'user') {
            continue
          }

          const userMessage = t.messages[cursor]
          idsToRemove.add(assistantMessage.id)
          idsToRemove.add(userMessage.id)
          pairsRemoved++
          cursor--
        }

        removedIds = Array.from(idsToRemove)
        messagesRemoved = removedIds.length

        if (removedIds.length === 0) {
          return t
        }

        return {
          ...t,
          messages: t.messages.filter((message) => !idsToRemove.has(message.id)),
          updatedAt: Date.now(),
        }
      }),
    }))

    if (removedIds.length > 0) {
      void persistInvoke('db_delete_messages', { ids: removedIds }, 'db_delete_messages rewind')
    }

    return { pairsRemoved, messagesRemoved }
  },
}))

export function getActiveThread(state: ChatState): ChatThread | undefined {
  return state.threads.find((t) => t.id === state.activeThreadId)
}

export type { PermissionMode }
