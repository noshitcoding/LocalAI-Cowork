import { describe, expect, it } from 'vitest'
import { buildKnowledgeImportEntries, chunkKnowledgeText } from './knowledgeImport'

describe('knowledgeImport', () => {
  it('chunks long sources with bounded overlap and no empty entries', () => {
    const source = Array.from({ length: 40 }, (_, index) => `Section ${index}: ${'x'.repeat(110)}`).join('\n\n')
    const chunks = chunkKnowledgeText(source, 1000, 120)

    expect(chunks.length).toBeGreaterThan(1)
    expect(chunks.every((chunk) => chunk.length > 0 && chunk.length <= 1000)).toBe(true)
    expect(chunks.join(' ')).toContain('Section 0')
    expect(chunks.join(' ')).toContain('Section 39')
  })

  it('creates shared knowledge entries with stable source labels', () => {
    let id = 0
    const entries = buildKnowledgeImportEntries(
      '  API   Handbook  ',
      `Intro\n\n${'contract '.repeat(700)}`,
      () => `knowledge-${++id}`,
    )

    expect(entries.length).toBeGreaterThan(1)
    expect(entries[0]).toMatchObject({
      id: 'knowledge-1',
      scope: 'shared',
      category: 'knowledge',
      confidence: 1,
    })
    expect(entries[0].key).toMatch(/^API Handbook \(1\//)
  })

  it('rejects empty or null-only sources', () => {
    expect(buildKnowledgeImportEntries('Empty', ' \n\0\0 ')).toEqual([])
  })
})
