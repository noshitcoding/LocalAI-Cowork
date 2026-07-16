import { describe, expect, it } from 'vitest'
import { normalizeEngineRunArtifact, normalizeEngineRunEvent } from './engineRunRecords'

describe('engine run record presentation', () => {
  it('normalizes snake_case events from the Rust boundary', () => {
    expect(normalizeEngineRunEvent({
      id: 'event-1',
      run_id: 'run-1',
      sequence: '7',
      event_type: 'tool_completed',
      summary: 'Report written',
      payload_json: '{"path":"report.md"}',
      redaction_level: 'metadata',
      created_at: '2026-07-12T20:00:00Z',
    })).toEqual({
      id: 'event-1',
      runId: 'run-1',
      sequence: 7,
      eventType: 'tool_completed',
      summary: 'Report written',
      payloadJson: '{"path":"report.md"}',
      redactionLevel: 'metadata',
      createdAt: '2026-07-12T20:00:00Z',
    })
  })

  it('normalizes camelCase artifacts and rejects records without an id', () => {
    expect(normalizeEngineRunArtifact({
      id: 'artifact-1',
      runId: 'run-1',
      kind: 'markdown',
      path: 'C:/workspace/report.md',
      title: 'Release report',
      summary: 'Verified release findings',
      createdAt: '2026-07-12T20:01:00Z',
    })).toMatchObject({ id: 'artifact-1', runId: 'run-1', title: 'Release report' })
    expect(normalizeEngineRunEvent({ summary: 'missing id' })).toBeNull()
    expect(normalizeEngineRunArtifact(null)).toBeNull()
  })
})
