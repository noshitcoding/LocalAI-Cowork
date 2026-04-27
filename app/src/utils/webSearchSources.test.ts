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
      '1. Wetter Stuttgart heute - Deutscher Wetterdienst',
      'https://www.dwd.de/stuttgart',
      'Vorhersage fuer heute mit Temperatur und Niederschlag.',
      '',
      '2. Stuttgart Wetter | wetter.com',
      'https://www.wetter.com/stuttgart',
      'Stundenweise Wetterdaten fuer Stuttgart.',
    ].join('\n'))

    expect(parsed).toEqual([
      {
        title: 'Wetter Stuttgart heute - Deutscher Wetterdienst',
        url: 'https://www.dwd.de/stuttgart',
        snippet: 'Vorhersage fuer heute mit Temperatur und Niederschlag.',
      },
      {
        title: 'Stuttgart Wetter | wetter.com',
        url: 'https://www.wetter.com/stuttgart',
        snippet: 'Stundenweise Wetterdaten fuer Stuttgart.',
      },
    ])
  })

  it('appends a visible Quellen block only once', () => {
    const sources = mergeWebSearchSources([], [
      { title: 'DWD', url: 'https://www.dwd.de/stuttgart', snippet: 'Vorhersage fuer heute.' },
      { title: 'DWD', url: 'https://www.dwd.de/stuttgart', snippet: 'Duplikat.' },
    ])

    const withSources = appendWebSearchSources('Heute wird es mild und wechselhaft.', sources)

    expect(withSources).toContain('Quellen:')
    expect(withSources).toContain('https://www.dwd.de/stuttgart')
    expect(withSources.match(/https:\/\/www\.dwd\.de\/stuttgart/g)).toHaveLength(1)
    expect(appendWebSearchSources(withSources, sources)).toBe(withSources)
    expect(formatWebSearchSourcesBlock(sources)).toContain('1. DWD')
  })

  it('extracts appended sources back into a compact structure', () => {
    const answer = appendWebSearchSources('Heute wird es mild und wechselhaft.', [
      { title: 'DWD', url: 'https://www.dwd.de/stuttgart', snippet: 'Vorhersage fuer heute.' },
    ])

    const extracted = extractWebSearchSources(answer)

    expect(extracted.content).toBe('Heute wird es mild und wechselhaft.')
    expect(extracted.sources).toEqual([
      {
        title: 'DWD',
        url: 'https://www.dwd.de/stuttgart',
        snippet: 'Vorhersage fuer heute.',
      },
    ])
  })
})