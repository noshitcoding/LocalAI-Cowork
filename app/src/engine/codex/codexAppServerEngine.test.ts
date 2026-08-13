import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { EngineConfig, EngineEvent } from '../core/queryEngine'
import { createAssistantMessage, createUserMessage, EMPTY_USAGE } from '../types'
import type { Tool } from '../types'
import { CodexAppServerEngine } from './codexAppServerEngine'

type RuntimeEnvelope = {
  profileId: string
  payload: { id?: number; method?: string; params?: Record<string, unknown> }
}

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listener: null as null | ((event: { payload: RuntimeEnvelope }) => void),
  unlisten: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mocks.invoke(...args),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_name: string, callback: (event: { payload: RuntimeEnvelope }) => void) => {
    mocks.listener = callback
    return mocks.unlisten
  }),
}))

function config(codex: EngineConfig['codex'] = { ownerKind: 'chat', ownerId: 'chat-1' }): EngineConfig {
  return {
    backend: 'codex',
    codex,
    cwd: 'C:/workspace',
    systemPrompt: '',
    permissionMode: 'default',
    allowedDirectories: ['C:/workspace'],
    threadId: 'chat-1',
  }
}

function emit(profileId: string, method: string, params: Record<string, unknown>, id?: number): void {
  mocks.listener?.({ payload: { profileId, payload: { id, method, params } } })
}

async function collect(engine: CodexAppServerEngine, onEvent?: (event: EngineEvent) => void): Promise<EngineEvent[]> {
  const events: EngineEvent[] = []
  for await (const event of engine.query([], 'Build the report')) {
    events.push(event)
    onEvent?.(event)
  }
  return events
}

