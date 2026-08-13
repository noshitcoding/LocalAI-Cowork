import { describe, expect, it } from 'vitest'
import { resolveAssistantPresentation, resolveDisplayedAssistantContent, resolveDisplayedThinkingContent } from './messageDisplay'

describe('resolveAssistantPresentation', () => {
  it('falls back to provided thinking content when visible text is empty', () => {
    const result = resolveAssistantPresentation('', {
      verboseMode: false,
      thinkingContent: 'Zwischenschritt der Analyse',
    })

    expect(result.content).toBe('Zwischenschritt der Analyse')
    expect(result.thinkingContent).toBe('Zwischenschritt der Analyse')
  })

  it('falls back to explicit fallback text when there is no content', () => {
    const result = resolveAssistantPresentation('', {
      verboseMode: false,
      fallbackText: 'No visible response provided.',
    })

    expect(result.content).toBe('No visible response provided.')
  })

  it('prefers visible assistant text over fallback content', () => {
    const result = resolveAssistantPresentation('Fertige answer', {
      verboseMode: false,
      fallbackText: 'Fallback',
      thinkingContent: 'Thinking',
    })

    expect(result.content).toBe('Fertige answer')
  })

  it('extracts OpenWebUI-style reasoning tags from assistant text', () => {
    const result = resolveAssistantPresentation('<thinking>Analyse</thinking>\nFinale answer', {
      verboseMode: false,
    })

    expect(result.content).toBe('Finale answer')
    expect(result.thinkingContent).toBe('Analyse')
  })
})

describe('resolveDisplayedThinkingContent', () => {
  it('prefers the longer live thinking buffer for a streaming message', () => {
    const result = resolveDisplayedThinkingContent('erster stand', 'erster stand\nzweiter stand', {
      streaming: true,
      preferLive: true,
    })

    expect(result).toBe('erster stand\nzweiter stand')
  })

  it('keeps the persisted thinking content when the message is not streaming', () => {
    const result = resolveDisplayedThinkingContent('abclosede analyse', 'kuerzer live rest', {
      streaming: false,
      preferLive: true,
    })

    expect(result).toBe('abclosede analyse')
  })

  it('keeps message thinking when live thinking belongs to a different thread', () => {
    const result = resolveDisplayedThinkingContent('thinking aus activem chat', 'thinking aus anderem chat', {
      streaming: true,
      preferLive: false,
    })

    expect(result).toBe('thinking aus activem chat')
  })
})

describe('resolveDisplayedAssistantContent', () => {
  it('hides duplicated assistant text when it matches the thinking block exactly', () => {
    expect(resolveDisplayedAssistantContent('gleicher inhalt', 'gleicher inhalt')).toBe('')
  })

  it('keeps visible assistant text when it differs from thinking', () => {
    expect(resolveDisplayedAssistantContent('visible answer', 'internal thought')).toBe('visible answer')
  })
})
