import { describe, expect, it } from 'vitest'
import { buildClaudeSystemAddendum } from './claudeBridge'

describe('buildClaudeSystemAddendum', () => {
  it('does not inject tool families into the user prompt context', () => {
    const result = buildClaudeSystemAddendum({
      globalInstruction: '',
      planMode: false,
      permissionMode: 'default',
      enabledTools: ['bash', 'read_file', 'move_path'],
    })

    expect(result).toBe('')
  })

  it('keeps relevant execution context when needed', () => {
    const result = buildClaudeSystemAddendum({
      globalInstruction: 'Work in the project folder.',
      planMode: true,
      permissionMode: 'plan',
      enabledTools: ['bash'],
    })

    expect(result).toContain('Project instruction: Work in the project folder.')
    expect(result).toContain('Plan mode is active')
    expect(result).toContain('Permission-Mode: plan')
    expect(result).not.toContain('Active Tool-Families:')
  })
})