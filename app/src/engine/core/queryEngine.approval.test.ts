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
        name: 'MovePath',
        input: {
          source_path: 'a.txt',
          destination_path: 'b.txt',
        },
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

async function* oneTextTurn() {
  return {
    content: [
      {
        type: 'text',
        text: 'done',
      },
    ],
    model: 'test-model',
    stopReason: 'end_turn',
    usage: {
      input_tokens: 1,
      output_tokens: 1,
    },
    costUsd: 0,
  }
}

async function* onePlanOnlyTurn() {
  return {
    content: [
      {
        type: 'text',
        text: 'Diese Aktion ist destruktiv (Fileverschiebung).\n\n**Status:** approval-beduerftig\n\n**Plan:**\n1. Create die Folder a und b.\n2. Verschiebe die Files abwechselnd nach a und b.',
      },
    ],
    model: 'test-model',
    stopReason: 'end_turn',
    usage: {
      input_tokens: 1,
      output_tokens: 1,
    },
    costUsd: 0,
  }
}

async function* oneDesktopPlanOnlyTurn() {
  return {
    content: [
      {
        type: 'text',
        text: 'I see that the dialog window is still open. I will now click "Cancel" directly and then continue renaming.',
      },
    ],
    model: 'test-model',
    stopReason: 'end_turn',
    usage: {
      input_tokens: 1,
      output_tokens: 1,
    },
    costUsd: 0,
  }
}

