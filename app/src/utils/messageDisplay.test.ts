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
      fallbackText: 'Keine sichtbare Antwort geliefert.',
    })

    expect(result.content).toBe('Keine sichtbare Antwort geliefert.')
  })

  it('prefers visible assistant text over fallback content', () => {
    const result = resolveAssistantPresentation('Fertige Antwort', {
      verboseMode: false,
      fallbackText: 'Fallback',
      thinkingContent: 'Thinking',
    })

    expect(result.content).toBe('Fertige Antwort')
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
    const result = resolveDisplayedThinkingContent('abgeschlossene analyse', 'kuerzer live rest', {
      streaming: false,
      preferLive: true,
    })

    expect(result).toBe('abgeschlossene analyse')
  })
})

describe('resolveDisplayedAssistantContent', () => {
  it('hides duplicated assistant text when it matches the thinking block exactly', () => {
    expect(resolveDisplayedAssistantContent('gleicher inhalt', 'gleicher inhalt')).toBe('')
  })

  it('keeps visible assistant text when it differs from thinking', () => {
    expect(resolveDisplayedAssistantContent('sichtbare antwort', 'interner gedanke')).toBe('sichtbare antwort')
  })
})