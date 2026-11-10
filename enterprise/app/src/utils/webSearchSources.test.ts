import { describe, expect, it } from 'vitest'
import {
  appendWebSearchSources,
  extractWebSearchSources,
  formatWebSearchSourcesBlock,
  mergeWebSearchSources,
  parseWebSearchSourcesFromToolResult,
} from './webSearchSources'

describe('webSearchSources', () => {
  it('parses web search tool output into structured sources', () => {
    const parsed = parseWebSearchSourcesFromToolResult([
      '1. Wetter Stuttgart heute - Englisher Wetterdienst',
      'https://www.dwd.de/stuttgart',
      'Vorhersage for heute mit Temperatur und Niederschlag.',
      '',
      '2. Stuttgart Wetter | wetter.com',
      'https://www.wetter.com/stuttgart',
      'Stundenweise Wetterdaten for Stuttgart.',
    ].join('\n'))

    expect(parsed).toEqual([
      {
        title: 'Wetter Stuttgart heute - Englisher Wetterdienst',
        url: 'https://www.dwd.de/stuttgart',
        snippet: 'Vorhersage for heute mit Temperatur und Niederschlag.',
      },
      {
        title: 'Stuttgart Wetter | wetter.com',
        url: 'https://www.wetter.com/stuttgart',
        snippet: 'Stundenweise Wetterdaten for Stuttgart.',
      },
    ])
  })

  it('parses consecutive numbered results without blank lines', () => {
    const parsed = parseWebSearchSourcesFromToolResult([
      '1. First result',
      'https://example.com/Path?Query=One',
      'First snippet line.',
      'Second snippet line.',
      '2. Second result',
      'https://example.com/Path?Query=Two',
      'Second snippet.',
      'https://example.com/third',
      'Third snippet.',
    ].join('\r\n'))

    expect(parsed).toEqual([
      {
        title: 'First result',
        url: 'https://example.com/Path?Query=One',
        snippet: 'First snippet line. Second snippet line.',
      },
      {
        title: 'Second result',
        url: 'https://example.com/Path?Query=Two',
        snippet: 'Second snippet.',
      },
      {
        title: 'https://example.com/third',
        url: 'https://example.com/third',
        snippet: 'Third snippet.',
      },
    ])
  })

  it('canonicalizes scheme and host but preserves case-sensitive URL components', () => {
    const merged = mergeWebSearchSources([], [
      { title: 'First', url: 'HTTPS://EXAMPLE.COM/Docs?Key=Value#Part' },
      { title: 'Duplicate', url: 'https://example.com/Docs?Key=Value#Part' },
      { title: 'Different path', url: 'https://example.com/docs?Key=Value#Part' },
      { title: 'Different query', url: 'https://example.com/Docs?key=Value#Part' },
      { title: 'Invalid upper', url: 'NOT A URL' },
      { title: 'Invalid lower', url: 'not a url' },
    ])

    expect(merged.map((source) => source.title)).toEqual([
      'First', 'Different path', 'Different query', 'Invalid upper', 'Invalid lower',
    ])
  })

  it('appends a visible sources block only once', () => {
    const sources = mergeWebSearchSources([], [
      { title: 'DWD', url: 'https://www.dwd.de/stuttgart', snippet: 'Vorhersage for heute.' },
      { title: 'DWD', url: 'https://www.dwd.de/stuttgart', snippet: 'Duplikat.' },
    ])

    const withSources = appendWebSearchSources('Heute wird es mild und wechselhaft.', sources)

    expect(withSources).toContain('Sources:')
    expect(withSources).toContain('https://www.dwd.de/stuttgart')
    expect(withSources.match(/https:\/\/www\.dwd\.de\/stuttgart/g)).toHaveLength(1)
    expect(appendWebSearchSources(withSources, sources)).toBe(withSources)
    expect(formatWebSearchSourcesBlock(sources)).toContain('1. DWD')
  })

  it('extracts appended sources back into a compact structure', () => {
    const answer = appendWebSearchSources('Heute wird es mild und wechselhaft.', [
      { title: 'DWD', url: 'https://www.dwd.de/stuttgart', snippet: 'Vorhersage for heute.' },
    ])

    const extracted = extractWebSearchSources(answer)

    expect(extracted.content).toBe('Heute wird es mild und wechselhaft.')
    expect(extracted.sources).toEqual([
      {
        title: 'DWD',
        url: 'https://www.dwd.de/stuttgart',
        snippet: 'Vorhersage for heute.',
      },
    ])
  })
})