async function* oneDesktopToolUseTurn() {
  return {
    content: [
      {
        type: 'tool_use',
        id: 'tool-desktop-1',
        name: 'DesktopClick',
        input: {
          x: 460,
          y: 512,
          button: 'left',
        },
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

async function* oneNarratedShellPlanTurn() {
  return {
    content: [
      {
        type: 'text',
        text: 'Since the clicks are not taking effect, I will now close KiCad via PowerShell, rename the project folder, and then restart the project.',
      },
    ],
    model: 'test-model',
    stopReason: 'end_turn',
    usage: {
      input_tokens: 1,
      output_tokens: 1,
    },
    costUsd: 0,
  }
}

async function* oneShellToolUseTurn() {
  return {
    content: [
      {
        type: 'tool_use',
        id: 'tool-shell-1',
        name: 'Bash',
        input: {
          command: 'Stop-Process -Name kicad -Force',
        },
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

function getLastUserText(
  messages: Array<{ role: 'user' | 'assistant'; content: string | Array<{ type: string; text?: string }> }>,
): string {
  const userMessages = messages.filter((message) => message.role === 'user')
  const last = userMessages[userMessages.length - 1]
  if (!last) return ''
  if (typeof last.content === 'string') return last.content
  return last.content
    .filter((block): block is { type: 'text'; text: string } => block.type === 'text' && typeof block.text === 'string')
    .map((block) => block.text)
    .join('\n')
}

describe('QueryEngine approval event flow', () => {
  it('emits approval_required before waiting for medium-risk tool approval', async () => {
    const { QueryEngine } = await import('./queryEngine')

    streamOllamaMessagesMock.mockReset()
    streamOllamaMessagesMock
      .mockImplementationOnce(() => oneToolUseTurn())
      .mockImplementationOnce(() => oneTextTurn())

    const moveTool: Tool = {
      name: 'MovePath',
      description: 'move path',
      category: 'filesystem',
      riskLevel: 'medium' as const,
      inputSchema: {
        type: 'object' as const,
        properties: {
          source_path: { type: 'string', description: 'source' },
          destination_path: { type: 'string', description: 'destination' },
        },
        required: ['source_path', 'destination_path'],
      },
      async call(): Promise<ToolResult<string>> {
        return { data: 'moved' }
      },
      isConcurrencySafe: () => false,
      isReadOnly: () => false,
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
      permissionMode: 'default',
      maxTurns: 5,
      customTools: [moveTool],
    })

    const events: string[] = []
    for await (const event of engine.query([], 'move the file')) {
      events.push(event.type)

      if (event.type === 'approval_required') {
        engine.resolveApproval({ allowed: true, reason: 'approved in test' })
      }
    }

    expect(streamOllamaMessagesMock).toHaveBeenCalledTimes(2)
    expect(events).toContain('approval_required')
    expect(events).toContain('tool_use_complete')
    expect(events).toContain('turn_complete')
    expect(events[events.length - 1]).toBe('done')
  })

  it('requests approval and retries execution when model only returns a destructive text plan', async () => {
    const { QueryEngine } = await import('./queryEngine')

    streamOllamaMessagesMock.mockReset()
    const seenLastUserTexts: string[] = []
    streamOllamaMessagesMock
      .mockImplementationOnce((_config: unknown, messages: Array<{ role: 'user' | 'assistant'; content: string | Array<{ type: string; text?: string }> }>) => {
        seenLastUserTexts.push(getLastUserText(messages))
        return onePlanOnlyTurn()
      })
      .mockImplementationOnce((_config: unknown, messages: Array<{ role: 'user' | 'assistant'; content: string | Array<{ type: string; text?: string }> }>) => {
        seenLastUserTexts.push(getLastUserText(messages))
        return oneToolUseTurn()
      })
      .mockImplementationOnce(() => oneTextTurn())

    const moveTool: Tool = {
      name: 'MovePath',
      description: 'move path',
      category: 'filesystem',
      riskLevel: 'medium' as const,
      inputSchema: {
        type: 'object' as const,
        properties: {
          source_path: { type: 'string', description: 'source' },
          destination_path: { type: 'string', description: 'destination' },
        },
        required: ['source_path', 'destination_path'],
      },
      async call(): Promise<ToolResult<string>> {
        return { data: 'moved' }
      },
      isConcurrencySafe: () => false,
      isReadOnly: () => false,
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
      permissionMode: 'default',
      maxTurns: 6,
      customTools: [moveTool],
    })

    const events: string[] = []
    for await (const event of engine.query([], 'sort the files into two folders')) {
      events.push(event.type)

      if (event.type === 'approval_required') {
        engine.resolveApproval({ allowed: true, reason: 'approved in test' })
      }
    }

    expect(streamOllamaMessagesMock).toHaveBeenCalledTimes(3)
    expect(events).toContain('approval_required')
    expect(events).toContain('tool_use_complete')
    expect(seenLastUserTexts[1]).toContain('Approval granted')
  })

  it('nudges desktop tasks back into tool execution when the model only narrates the next click', async () => {
    const { QueryEngine } = await import('./queryEngine')

    streamOllamaMessagesMock.mockReset()
    const seenLastUserTexts: string[] = []
    streamOllamaMessagesMock
      .mockImplementationOnce((_config: unknown, messages: Array<{ role: 'user' | 'assistant'; content: string | Array<{ type: string; text?: string }> }>) => {
        seenLastUserTexts.push(getLastUserText(messages))
        return oneDesktopPlanOnlyTurn()
      })
      .mockImplementationOnce((_config: unknown, messages: Array<{ role: 'user' | 'assistant'; content: string | Array<{ type: string; text?: string }> }>) => {
        seenLastUserTexts.push(getLastUserText(messages))
        return oneDesktopToolUseTurn()
      })
      .mockImplementationOnce(() => oneTextTurn())

    const desktopClickTool: Tool = {
      name: 'DesktopClick',
      description: 'desktop click',
      category: 'desktop',
      riskLevel: 'high' as const,
      inputSchema: {
        type: 'object' as const,
        properties: {
          x: { type: 'number', description: 'x' },
          y: { type: 'number', description: 'y' },
          button: { type: 'string', description: 'button' },
        },
        required: ['x', 'y'],
      },
      async call(): Promise<ToolResult<string>> {
        return { data: 'clicked' }
      },
      isConcurrencySafe: () => false,
      isReadOnly: () => false,
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
      permissionMode: 'default',
      maxTurns: 6,
      customTools: [desktopClickTool],
    })

    const events: string[] = []
    for await (const event of engine.query([], 'schliesse das dialogfenster in kicad')) {
      events.push(event.type)

      if (event.type === 'approval_required') {
        engine.resolveApproval({ allowed: true, reason: 'approved in test' })
      }
    }

    expect(streamOllamaMessagesMock).toHaveBeenCalledTimes(3)
    expect(events).toContain('approval_required')
    expect(events).toContain('tool_use_complete')
    expect(seenLastUserTexts[1]).toContain('Execute the next desktop step now')
  })

  it('nudges narrated shell fallback plans back into actual tool execution', async () => {
    const { QueryEngine } = await import('./queryEngine')

    streamOllamaMessagesMock.mockReset()
    const seenLastUserTexts: string[] = []
    streamOllamaMessagesMock
      .mockImplementationOnce((_config: unknown, messages: Array<{ role: 'user' | 'assistant'; content: string | Array<{ type: string; text?: string }> }>) => {
        seenLastUserTexts.push(getLastUserText(messages))
        return oneNarratedShellPlanTurn()
      })
      .mockImplementationOnce((_config: unknown, messages: Array<{ role: 'user' | 'assistant'; content: string | Array<{ type: string; text?: string }> }>) => {
        seenLastUserTexts.push(getLastUserText(messages))
        return oneShellToolUseTurn()
      })
      .mockImplementationOnce(() => oneTextTurn())

    const bashTool: Tool = {
      name: 'Bash',
      description: 'shell command',
      category: 'shell',
      riskLevel: 'high' as const,
      inputSchema: {
        type: 'object' as const,
        properties: {
          command: { type: 'string', description: 'command' },
        },
        required: ['command'],
      },
      async call(): Promise<ToolResult<string>> {
        return { data: 'shell ok' }
      },
      isConcurrencySafe: () => false,
      isReadOnly: () => false,
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
      maxTurns: 6,
      customTools: [bashTool],
    })

    const events: string[] = []
    for await (const event of engine.query([], 'nochmal testen')) {
      events.push(event.type)
    }

    expect(streamOllamaMessagesMock).toHaveBeenCalledTimes(3)
    expect(events).toContain('tool_use_complete')
    expect(seenLastUserTexts[1]).toContain('Execute the last described next step now')
  })
})
