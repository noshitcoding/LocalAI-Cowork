/* eslint-disable require-yield */
import { describe, expect, it, vi } from 'vitest'
import type { Tool, ToolResult } from '../types'

const streamOllamaMessagesMock = vi.fn()

vi.mock('../api/ollamaClient', () => ({
  buildOllamaChatRequest: vi.fn(() => ({ body: {}, debugPreview: '{}' })),
  streamOllamaMessages: (...args: unknown[]) => streamOllamaMessagesMock(...args),
}))

vi.mock('../api/anthropicClient', () => ({
  streamMessages: vi.fn(),
  toAPIToolDefs: vi.fn(() => []),
}))

async function* oneToolUseTurn() {
  return {
    content: [
      {
        type: 'tool_use',
        id: 'tool-1',
        name: 'AskUser',
        input: { question: 'Need confirmation?' },
      },
    ],
    model: 'test-model',
    stopReason: 'tool_use',
    usage: {
      input_tokens: 1,
      output_tokens: 1,
    },
    costUsd: 0,
  }
}

describe('QueryEngine AskUser pause behavior', () => {
  it('allows AskUser without a separate approval even in strict mode', async () => {
    const { QueryEngine } = await import('./queryEngine')

    const askUserTool: Tool = {
      name: 'AskUser',
      description: 'ask user',
      category: 'user_interaction',
      riskLevel: 'low' as const,
      inputSchema: {
        type: 'object' as const,
        properties: {
          question: {
            type: 'string',
            description: 'question',
          },
        },
        required: ['question'],
      },
      async call(): Promise<ToolResult<string>> {
        return {
          data: 'waiting for user',
          awaitUserInput: true,
        }
      },
      isConcurrencySafe: () => false,
      isReadOnly: () => true,
    }

    const engine = new QueryEngine({
      backend: 'ollama',
      ollama: {
        baseUrl: 'http://localhost:11434',
        model: 'test-model',
        timeoutMs: 10_000,
        contextWindow: 16_000,
        temperature: 0,
      },
      cwd: 'C:/workspace',
      systemPrompt: 'test',
      permissionMode: 'strict',
      maxTurns: 5,
      customTools: [askUserTool],
    })

    const decision = (engine as unknown as {
      evaluatePermission: (
        tool: Tool,
        input: Record<string, unknown>,
        context: { permissionContext: { mode: 'strict'; denyRules: []; allowRules: [] } },
      ) => { kind: string }
    }).evaluatePermission(askUserTool, { question: 'Need confirmation?' }, {
      permissionContext: {
        mode: 'strict',
        denyRules: [],
        allowRules: [],
      },
    })

    expect(decision.kind).toBe('allow')
  })

  it('stops loop with await_user when tool requests user input', async () => {
    const { QueryEngine } = await import('./queryEngine')

    streamOllamaMessagesMock.mockImplementation(() => oneToolUseTurn())

    const askUserTool: Tool = {
      name: 'AskUser',
      description: 'ask user',
      category: 'user_interaction',
      riskLevel: 'low' as const,
      inputSchema: {
        type: 'object' as const,
        properties: {
          question: {
            type: 'string',
            description: 'question',
          },
        },
        required: ['question'],
      },
      async call(): Promise<ToolResult<string>> {
        return {
          data: 'waiting for user',
          awaitUserInput: true,
        }
      },
      isConcurrencySafe: () => false,
      isReadOnly: () => true,
    }

    const engine = new QueryEngine({
      backend: 'ollama',
      ollama: {
        baseUrl: 'http://localhost:11434',
        model: 'test-model',
        timeoutMs: 10_000,
        contextWindow: 16_000,
        temperature: 0,
      },
      cwd: 'C:/workspace',
      systemPrompt: 'test',
      permissionMode: 'bypass',
      maxTurns: 5,
      customTools: [askUserTool],
    })

    const events: Array<{ type: string; stopReason?: string | null }> = []
    for await (const event of engine.query([], 'hello')) {
      if (event.type === 'turn_complete') {
        events.push({ type: event.type, stopReason: event.stopReason })
      }
      if (event.type === 'done') {
        events.push({ type: event.type })
      }
    }

    expect(streamOllamaMessagesMock).toHaveBeenCalledTimes(1)
    expect(events).toContainEqual({ type: 'turn_complete', stopReason: 'await_user' })
    expect(events[events.length - 1]).toEqual({ type: 'done' })
  })
})
