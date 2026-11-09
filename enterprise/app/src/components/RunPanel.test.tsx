import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import RunPanel from './RunPanel'
import { useEngineStore } from '../stores/engineStore'

const invokeMock = vi.fn()

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => { resolve = done })
  return { promise, resolve }
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

describe('RunPanel', () => {
  beforeEach(() => {
    invokeMock.mockReset()
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      value: {},
      configurable: true,
      writable: true,
    })
    useEngineStore.setState({ currentRunId: 'run-1' })
  })

  it('accepts snake_case run payloads from Rust without crashing', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'engine_run_list') {
        return Promise.resolve([
          {
            id: 'run-1',
            parent_run_id: null,
            title: 'Plan migration',
            input_summary: 'Summarized input',
            status: 'running',
            phase: 'planning',
            cwd: 'C:/project',
            model: 'gpt-5.4',
            provider: 'openrouter',
            retry_count: 2,
            resumed_from_run_id: null,
            checkpoint_json: null,
            result_summary: null,
            error: null,
            updated_at: '2026-05-03T10:00:00Z',
            created_at: '2026-05-03T09:00:00Z',
          },
        ])
      }

      if (command === 'engine_run_checkpoint_list') {
        return Promise.resolve([
          {
            id: 'cp-1',
            run_id: 'run-1',
            label: 'Before tool call',
            snapshot_json: '{"step":"collect"}',
            created_at: '2026-05-03T10:01:00Z',
          },
        ])
      }

      if (command === 'engine_run_artifact_list') {
        return Promise.resolve([])
      }

      if (command === 'engine_run_event_list') {
        return Promise.resolve([])
      }

      if (command === 'worker_sandbox_get_for_run') {
        return Promise.resolve({
          id: 'sb-1',
          run_id: 'run-1',
          backend_id: 'local',
          status: 'running',
          mode: 'workspace-write',
          source_cwd: 'C:/project',
          workspace_root: 'C:/workspace',
          allow_file_read: true,
          allow_file_write: false,
          allow_shell_execution: true,
          allow_web_fetch: true,
          allow_web_search: false,
          allow_mcp: true,
        })
      }

      return Promise.reject(new Error(`Unexpected command: ${command}`))
    })

    render(<RunPanel />)

    await waitFor(() => {
      expect(screen.getByText((_content, element) => element?.textContent === 'Workspace: C:/workspace')).toBeInTheDocument()
      expect(screen.getByText('{"step":"collect"}')).toBeInTheDocument()
    })
    expect(screen.getByText((_content, element) => element?.textContent === 'Retry Count: 2')).toBeInTheDocument()
  })

  it('does not combine detail responses from different runs', async () => {
    const checkpointsA = deferred<unknown[]>()
    const checkpointsB = deferred<unknown[]>()
    const run = (id: string, title: string) => ({
      id, parent_run_id: null, thread_id: null, title, input_summary: null,
      source: 'desktop', status: 'running', phase: 'running', cwd: 'C:/project',
      workspace_path: 'C:/project', model: null, provider: null, provider_profile_id: null,
      runtime_mode: 'host', toolset_policy_id: null, channel_kind: null, channel_ref: null,
      retry_count: 0, resumed_from_run_id: null, checkpoint_json: null,
      result_summary: null, error: null, updated_at: '2026-08-30T12:00:00Z',
      created_at: '2026-08-30T12:00:00Z',
    })
    invokeMock.mockImplementation((command: string, args: { runId?: string }) => {
      if (command === 'engine_run_list') return Promise.resolve([run('run-a', 'Run A'), run('run-b', 'Run B')])
      if (command === 'engine_run_checkpoint_list') return args.runId === 'run-a' ? checkpointsA.promise : checkpointsB.promise
      if (command === 'engine_run_event_list' || command === 'engine_run_artifact_list') return Promise.resolve([])
      if (command === 'worker_sandbox_get_for_run') return Promise.resolve(null)
      return Promise.reject(new Error(`Unexpected command: ${command}`))
    })

    render(<RunPanel />)
    fireEvent.click(await screen.findByRole('button', { name: /Run B/ }))
    await act(async () => checkpointsB.resolve([{
      id: 'checkpoint-b', run_id: 'run-b', label: 'Checkpoint B', snapshot_json: '{}',
      created_at: '2026-08-30T12:01:00Z',
    }]))
    expect(await screen.findByText('Checkpoint B')).toBeInTheDocument()

    await act(async () => checkpointsA.resolve([{
      id: 'checkpoint-a', run_id: 'run-a', label: 'Checkpoint A', snapshot_json: '{}',
      created_at: '2026-08-30T12:00:00Z',
    }]))
    expect(screen.queryByText('Checkpoint A')).not.toBeInTheDocument()
    expect(screen.getByText('Checkpoint B')).toBeInTheDocument()
  })
})
