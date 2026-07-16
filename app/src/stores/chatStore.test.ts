import { describe, it, expect, beforeEach, vi } from 'vitest'
import { useChatStore, getActiveThread } from './chatStore'

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
      'Open_Cowork is ready. Send a task to start planning and execution in chat mode.',
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
      provider: 'ollama',
      model: 'llama3',
    })
    const secondThreadId = useChatStore.getState().addThread('Zweiter Chat', {
      provider: 'openrouter',
      model: 'anthropic/claude-sonnet-4',
      profileId: 'default-openrouter',
    })

    useChatStore.getState().setThreadProviderSettings(firstThreadId, {
      provider: 'openai-compatible',
      model: 'gpt-4.1-mini',
      profileId: 'default-openai-compatible',
    })

    const firstThread = useChatStore.getState().threads.find((thread) => thread.id === firstThreadId)
    const secondThread = useChatStore.getState().threads.find((thread) => thread.id === secondThreadId)

    expect(firstThread?.providerSettings).toEqual({
      provider: 'openai-compatible',
      model: 'gpt-4.1-mini',
      profileId: 'default-openai-compatible',
    })
    expect(secondThread?.providerSettings).toEqual({
      provider: 'openrouter',
      model: 'anthropic/claude-sonnet-4',
      profileId: 'default-openrouter',
    })
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

  it('loads persisted session JSON messages into readable chat history', async () => {
    invokeMock.mockImplementation(async (command, args) => {
      const typedArgs = args as { threadId?: string } | undefined

      if (command === 'db_list_threads') {
        return [{ id: 'session-1', title: 'Persistierte Analyse', created_at: '2025-01-01T00:00:00.000Z', updated_at: '2025-01-01T00:00:00.000Z' }]
      }

      if (command === 'db_list_messages' && typedArgs?.threadId === 'session-1') {
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
  })
})
