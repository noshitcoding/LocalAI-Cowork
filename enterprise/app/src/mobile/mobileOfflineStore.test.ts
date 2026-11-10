import { beforeEach, describe, expect, it } from 'vitest'

import type { RunRecord } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import { createOfflineThreadMessageOperation, EMPTY_MOBILE_OFFLINE_STATE, mobileCacheBinding, flushMobileOutbox, loadMobileOfflineState, mergeMobileOutboxAfterFlush, saveMobileOfflineState } from './mobileOfflineStore'
import { resetMobileSecureForTests } from './mobileSecure'

const binding = mobileCacheBinding('https://fixture.example', 'verified-fixture-user')
const BOUND_STATE = { ...EMPTY_MOBILE_OFFLINE_STATE, schemaVersion: 2 as const, binding }
function cacheStorageKey(): string {
  const key = Object.keys(localStorage).find(key => key.startsWith('open-cowork-mobile-cache-v2:'))
  if (!key) throw new Error('The account-bound cache was not written')
  return key
}

describe('encrypted Android offline state', () => {
  it('requires a verified account and does not disclose another account cache', async () => {
    await expect(saveMobileOfflineState(EMPTY_MOBILE_OFFLINE_STATE)).rejects.toThrow(/verified account/)
    await saveMobileOfflineState({ ...BOUND_STATE, outbox: [{ id: 'private', kind: 'cancel_run', runId: 'private-run', createdAt: new Date().toISOString(), attempts: 0 }] })
    const other = await loadMobileOfflineState(mobileCacheBinding('https://fixture.example', 'another-user'))
    expect(other.outbox).toEqual([])
    expect((await loadMobileOfflineState(binding)).outbox).toHaveLength(1)
  })

  beforeEach(() => {
    localStorage.clear()
    resetMobileSecureForTests()
  })

  it('encrypts and restores the offline outbox', async () => {
    const secretRunId = '58e5435f-c495-47fb-87c7-c21fde0ca2bc'
    await saveMobileOfflineState({
      ...BOUND_STATE,
      outbox: [{
        id: 'b6584aef-87c6-48b6-9acb-4acdeed6d7a6',
        kind: 'cancel_run',
        runId: secretRunId,
        createdAt: '2026-08-08T12:00:00.000Z',
        attempts: 0,
      }],
    })
    const ciphertext = localStorage.getItem(cacheStorageKey())
    expect(ciphertext).toBeTruthy()
    expect(ciphertext).not.toContain(secretRunId)
    const restored = await loadMobileOfflineState(binding)
    expect(restored.outbox).toHaveLength(1)
    expect(restored.outbox[0]).toMatchObject({ kind: 'cancel_run', runId: secretRunId })
  })

  it('encrypts and restores cached thread messages', async () => {
    const threadId = '58e5435f-c495-47fb-87c7-c21fde0ca2bc'
    await saveMobileOfflineState({
      ...BOUND_STATE,
      messages: {
        [threadId]: [{
          schema_version: 2,
          id: 'b6584aef-87c6-48b6-9acb-4acdeed6d7a6',
          revision: 1,
          etag: 'W/"message:1"',
          thread_id: threadId,
          author_user_id: null,
          role: 'assistant',
          content: { text: 'private cached answer' },
          run_id: null,
          created_at: '2026-08-10T12:00:00Z',
          updated_at: '2026-08-10T12:00:00Z',
          deleted_at: null,
        }],
      },
    })
    const ciphertext = localStorage.getItem(cacheStorageKey())
    expect(ciphertext).not.toContain('private cached answer')
    const restored = await loadMobileOfflineState(binding)
    expect(restored.messages[threadId]?.[0]?.content).toEqual({ text: 'private cached answer' })
  })

  it('freezes an offline thread reply with a stable idempotency key', async () => {
    const operationId = 'b6584aef-87c6-48b6-9acb-4acdeed6d7a6'
    const run = {
      spec: {
        thread_id: '58e5435f-c495-47fb-87c7-c21fde0ca2bc',
        project_id: '58e5435f-c495-47fb-87c7-c21fde0ca2bd',
        project: { id: '58e5435f-c495-47fb-87c7-c21fde0ca2bd', revision: 4 },
        project_privacy: 'private_local',
        executor_target: { kind: 'personal_device', device_id: '58e5435f-c495-47fb-87c7-c21fde0ca2be' },
        required_capabilities: ['files'],
        model_profile_id: '58e5435f-c495-47fb-87c7-c21fde0ca2bf',
      },
    } as unknown as RunRecord
    const operation = createOfflineThreadMessageOperation(
      run, '  Continue the report  ', operationId, '2026-08-08T12:00:00.000Z',
    )

    expect(operation).toMatchObject({
      id: operationId,
      kind: 'thread_message',
      threadId: run.spec.thread_id,
      request: {
        content: { text: 'Continue the report' },
        run: {
          project_revision: 4,
          project_privacy: 'private_local',
          task: null,
          executor_target: run.spec.executor_target,
          required_capabilities: ['files'],
          model_profile_id: run.spec.model_profile_id,
          snapshot_id: null,
          idempotency_key: operationId,
        },
      },
    })
    await saveMobileOfflineState({ ...BOUND_STATE, outbox: [operation] })
    expect(localStorage.getItem(cacheStorageKey())).not.toContain('Continue the report')
    expect((await loadMobileOfflineState(binding)).outbox).toEqual([operation])
  })

  it('flushes actions serially and retains failed operations with diagnostics', async () => {
    const run = {
      spec: {
        id: '58e5435f-c495-47fb-87c7-c21fde0ca2c0',
        thread_id: '58e5435f-c495-47fb-87c7-c21fde0ca2bc',
        project_id: '58e5435f-c495-47fb-87c7-c21fde0ca2bd',
        project: { id: '58e5435f-c495-47fb-87c7-c21fde0ca2bd', revision: 4 },
        project_privacy: 'private_local',
        executor_target: { kind: 'personal_device', device_id: '58e5435f-c495-47fb-87c7-c21fde0ca2be' },
        required_capabilities: [],
      },
    } as unknown as RunRecord
    const reply = createOfflineThreadMessageOperation(
      run, 'Resume', 'b6584aef-87c6-48b6-9acb-4acdeed6d7a6', '2026-08-08T12:00:00.000Z',
    )
    const cancel = {
      id: 'b6584aef-87c6-48b6-9acb-4acdeed6d7a7', kind: 'cancel_run' as const,
      runId: run.spec.id, createdAt: '2026-08-08T12:01:00.000Z', attempts: 1,
    }
    const calls: string[] = []
    const client = {
      createThreadMessage: async () => { calls.push('reply'); return { run } },
      cancelRun: async () => { calls.push('cancel'); throw new Error('temporarily unavailable') },
    } as unknown as Pick<RemoteRuntimeClient, 'cancelRun' | 'createThreadMessage'>

    const flushed = await flushMobileOutbox(client, [reply, cancel])
    expect(calls).toEqual(['reply', 'cancel'])
    expect(flushed.createdRuns).toEqual([run])
    expect(flushed.remaining).toEqual([{
      ...cancel, attempts: 2, lastError: 'temporarily unavailable',
    }])
  })

  it('stops at the first failure and preserves the untouched global FIFO tail', async () => {
    const first = {
      id: 'first', kind: 'cancel_run' as const, runId: 'run-1',
      createdAt: '2026-08-08T12:00:00.000Z', attempts: 2,
    }
    const second = {
      id: 'second', kind: 'cancel_run' as const, runId: 'run-2',
      createdAt: '2026-08-08T12:01:00.000Z', attempts: 0,
    }
    const calls: string[] = []
    const client = {
      cancelRun: async (runId: string) => {
        calls.push(runId)
        if (runId === 'run-1') throw new Error('offline')
      },
      createThreadMessage: async () => { throw new Error('unexpected') },
    } as unknown as Pick<RemoteRuntimeClient, 'cancelRun' | 'createThreadMessage'>

    const failed = await flushMobileOutbox(client, [first, second])
    expect(calls).toEqual(['run-1'])
    expect(failed.remaining).toEqual([
      { ...first, attempts: 3, lastError: 'offline' },
      second,
    ])

    calls.length = 0
    const succeeded = await flushMobileOutbox(client, failed.remaining.map((operation) => (
      operation.id === 'first' ? { ...operation, runId: 'recovered' } : operation
    )))
    expect(calls).toEqual(['recovered', 'run-2'])
    expect(succeeded.remaining).toEqual([])
  })

  it('uses a fresh random IV for every save', async () => {
    await saveMobileOfflineState(BOUND_STATE)
    const first = localStorage.getItem(cacheStorageKey())
    await saveMobileOfflineState(BOUND_STATE)
    expect(localStorage.getItem(cacheStorageKey())).not.toBe(first)
  })

  it('serializes concurrent cache writes so the latest requested state wins', async () => {
    const states = Array.from({ length: 10 }, (_, index) => ({
      ...BOUND_STATE,
      outbox: [{
        id: `operation-${index}`,
        kind: 'cancel_run' as const,
        runId: `run-${index}`,
        createdAt: `2026-08-08T12:00:0${index}.000Z`,
        attempts: 0,
      }],
    }))

    await Promise.all(states.map((state) => saveMobileOfflineState(state)))

    expect((await loadMobileOfflineState(binding)).outbox).toEqual(states.at(-1)?.outbox)
  })

  it('merges a flush result into the latest outbox without deleting new or restoring discarded work', () => {
    const flushed = {
      id: 'flushed', kind: 'cancel_run' as const, runId: 'run-old',
      createdAt: '2026-08-08T12:00:00.000Z', attempts: 0,
    }
    const addedDuringFlush = {
      id: 'new', kind: 'cancel_run' as const, runId: 'run-new',
      createdAt: '2026-08-08T12:01:00.000Z', attempts: 0,
    }

    expect(mergeMobileOutboxAfterFlush(
      [flushed, addedDuringFlush],
      [flushed],
      [],
    )).toEqual([addedDuringFlush])
    expect(mergeMobileOutboxAfterFlush(
      [],
      [flushed],
      [{ ...flushed, attempts: 1, lastError: 'offline' }],
    )).toEqual([])
  })

  it('rejects tampering and preserves the encrypted source for recovery', async () => {
    await saveMobileOfflineState(BOUND_STATE)
    const encoded = localStorage.getItem(cacheStorageKey())!
    localStorage.setItem(cacheStorageKey(), `${encoded.slice(0, -2)}AA`)
    await expect(loadMobileOfflineState(binding)).rejects.toThrow(/could not be opened/)
    expect(localStorage.getItem(cacheStorageKey())).toBe(`${encoded.slice(0, -2)}AA`)
  })
})
