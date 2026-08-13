import { describe, expect, it } from 'vitest'
import { QueryEngine } from './queryEngine'
import type { Tool } from '../types'

function visibleToolNames(engine: QueryEngine): string[] {
  return (engine as unknown as { tools: Tool[] }).tools.map((tool) => tool.name)
}

describe('QueryEngine sandbox capability ceiling', () => {
  it('advertises only effective tools and recomputes them when the run mode changes', () => {
    const engine = new QueryEngine({
      backend: 'ollama',
      ollama: {
        baseUrl: 'http://127.0.0.1:11434',
        model: 'test',
        temperature: 0,
        contextWindow: 8_000,
        timeoutMs: 1_000,
        thinkingEnabled: false,
      },
      cwd: 'C:/shared',
      systemPrompt: 'test',
      availableToolNames: ['Read', 'Glob', 'WebSearch'],
    })

    expect(visibleToolNames(engine).sort()).toEqual(['Glob', 'Read', 'WebSearch'])
    expect(visibleToolNames(engine)).not.toContain('Bash')
    expect(visibleToolNames(engine)).not.toContain('Write')
    expect(visibleToolNames(engine)).not.toContain('DesktopLaunchApp')

    engine.updateConfig({ availableToolNames: ['Bash', 'Read', 'Write'] })
    expect(visibleToolNames(engine).sort()).toEqual(['Bash', 'Read', 'Write'])
  })

  it('keeps desktop tools out of normal workspace runs even if named in the ceiling', () => {
    const engine = new QueryEngine({
      backend: 'ollama',
      ollama: {
        baseUrl: 'http://127.0.0.1:11434',
        model: 'test',
        temperature: 0,
        contextWindow: 8_000,
        timeoutMs: 1_000,
        thinkingEnabled: false,
      },
      cwd: 'C:/shared',
      systemPrompt: 'test',
      availableToolNames: ['Read', 'DesktopLaunchApp', 'DesktopTypeText'],
      desktopControlEnabled: false,
    })

    expect(visibleToolNames(engine)).toEqual(['Read'])
  })
})