describe('CodexAppServerEngine protocol adapter', () => {
  beforeEach(() => {
    mocks.invoke.mockReset()
    mocks.unlisten.mockReset()
    mocks.listener = null
  })

  it('maps streaming, thinking, tools, approvals, usage and completion events', async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === 'codex_thread_open') {
        return { authProfileId: 'account-a', rebuilt: false, result: { thread: { id: 'thread-a' } } }
      }
      if (command === 'codex_turn_start') {
        queueMicrotask(() => {
          emit('account-a', 'item/reasoning/summaryTextDelta', { threadId: 'thread-a', turnId: 'turn-a', delta: 'Plan' })
          emit('account-a', 'item/agentMessage/delta', { threadId: 'thread-a', turnId: 'turn-a', delta: 'Done' })
          emit('account-a', 'item/started', { threadId: 'thread-a', turnId: 'turn-a', item: { id: 'tool-a', type: 'commandExecution' } })
          emit('account-a', 'item/completed', { threadId: 'thread-a', turnId: 'turn-a', item: { id: 'tool-a', type: 'commandExecution', status: 'completed' } })
          emit('account-a', 'item/commandExecution/requestApproval', { threadId: 'thread-a', turnId: 'turn-a', command: 'npm test' }, 41)
          emit('account-a', 'thread/tokenUsage/updated', { threadId: 'thread-a', turnId: 'turn-a', tokenUsage: { total: { inputTokens: 12, outputTokens: 4, cachedInputTokens: 3 } } })
          emit('account-a', 'turn/completed', { threadId: 'thread-a', turn: { id: 'turn-a', status: 'completed' } })
        })
        return { turn: { id: 'turn-a', status: 'inProgress' } }
      }
      return {}
    })
    const engine = new CodexAppServerEngine(config())
    const toolUi = vi.fn()
    engine.setToolUICallback(toolUi)

    const events = await collect(engine, (event) => {
      if (event.type === 'approval_required') engine.resolveApproval({ allowed: true })
    })

    expect(events.map((event) => event.type)).toEqual(expect.arrayContaining([
      'thinking_delta', 'text_delta', 'tool_use_start', 'tool_use_complete',
      'approval_required', 'usage_update', 'assistant_message', 'done',
    ]))
    expect(events.find((event) => event.type === 'usage_update')).toMatchObject({
      usage: { input_tokens: 12, output_tokens: 4, cache_read_input_tokens: 3 },
    })
    expect(mocks.invoke).toHaveBeenCalledWith('codex_server_request_respond', {
      profileId: 'account-a',
      requestId: 41,
      result: { decision: 'accept' },
    })
    expect(toolUi).toHaveBeenCalledWith(expect.objectContaining({ type: 'approval', toolName: 'CodexCommand' }))
    expect(mocks.unlisten).toHaveBeenCalledOnce()
  })

  it('rebuilds a lost binding from sanitized OpenCowork history', async () => {
    let turnRequest: Record<string, unknown> | undefined
    mocks.invoke.mockImplementation(async (command: string, args: Record<string, unknown>) => {
      if (command === 'codex_thread_open') {
        return { authProfileId: 'account-a', rebuilt: true, result: { thread: { id: 'thread-new' } } }
      }
      if (command === 'codex_turn_start') {
        turnRequest = args
        queueMicrotask(() => emit('account-a', 'turn/completed', {
          threadId: 'thread-new', turn: { id: 'turn-new', status: 'completed' },
        }))
        return { turn: { id: 'turn-new' } }
      }
      return {}
    })
    const engine = new CodexAppServerEngine(config())
    const history = [
      createUserMessage([{ type: 'text', text: 'Earlier request' }]),
      createAssistantMessage([{ type: 'text', text: 'Earlier answer' }], 'codex', EMPTY_USAGE, 'end_turn'),
    ]

    const events: EngineEvent[] = []
    for await (const event of engine.query(history, 'Continue now')) events.push(event)

    expect(JSON.stringify(turnRequest)).toContain('Earlier request')
    expect(JSON.stringify(turnRequest)).toContain('Earlier answer')
    expect(JSON.stringify(turnRequest)).toContain('Continue now')
    expect(events.at(-1)?.type).toBe('done')
  })

  it('marks a limited automatic account and retries the next account only', async () => {
    let opens = 0
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === 'codex_thread_open') {
        opens += 1
        const profile = opens === 1 ? 'account-a' : 'account-b'
        return { authProfileId: profile, rebuilt: false, result: { thread: { id: `thread-${profile}` } } }
      }
      if (command === 'codex_turn_start') {
        const profile = opens === 1 ? 'account-a' : 'account-b'
        queueMicrotask(() => emit(profile, 'turn/completed', {
          threadId: `thread-${profile}`,
          turn: opens === 1
            ? { id: 'turn-a', status: 'failed', error: { message: 'usage limit reached' } }
            : { id: 'turn-b', status: 'completed' },
        }))
        return { turn: { id: opens === 1 ? 'turn-a' : 'turn-b' } }
      }
      return {}
    })
    const engine = new CodexAppServerEngine(config())

    const events = await collect(engine)

    expect(opens).toBe(2)
    expect(events).toContainEqual(expect.objectContaining({ type: 'retry', attempt: 1 }))
    expect(mocks.invoke).toHaveBeenCalledWith('codex_profile_mark_limited', {
      profileId: 'account-a', reason: 'usage limit reached',
    })
  })

  it('interrupts the active App Server turn', async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === 'codex_thread_open') {
        return { authProfileId: 'account-a', rebuilt: false, result: { thread: { id: 'thread-a' } } }
      }
      if (command === 'codex_turn_start') {
        setTimeout(() => {
          emit('account-a', 'item/agentMessage/delta', { threadId: 'thread-a', turnId: 'turn-a', delta: 'partial' })
          emit('account-a', 'turn/completed', { threadId: 'thread-a', turn: { id: 'turn-a', status: 'interrupted' } })
        }, 0)
        return { turn: { id: 'turn-a' } }
      }
      return {}
    })
    const engine = new CodexAppServerEngine(config())

    await collect(engine, (event) => {
      if (event.type === 'text_delta') engine.abort()
    })

    expect(mocks.invoke).toHaveBeenCalledWith('codex_turn_interrupt', {
      profileId: 'account-a', threadId: 'thread-a', turnId: 'turn-a',
    })
  })

  it('offers and executes only governance-allowed dynamic tools with approval', async () => {
    const call = vi.fn(async () => ({ data: { ticket: 'ABC-123', status: 'open' } }))
    const tool: Tool = {
      name: 'WorkspaceLookup',
      description: 'Look up a workspace record.',
      category: 'task',
      riskLevel: 'medium',
      inputSchema: {
        type: 'object',
        properties: { id: { type: 'string', description: 'Record id' } },
        required: ['id'],
      },
      isReadOnly: () => false,
      call,
    }
    let openRequest: Record<string, unknown> | undefined
    mocks.invoke.mockImplementation(async (command: string, args: Record<string, unknown>) => {
      if (command === 'codex_thread_open') {
        openRequest = args
        return { authProfileId: 'account-a', rebuilt: false, result: { thread: { id: 'thread-a' } } }
      }
      if (command === 'codex_turn_start') {
        queueMicrotask(() => emit('account-a', 'item/tool/call', {
          threadId: 'thread-a', turnId: 'turn-a', callId: 'call-a',
          tool: 'WorkspaceLookup', arguments: { id: 'ABC-123' },
        }, 73))
        return { turn: { id: 'turn-a' } }
      }
      if (command === 'codex_server_request_respond') {
        queueMicrotask(() => emit('account-a', 'turn/completed', {
          threadId: 'thread-a', turn: { id: 'turn-a', status: 'completed' },
        }))
        return {}
      }
      return {}
    })
    const engine = new CodexAppServerEngine(config())
    engine.updateConfig({ customTools: [tool], availableToolNames: ['WorkspaceLookup'] })

    const events = await collect(engine, (event) => {
      if (event.type === 'approval_required') engine.resolveApproval({ allowed: true })
    })

    expect(JSON.stringify(openRequest)).toContain('WorkspaceLookup')
    expect(call).toHaveBeenCalledWith(
      { id: 'ABC-123' },
      expect.objectContaining({ cwd: 'C:/workspace' }),
    )
    expect(mocks.invoke).toHaveBeenCalledWith('codex_server_request_respond', {
      profileId: 'account-a',
      requestId: 73,
      result: {
        contentItems: [{ type: 'inputText', text: '{"ticket":"ABC-123","status":"open"}' }],
        success: true,
      },
    })
    expect(events.map((event) => event.type)).toEqual(expect.arrayContaining([
      'tool_use_start', 'approval_required', 'tool_use_complete', 'done',
    ]))
  })
})
