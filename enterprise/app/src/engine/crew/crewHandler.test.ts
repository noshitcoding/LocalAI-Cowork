import { describe, expect, it } from 'vitest'
import { createChatCrewWorkTask } from './crewHandler'

describe('crew chat task adapter', () => {
  it('creates an ephemeral work task for a normal chat without persisting one first', () => {
    const task = createChatCrewWorkTask({
      userInput: 'Research the current market and write a concise brief.',
      cwd: 'C:/workspace',
      threadId: 'chat-thread',
      crewId: 'research-crew',
      runId: 'run-1',
    })

    expect(task).toMatchObject({
      id: 'chat-run-1',
      prompt: 'Research the current market and write a concise brief.',
      workDir: 'C:/workspace',
      threadId: 'chat-thread',
      runner: 'crew',
      crewId: 'research-crew',
    })
    expect(task.expectedOutput).toContain('final result')
  })

  it('preserves text when the chat request also contains image blocks', () => {
    const task = createChatCrewWorkTask({
      userInput: [
        { type: 'text', text: 'Review this screenshot.' },
        { type: 'image', source: { type: 'base64', media_type: 'image/png', data: 'abc' } },
      ],
      cwd: '',
      threadId: 'chat-thread',
      crewId: 'review-crew',
      runId: 'run-2',
    })

    expect(task.prompt).toBe('Review this screenshot.')
  })
})
