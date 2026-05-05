import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import CrewLiveMonitor from './CrewLiveMonitor'
import type { CrewLiveState, CrewLiveEntry } from '../stores/chatStore'

function createEntry(index: number, overrides: Partial<CrewLiveEntry> = {}): CrewLiveEntry {
  return {
    id: `entry-${index}`,
    timestamp: 1_700_000_000_000 + index,
    agentId: 'agent-architect',
    taskId: `task-${index}`,
    action: 'runtime_stdout',
    category: 'output',
    title: `Zeile ${index}`,
    detail: '',
    ...overrides,
  }
}

function createLive(entries: CrewLiveEntry[]): CrewLiveState {
  return {
    streamId: 'stream-1',
    title: 'Crew Testlauf',
    status: 'running',
    entries,
    agentColors: {
      'agent-architect': '#2563eb',
    },
    updatedAt: Date.now(),
  }
}

describe('CrewLiveMonitor', () => {
  it('shows only the last 150 log lines in the rolling window', () => {
    const entries = Array.from({ length: 160 }, (_, index) => createEntry(index + 1))

    const { container } = render(<CrewLiveMonitor live={createLive(entries)} />)

    expect(container.querySelectorAll('.crew-live-line')).toHaveLength(150)
    expect(screen.getByText('150 / 150 Zeilen')).toBeInTheDocument()
    expect(screen.queryByText('Zeile 1')).not.toBeInTheDocument()
    expect(screen.getByText('Zeile 160')).toBeInTheDocument()
  })

  it('keeps tool and error rows separately highlighted in the compact log', () => {
    const entries = [
      createEntry(1, {
        category: 'tool',
        title: 'Tool: ReadFile',
        detail: 'Args: README.md\nResult: ok',
      }),
      createEntry(2, {
        category: 'error',
        title: 'Crew-Fehler',
        detail: 'Traceback: boom',
      }),
    ]

    const { container } = render(<CrewLiveMonitor live={createLive(entries)} />)

    expect(screen.getByText('Tool: ReadFile').closest('.crew-live-line')).toHaveClass('tone-tool')
    expect(screen.getByText('Crew-Fehler').closest('.crew-live-line')).toHaveClass('tone-error')
    expect(screen.getByText('Args')).toBeInTheDocument()
    expect(screen.getByText('README.md')).toBeInTheDocument()
    expect(container.querySelectorAll('.tone-tool').length).toBeGreaterThan(0)
    expect(container.querySelectorAll('.tone-error').length).toBeGreaterThan(0)
  })
})