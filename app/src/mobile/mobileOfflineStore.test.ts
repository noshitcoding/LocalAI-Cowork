import { beforeEach, describe, expect, it } from 'vitest'

import type { RunRecord } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import { createOfflineThreadMessageOperation, EMPTY_MOBILE_OFFLINE_STATE, flushMobileOutbox, loadMobileOfflineState, saveMobileOfflineState } from './mobileOfflineStore'
import { resetMobileSecureForTests } from './mobileSecure'

describe('encrypted Android offline state', () => {
  beforeEach(() => {
    localStorage.clear()
    resetMobileSecureForTests()
  })

  it('encrypts and restores the offline outbox', async () => {
    const secretRunId = '58e5435f-c495-47fb-87c7-c21fde0ca2bc'
    await saveMobileOfflineState({
      ...EMPTY_MOBILE_OFFLINE_STATE,
      outbox: [{
        id: 'b6584aef-87c6-48b6-9acb-4acdeed6d7a6',
        kind: 'cancel_run',
        runId: secretRunId,
        createdAt: '2026-08-08T12:00:00.000Z',
        attempts: 0,
      }],
    })
    const ciphertext = localStorage.getItem('open-cowork-mobile-cache-v1')
    expect(ciphertext).toBeTruthy()
    expect(ciphertext).not.toContain(secretRunId)
    const restored = await loadMobileOfflineState()
    expect(restored.outbox).toHaveLength(1)
    expect(restored.outbox[0]).toMatchObject({ kind: 'cancel_run', runId: secretRunId })
  })

  it('encrypts and restores cached thread messages', async () => {
    const threadId = '58e5435f-c495-47fb-87c7-c21fde0ca2bc'
    await saveMobileOfflineState({
      ...EMPTY_MOBILE_OFFLINE_STATE,
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
    const ciphertext = localStorage.getItem('open-cowork-mobile-cache-v1')
    expect(ciphertext).not.toContain('private cached answer')
    const restored = await loadMobileOfflineState()
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
    await saveMobileOfflineState({ ...EMPTY_MOBILE_OFFLINE_STATE, outbox: [operation] })
    expect(localStorage.getItem('open-cowork-mobile-cache-v1')).not.toContain('Continue the report')
    expect((await loadMobileOfflineState()).outbox).toEqual([operation])
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

  it('uses a fresh random IV for every save', async () => {
    await saveMobileOfflineState(EMPTY_MOBILE_OFFLINE_STATE)
    const first = localStorage.getItem('open-cowork-mobile-cache-v1')
    await saveMobileOfflineState(EMPTY_MOBILE_OFFLINE_STATE)
    expect(localStorage.getItem('open-cowork-mobile-cache-v1')).not.toBe(first)
  })

  it('rejects tampering and removes the unreadable cache', async () => {
    await saveMobileOfflineState(EMPTY_MOBILE_OFFLINE_STATE)
    const encoded = localStorage.getItem('open-cowork-mobile-cache-v1')!
    localStorage.setItem('open-cowork-mobile-cache-v1', `${encoded.slice(0, -2)}AA`)
    await expect(loadMobileOfflineState()).rejects.toThrow(/could not be opened/)
    expect(localStorage.getItem('open-cowork-mobile-cache-v1')).toBeNull()
  })
})
