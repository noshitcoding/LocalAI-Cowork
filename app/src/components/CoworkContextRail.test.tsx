import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Task } from '../stores/taskStore'
import { safeInvoke } from '../utils/safeInvoke'
import CoworkContextRail from './CoworkContextRail'

vi.mock('../utils/safeInvoke', () => ({ safeInvoke: vi.fn() }))

const safeInvokeMock = vi.mocked(safeInvoke)

const task: Task = {
  id: 'task-1',
  title: 'Release review',
  prompt: 'Review the release',
  status: 'running',
  threadId: 'thread-1',
  createdAt: 1,
  updatedAt: 2,
  error: null,
  steps: [
    { id: 'step-1', index: 0, title: 'Inspect', state: 'completed', requiresApproval: false, riskLevel: 'low', output: 'Inspection report' },
    { id: 'step-2', index: 1, title: 'Publish', state: 'running', requiresApproval: true, riskLevel: 'high', output: null },
  ],
}

const baseProps = {
  open: true,
  engineStatus: 'idle' as const,
  error: null,
  sessionId: 'session-123456789',
  runId: null,
  providerLabel: 'Ollama',
  model: 'llama3.1:8b',
  workingContext: 'C:\\workspace',
  contextWarning: { level: 'none' as const, estimatedTokens: 0 },
  compactionCount: 0,
  approvalSteps: [],
  toolCalls: [],
  task: null,
  onClose: vi.fn(),
  onStop: vi.fn(),
  onOpenRuns: vi.fn(),
  onOpenTasks: vi.fn(),
}

describe('CoworkContextRail', () => {
  beforeEach(() => {
    safeInvokeMock.mockReset()
    safeInvokeMock.mockResolvedValue([])
  })

  it('presents the current environment and useful empty states', () => {
    render(<CoworkContextRail {...baseProps} />)

    expect(screen.getByRole('complementary', { name: 'Run context' })).toBeInTheDocument()
    expect(screen.getByText('llama3.1:8b')).toBeInTheDocument()
    expect(screen.getByText('A plan appears here when the task needs multiple steps or approval.')).toBeInTheDocument()
    expect(screen.getByText('Tool calls will appear here with their live status.')).toBeInTheDocument()
  })

  it('surfaces approvals, task progress, tools and outputs from real run state', () => {
    const { rerender } = render(
      <CoworkContextRail {...baseProps} engineStatus="waiting_approval" approvalSteps={['Allow write to report.md']} task={task} />,
    )
    expect(screen.getByText('Your decision is required')).toBeInTheDocument()
    expect(screen.getByText('Allow write to report.md')).toBeInTheDocument()

    rerender(
      <CoworkContextRail
        {...baseProps}
        engineStatus="tool_running"
        task={task}
        toolCalls={[{ id: 'tool-1', toolName: 'read_file', input: {}, status: 'running', startedAt: 1 }]}
      />,
    )
    expect(screen.getByText('1/2')).toBeInTheDocument()
    expect(screen.getByText('read_file')).toBeInTheDocument()
    expect(screen.getByText('Inspection report')).toBeInTheDocument()
  })

  it('keeps stop, navigation and close actions operable', () => {
    render(<CoworkContextRail {...baseProps} engineStatus="streaming" runId="run-123456789" />)

    fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
    fireEvent.click(screen.getByRole('button', { name: 'Open tasks' }))
    fireEvent.click(screen.getByRole('button', { name: 'Open run history' }))
    fireEvent.click(screen.getByRole('button', { name: 'Close run context' }))

    expect(baseProps.onStop).toHaveBeenCalledTimes(1)
    expect(baseProps.onOpenTasks).toHaveBeenCalledTimes(1)
    expect(baseProps.onOpenRuns).toHaveBeenCalledTimes(1)
    expect(baseProps.onClose).toHaveBeenCalledTimes(1)
  })

  it('loads persisted events and artifacts for the active run', async () => {
    safeInvokeMock.mockImplementation(async (command) => {
      if (command === 'engine_run_event_list') {
        return [{ id: 'event-1', run_id: 'run-1', sequence: 4, event_type: 'artifact_written', summary: 'Wrote release report', created_at: '2026-07-12T20:00:00Z' }]
      }
      if (command === 'engine_run_artifact_list') {
        return [{ id: 'artifact-1', run_id: 'run-1', kind: 'pdf', path: 'C:/workspace/report.pdf', title: 'Release report', summary: 'Verified findings', created_at: '2026-07-12T20:00:01Z' }]
      }
      if (command === 'office_open_document') return { launched: true }
      return []
    })

    render(<CoworkContextRail {...baseProps} runId="run-1" />)

    await waitFor(() => expect(screen.getByText('Wrote release report')).toBeInTheDocument())
    expect(screen.getByText('Release report')).toBeInTheDocument()
    expect(screen.getByText('Verified findings')).toBeInTheDocument()
    expect(safeInvokeMock).toHaveBeenCalledWith('engine_run_event_list', { runId: 'run-1', limit: 12 }, [])

    fireEvent.click(screen.getByRole('button', { name: 'Open output: Release report' }))
    await waitFor(() => expect(safeInvokeMock).toHaveBeenCalledWith('office_open_document', {
      request: { path: 'C:/workspace/report.pdf' },
      runId: 'run-1',
    }))
  })

  it('keeps an output failure visible without losing the artifact', async () => {
    safeInvokeMock.mockImplementation(async (command) => {
      if (command === 'engine_run_event_list') return []
      if (command === 'engine_run_artifact_list') {
        return [{ id: 'artifact-1', run_id: 'run-1', kind: 'pdf', path: 'C:/workspace/report.pdf', title: 'Release report' }]
      }
      if (command === 'office_open_document') throw new Error('No PDF viewer is available.')
      return []
    })

    render(<CoworkContextRail {...baseProps} runId="run-1" />)
    fireEvent.click(await screen.findByRole('button', { name: 'Open output: Release report' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('No PDF viewer is available.')
    expect(screen.getByText('Release report')).toBeInTheDocument()
  })
})
