import { describe, it, expect, beforeEach, vi } from 'vitest'
import {
  getActiveThread,
  messageMetadataForDaemon,
  threadMetadataForDaemon,
  useChatStore,
} from './chatStore'

const invokeMock = vi.fn(async (_command: string, _args?: unknown): Promise<unknown> => undefined)

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (command: string, args?: unknown) => invokeMock(command, args),
}))

describe('chatStore', () => {
  beforeEach(() => {
    invokeMock.mockReset()
    useChatStore.setState({
      threads: [],
      activeThreadId: null,
      pendingApproval: [],
      busy: false,
      error: null,
    })
  })

  it('creates a thread with system message', () => {
    const id = useChatStore.getState().addThread('Test')
    const state = useChatStore.getState()
    expect(state.threads).toHaveLength(1)
    expect(state.threads[0].id).toBe(id)
    expect(state.threads[0].title).toBe('Test')
    expect(state.threads[0].messages).toHaveLength(1)
    expect(state.threads[0].messages[0].role).toBe('system')
    expect(state.activeThreadId).toBe(id)
  })

  it('reconstructs a missing task thread without changing its persisted id', () => {
    const first = useChatStore.getState().ensureThread('task-thread-stable', 'Stable task chat')
    const second = useChatStore.getState().ensureThread('task-thread-stable', 'Stable task chat')

    expect(first).toEqual({ id: 'task-thread-stable', created: true })
    expect(second).toEqual({ id: 'task-thread-stable', created: false })
    expect(useChatStore.getState().threads).toHaveLength(1)
    expect(useChatStore.getState().threads[0]?.id).toBe('task-thread-stable')
  })

  it('adds messages to a thread', () => {
    const id = useChatStore.getState().addThread('Test')
    useChatStore.getState().addMessage(id, {
      role: 'user',
      content: 'Hello',
      timestamp: Date.now(),
    })
    const thread = useChatStore.getState().threads[0]
    expect(thread.messages).toHaveLength(2)
    expect(thread.messages[1].role).toBe('user')
    expect(thread.messages[1].content).toBe('Hello')
    expect(thread.messages[1].id).toBeDefined()
  })

  it('deletes a thread and clears active', () => {
    const id = useChatStore.getState().addThread('Test')
    expect(useChatStore.getState().activeThreadId).toBe(id)
    useChatStore.getState().deleteThread(id)
    expect(useChatStore.getState().threads).toHaveLength(0)
    expect(useChatStore.getState().activeThreadId).toBeNull()
  })

  it('removes the latest user-assistant pair on rewind', () => {
    const id = useChatStore.getState().addThread('Test')
    useChatStore.getState().addMessage(id, {
      role: 'user',
      content: 'question 1',
      timestamp: Date.now(),
    })
    useChatStore.getState().addMessage(id, {
      role: 'assistant',
      content: 'answer 1',
      timestamp: Date.now() + 1,
    })
    useChatStore.getState().addMessage(id, {
      role: 'user',
      content: 'question 2',
      timestamp: Date.now() + 2,
    })
    useChatStore.getState().addMessage(id, {
      role: 'assistant',
      content: 'answer 2',
      timestamp: Date.now() + 3,
    })

    const result = useChatStore.getState().removeLastMessagePairs(id, 1)
    const thread = useChatStore.getState().threads[0]

    expect(result.pairsRemoved).toBe(1)
    expect(result.messagesRemoved).toBe(2)
    expect(thread.messages.map((message) => message.content)).toEqual([
      'LocalAI Cowork is ready. Send a task to start planning and execution in chat mode.',
      'question 1',
      'answer 1',
    ])
  })

  it('getActiveThread returns the active thread', () => {
    useChatStore.getState().addThread('First')
    const id2 = useChatStore.getState().addThread('Second')
    const active = getActiveThread(useChatStore.getState())
    expect(active?.id).toBe(id2)
  })

  it('manages pending approval state', () => {
    useChatStore.getState().setPendingApproval(['step1', 'step2'])
    expect(useChatStore.getState().pendingApproval).toEqual(['step1', 'step2'])
    useChatStore.getState().clearApproval()
    expect(useChatStore.getState().pendingApproval).toEqual([])
  })

  it('manages busy and error state', () => {
    useChatStore.getState().setBusy(true)
    expect(useChatStore.getState().busy).toBe(true)
    useChatStore.getState().setError('Test error')
    expect(useChatStore.getState().error).toBe('Test error')
  })

  it('preserves thinking content when switching away from and back to a streaming thread', () => {
    const firstThreadId = useChatStore.getState().addThread('Erster Chat')
    const secondThreadId = useChatStore.getState().addThread('Zweiter Chat')

    const assistantMessageId = useChatStore.getState().addMessage(firstThreadId, {
      role: 'assistant',
      content: '',
      timestamp: Date.now(),
      streaming: true,
      thinkingContent: 'still thinking',
    })

    useChatStore.getState().setActiveThread(secondThreadId)
    useChatStore.getState().updateMessage(firstThreadId, assistantMessageId, {
      thinkingContent: 'still thinking\nnext thought',
    })
    useChatStore.getState().setActiveThread(firstThreadId)

    const activeThread = getActiveThread(useChatStore.getState())
    const assistantMessage = activeThread?.messages.find((message) => message.id === assistantMessageId)

    expect(activeThread?.id).toBe(firstThreadId)
    expect(assistantMessage?.thinkingContent).toBe('still thinking\nnext thought')
    expect(assistantMessage?.streaming).toBe(true)
  })

  it('keeps provider settings isolated per thread', () => {
    const firstThreadId = useChatStore.getState().addThread('Erster Chat', {
      backend: 'openai-compatible',
      profileId: 'default-ollama',
      model: 'llama3',
    })
    const secondThreadId = useChatStore.getState().addThread('Zweiter Chat', {
      backend: 'openai-compatible',
      model: 'anthropic/claude-sonnet-4',
      profileId: 'default-openrouter',
    })

    useChatStore.getState().setThreadProviderSettings(firstThreadId, {
      backend: 'openai-compatible',
      model: 'gpt-4.1-mini',
      profileId: 'default-openai-compatible',
    })

    const firstThread = useChatStore.getState().threads.find((thread) => thread.id === firstThreadId)
    const secondThread = useChatStore.getState().threads.find((thread) => thread.id === secondThreadId)

    expect(firstThread?.providerSettings).toEqual({
      backend: 'openai-compatible',
      model: 'gpt-4.1-mini',
      profileId: 'default-openai-compatible',
    })
    expect(secondThread?.providerSettings).toEqual({
      backend: 'openai-compatible',
      model: 'anthropic/claude-sonnet-4',
      profileId: 'default-openrouter',
    })
  })

  it('renames a thread through the revision-friendly store operation', () => {
    const id = useChatStore.getState().addThread('Old title')
    useChatStore.getState().renameThread(id, '  New title  ')
    expect(useChatStore.getState().threads[0]?.title).toBe('New title')
  })

  it('keeps device paths and permission roots outside synchronized chat metadata', () => {
    const id = useChatStore.getState().addThread('Private chat', undefined, {
      mode: 'strict',
      allowedDirectories: ['C:/secret/workspace'],
    })
    const messageId = useChatStore.getState().addMessage(id, {
      role: 'user',
      content: 'Review the attached file.',
      timestamp: 42,
      attachments: [{
        path: 'C:/secret/workspace/customer-list.xlsx',
        kind: 'file',
        mediaType: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
      }],
    })
    const thread = useChatStore.getState().threads[0]
    const message = thread.messages.find((entry) => entry.id === messageId)!
    const threadPayload = threadMetadataForDaemon(thread)
    const messagePayload = messageMetadataForDaemon(id, message)

    expect(threadPayload).not.toHaveProperty('permissionConfig')
    expect(JSON.stringify(threadPayload)).not.toContain('secret/workspace')
    expect(messagePayload).toMatchObject({
      thread_id: id,
      role: 'user',
      attachment_descriptors: [{
        kind: 'file',
        label: 'customer-list.xlsx',
        availability: 'personal_device',
      }],
    })
    expect(JSON.stringify(messagePayload)).not.toContain('C:/secret')
    expect(JSON.stringify(messagePayload)).not.toContain('dataUrl')
  })

  it('switches a chat between model and crew runners and persists the selection', () => {
    const threadId = useChatStore.getState().addThread('Crew chat')
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true })
    invokeMock.mockClear()

    useChatStore.getState().setThreadRunner(threadId, 'crew', 'crew-research')
    expect(useChatStore.getState().threads[0]).toMatchObject({
      runner: 'crew',
      crewId: 'crew-research',
    })
    expect(invokeMock).toHaveBeenCalledWith('db_update_thread_runner', {
      id: threadId,
      runner: 'crew',
      crewId: 'crew-research',
    })

    useChatStore.getState().setThreadRunner(threadId, 'model', 'crew-research')
    expect(useChatStore.getState().threads[0]).toMatchObject({
      runner: 'model',
      crewId: null,
    })
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
  })

  it('updates live tool calls without persisting message content', () => {
    const threadId = useChatStore.getState().addThread('Tool Chat')
    const assistantMessageId = useChatStore.getState().addMessage(threadId, {
      role: 'assistant',
      content: '',
      timestamp: Date.now(),
      streaming: true,
    })

    useChatStore.getState().updateMessage(threadId, assistantMessageId, {
      liveToolCalls: [{
        id: 'tool-1',
        toolName: 'Read',
        input: { file_path: 'README.md' },
        status: 'running',
        startedAt: 10,
      }],
    })

    const assistantMessage = useChatStore.getState().threads[0].messages.find((message) => message.id === assistantMessageId)
    expect(assistantMessage?.liveToolCalls).toEqual([{
      id: 'tool-1',
      toolName: 'Read',
      input: { file_path: 'README.md' },
      status: 'running',
      startedAt: 10,
    }])
    expect(invokeMock).not.toHaveBeenCalledWith('db_update_message_content', expect.anything())
  })

  it('throttles partial streaming persistence and flushes final content immediately', async () => {
    vi.useFakeTimers()
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true })
    try {
      const threadId = useChatStore.getState().addThread('Streaming persistence')
      const messageId = useChatStore.getState().addMessage(threadId, {
        role: 'assistant',
        content: '',
        timestamp: 1,
        streaming: true,
      })
      invokeMock.mockClear()

      useChatStore.getState().updateMessage(threadId, messageId, {
        content: 'partial answer',
      })
      expect(invokeMock).not.toHaveBeenCalledWith('db_update_message_content', expect.anything())

      await vi.advanceTimersByTimeAsync(750)
      expect(invokeMock).toHaveBeenCalledWith(
        'db_update_message_content',
        expect.objectContaining({ id: messageId, content: expect.stringContaining('partial answer') }),
      )

      invokeMock.mockClear()
      useChatStore.getState().updateMessage(threadId, messageId, {
        content: 'final answer',
        streaming: false,
      }, { persist: true })
      expect(invokeMock).toHaveBeenCalledWith(
        'db_update_message_content',
        expect.objectContaining({ id: messageId, content: expect.stringContaining('final answer') }),
      )
    } finally {
      delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
      vi.useRealTimers()
    }
  })

  it('loads persisted structured messages into readable chat history', async () => {
    invokeMock.mockImplementation(async (command, args) => {
      const typedArgs = args as { threadId?: string } | undefined

      if (command === 'db_list_threads') {
        return [{ id: 'thread-structured', title: 'Persistierte Analyse', created_at: '2025-01-01T00:00:00.000Z', updated_at: '2025-01-01T00:00:00.000Z', runner: 'crew', crewId: 'crew-persisted' }]
      }

      if (command === 'db_list_messages' && typedArgs?.threadId === 'thread-structured') {
        return [{
          id: 'message-1',
          role: 'assistant',
          content: JSON.stringify({
            type: 'assistant',
            uuid: 'assistant-1',
            content: [{ type: 'tool_use', id: 'tool-1', name: 'ListDir', input: { path: 'C:/workspace' } }],
            model: 'llama3.1:8b',
            usage: { input_tokens: 0, output_tokens: 0 },
            stopReason: 'tool_use',
            timestamp: 10,
          }),
          timestamp: 10,
        }]
      }

      return []
    })

    await useChatStore.getState().loadFromDb()

    const message = useChatStore.getState().threads[0]?.messages[0]
    expect(message?.content).toContain('Tool-Aufruf: ListDir')
    expect(message?.content).toContain('C:/workspace')
    expect(message?.debugContent).toContain('"tool_use"')
    expect(useChatStore.getState().threads[0]).toMatchObject({ runner: 'crew', crewId: 'crew-persisted' })
  })

  it('waits for an inactive chat history before completing chat selection', async () => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true })
    let releaseInactiveHistory = (_messages: unknown[]) => {}
    const inactiveHistory = new Promise<unknown[]>((resolve) => {
      releaseInactiveHistory = resolve
    })

    invokeMock.mockImplementation(async (command, args) => {
      const typedArgs = args as { threadId?: string } | undefined
      if (command === 'db_list_threads') {
        return [
          { id: 'active-lazy-chat', title: 'Active', created_at: '2026-01-02T00:00:00Z', updated_at: '2026-01-02T00:00:00Z' },
          { id: 'inactive-lazy-chat', title: 'Inactive', created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z' },
        ]
      }
      if (command === 'db_list_messages' && typedArgs?.threadId === 'active-lazy-chat') return []
      if (command === 'db_list_messages' && typedArgs?.threadId === 'inactive-lazy-chat') return inactiveHistory
      return undefined
    })

    await useChatStore.getState().loadFromDb()
    const selecting = useChatStore.getState().setActiveThread('inactive-lazy-chat')

    expect(useChatStore.getState().activeThreadId).toBe('inactive-lazy-chat')
    expect(useChatStore.getState().threads.find((thread) => thread.id === 'inactive-lazy-chat')?.messages).toEqual([])

    releaseInactiveHistory([{
      id: 'inactive-message',
      role: 'user',
      content: 'Persisted before restart',
      timestamp: 10,
    }])
    await selecting

    expect(useChatStore.getState().threads.find((thread) => thread.id === 'inactive-lazy-chat')?.messages)
      .toEqual([expect.objectContaining({ content: 'Persisted before restart' })])
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
  })

  it('restores persisted provider metadata when returning to a chat', async () => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true })
    const threadId = 'thread-provider-refresh'
    useChatStore.setState({
      threads: [{
        id: threadId,
        title: 'Codex chat',
        messages: [],
        createdAt: 1,
        updatedAt: 2,
        providerSettings: {
          backend: 'openai-compatible',
          profileId: 'default-ollama',
          model: 'gemma4:latest',
        },
      }],
      activeThreadId: null,
    })
    invokeMock.mockImplementation(async (command) => {
      if (command === 'db_list_threads') {
        return [{
          id: threadId,
          title: 'Codex chat',
          createdAt: '2026-08-10T12:00:00Z',
          updatedAt: '2026-08-10T13:00:00Z',
          providerSettingsJson: JSON.stringify({
            backend: 'codex',
            authProfileId: 'codex-account-1',
            model: 'gpt-5.6-sol',
            reasoningEffort: 'xhigh',
          }),
          runner: 'model',
        }]
      }
      if (command === 'db_list_messages') return []
      return undefined
    })

    await useChatStore.getState().setActiveThread(threadId)

    expect(getActiveThread(useChatStore.getState())?.providerSettings).toEqual({
      backend: 'codex',
      authProfileId: 'codex-account-1',
      model: 'gpt-5.6-sol',
      reasoningEffort: 'xhigh',
    })
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
  })

  it('reloads the complete persisted history for every request preparation', async () => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true })
    let historyReadCount = 0
    invokeMock.mockImplementation(async (command, args) => {
      const typedArgs = args as { threadId?: string } | undefined
      if (command === 'db_list_threads') {
        return [{
          id: 'thread-request-refresh',
          title: 'Refresh',
          created_at: '2026-01-01T00:00:00Z',
          updated_at: '2026-01-01T00:00:00Z',
        }]
      }
      if (command === 'db_list_messages' && typedArgs?.threadId === 'thread-request-refresh') {
        historyReadCount += 1
        return [{
          id: `message-${historyReadCount}`,
          role: 'user',
          content: `history-${historyReadCount}`,
          timestamp: historyReadCount,
        }]
      }
      return undefined
    })

    await useChatStore.getState().loadFromDb()
    await useChatStore.getState().reloadThreadMessages('thread-request-refresh')
    await useChatStore.getState().reloadThreadMessages('thread-request-refresh')

    expect(historyReadCount).toBe(3)
    expect(useChatStore.getState().threads[0]?.messages[0]?.content).toBe('history-3')
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
  })
})
