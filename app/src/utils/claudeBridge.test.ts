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
      globalInstruction: 'Arbeite im Projektordner.',
      planMode: true,
      permissionMode: 'plan',
      enabledTools: ['bash'],
    })

    expect(result).toContain('Projekt-Instruktion: Arbeite im Projektordner.')
    expect(result).toContain('Plan-Mode ist aktiv')
    expect(result).toContain('Permission-Modus: plan')
    expect(result).not.toContain('Aktive Tool-Familien:')
  })
})
