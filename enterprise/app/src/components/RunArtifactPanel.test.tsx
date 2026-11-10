import { act, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { RunArtifact } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import RunArtifactPanel from './RunArtifactPanel'

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => { resolve = done })
  return { promise, resolve }
}

function artifact(runId: string, id: string, name: string): RunArtifact {
  return {
    schema_version: 2, id, run_id: runId, revision: 1, kind: 'output',
    media_type: 'text/plain', name, digest: 'a'.repeat(64), size_bytes: 4,
    metadata: {}, created_at: '2026-08-30T12:00:00Z', deleted_at: null,
  }
}

describe('RunArtifactPanel', () => {
  beforeEach(() => {
    vi.stubGlobal('URL', {
      ...URL,
      createObjectURL: vi.fn(() => 'blob:preview'),
      revokeObjectURL: vi.fn(),
    })
  })

  afterEach(() => vi.unstubAllGlobals())

  it('ignores an artifact list that belongs to the previous run', async () => {
    const runA = '10000000-0000-4000-8000-000000000301'
    const runB = '10000000-0000-4000-8000-000000000302'
    const requestA = deferred<RunArtifact[]>()
    const requestB = deferred<RunArtifact[]>()
    const client = {
      listArtifacts: vi.fn((id: string) => id === runA ? requestA.promise : requestB.promise),
    } as unknown as RemoteRuntimeClient
    const view = render(<RunArtifactPanel client={client} runId={runA} />)
    view.rerender(<RunArtifactPanel client={client} runId={runB} />)

    await act(async () => requestB.resolve([artifact(runB, '10000000-0000-4000-8000-000000000303', 'B.txt')]))
    expect(await screen.findByText('B.txt')).toBeInTheDocument()
    await act(async () => requestA.resolve([artifact(runA, '10000000-0000-4000-8000-000000000304', 'A.txt')]))
    expect(screen.queryByText('A.txt')).not.toBeInTheDocument()
  })

  it('discards and revokes a preview downloaded for the previous run', async () => {
    const runA = '10000000-0000-4000-8000-000000000311'
    const runB = '10000000-0000-4000-8000-000000000312'
    const download = deferred<Blob>()
    const artifactA = artifact(runA, '10000000-0000-4000-8000-000000000313', 'A.txt')
    const client = {
      listArtifacts: vi.fn(async (id: string) => id === runA ? [artifactA] : []),
      downloadArtifact: vi.fn(() => download.promise),
    } as unknown as RemoteRuntimeClient
    const view = render(<RunArtifactPanel client={client} runId={runA} />)
    fireEvent.click(await screen.findByRole('button', { name: /A\.txt/ }))
    view.rerender(<RunArtifactPanel client={client} runId={runB} />)

    await act(async () => download.resolve(new Blob(['old'], { type: 'text/plain' })))
    expect(screen.queryByText('old')).not.toBeInTheDocument()
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:preview')
    expect(client.downloadArtifact).toHaveBeenCalledWith(runA, artifactA.id)
  })
})
