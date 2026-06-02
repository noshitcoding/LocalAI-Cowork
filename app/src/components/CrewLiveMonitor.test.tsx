import { fireEvent, render, screen, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import CrewLiveMonitor from './CrewLiveMonitor'
import type { CrewLiveState, CrewLiveEntry } from '../stores/chatStore'

const AGENT_COLORS = {
  'agent-architect': '#2563eb',
  'agent-builder': '#059669',
  'agent-reviewer': '#d97706',
  'agent-researcher': '#7c3aed',
}

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
    agentColors: AGENT_COLORS,
    updatedAt: Date.now(),
  }
}

describe('CrewLiveMonitor', () => {
  it('keeps the full event history while virtualizing rendered log lines', () => {
    const entries = Array.from({ length: 510 }, (_, index) => createEntry(index + 1))

    const { container } = render(<CrewLiveMonitor live={createLive(entries)} />)
    const log = screen.getByLabelText('Crew-Live-Log')

    expect(container.querySelectorAll('.crew-live-line').length).toBeLessThan(510)
    expect(screen.getByText('510 Zeilen')).toBeInTheDocument()
    expect(log).toHaveAttribute('aria-rowcount', '510')
    expect(within(log).getByText('Zeile 1')).toBeInTheDocument()
  })

  it('renders the three most recently active agents in focus columns', () => {
    const entries = [
      createEntry(1, { agentId: 'agent-architect', title: 'Architect arbeitet' }),
      createEntry(2, { agentId: 'agent-builder', title: 'Builder arbeitet' }),
      createEntry(3, { agentId: 'agent-reviewer', title: 'Reviewer arbeitet' }),
      createEntry(4, { agentId: 'agent-researcher', title: 'Researcher arbeitet' }),
    ]

    const { container } = render(<CrewLiveMonitor live={createLive(entries)} />)
    const columns = container.querySelectorAll('.crew-live-focus-column')

    expect(columns).toHaveLength(3)
    expect(within(columns[0] as HTMLElement).getByText('researcher')).toBeInTheDocument()
    expect(within(columns[1] as HTMLElement).getByText('reviewer')).toBeInTheDocument()
    expect(within(columns[2] as HTMLElement).getByText('builder')).toBeInTheDocument()
  })

  it('uses gallery arrows for manual focus and keeps that focus across new activity', () => {
    const initial = [
      createEntry(1, { agentId: 'agent-architect', title: 'Architect arbeitet' }),
      createEntry(2, { agentId: 'agent-builder', title: 'Builder arbeitet' }),
      createEntry(3, { agentId: 'agent-reviewer', title: 'Reviewer arbeitet' }),
      createEntry(4, { agentId: 'agent-researcher', title: 'Researcher arbeitet' }),
    ]
    const { container, rerender } = render(<CrewLiveMonitor live={createLive(initial)} />)

    fireEvent.click(screen.getByRole('button', { name: 'Naechste Personen anzeigen' }))

    let columns = container.querySelectorAll('.crew-live-focus-column')
    expect(within(columns[0] as HTMLElement).getByText('reviewer')).toBeInTheDocument()
    expect(within(columns[1] as HTMLElement).getByText('builder')).toBeInTheDocument()
    expect(within(columns[2] as HTMLElement).getByText('architect')).toBeInTheDocument()

    rerender(<CrewLiveMonitor live={createLive([
      ...initial,
      createEntry(5, { agentId: 'agent-researcher', title: 'Researcher neuer Output' }),
    ])} />)

    columns = container.querySelectorAll('.crew-live-focus-column')
    expect(within(columns[0] as HTMLElement).getByText('reviewer')).toBeInTheDocument()
    expect(within(columns[1] as HTMLElement).getByText('builder')).toBeInTheDocument()
    expect(within(columns[2] as HTMLElement).getByText('architect')).toBeInTheDocument()
  })

  it('filters MCP, tool, error and runtime events via category chips', () => {
    const entries = [
      createEntry(1, {
        category: 'tool',
        title: 'Tool: ReadFile',
        detail: 'Args: README.md\nResult: ok',
      }),
      createEntry(2, {
        category: 'mcp',
        title: 'MCP-Kontext oder MCP-Zugriff',
        detail: 'Server: workspace\nTool: search',
      }),
      createEntry(3, {
        category: 'error',
        title: 'Crew-Fehler',
        detail: 'Traceback: boom',
      }),
      createEntry(4, {
        agentId: 'python-runtime',
        action: 'runtime_context',
        category: 'context',
        title: 'Runtime-Kontext geladen',
        detail: 'subject=workspace-user',
      }),
    ]

    render(<CrewLiveMonitor live={createLive(entries)} />)

    fireEvent.click(screen.getByRole('button', { name: 'Filter MCP' }))
    let log = screen.getByLabelText('Crew-Live-Log')
    expect(within(log).getByText('MCP-Kontext oder MCP-Zugriff').closest('.crew-live-line')).toHaveClass('tone-mcp')
    expect(within(log).queryByText('Crew-Fehler')).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Filter Tool' }))
    log = screen.getByLabelText('Crew-Live-Log')
    expect(within(log).getByText('Tool: ReadFile').closest('.crew-live-line')).toHaveClass('tone-tool')
    expect(within(log).queryByText('Runtime-Kontext geladen')).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Filter Fehler' }))
    log = screen.getByLabelText('Crew-Live-Log')
    expect(within(log).getByText('Crew-Fehler').closest('.crew-live-line')).toHaveClass('tone-error')

    fireEvent.click(screen.getByRole('button', { name: 'Filter Runtime' }))
    log = screen.getByLabelText('Crew-Live-Log')
    expect(within(log).getByText('Runtime-Kontext geladen').closest('.crew-live-line')).toHaveClass('tone-runtime')
  })

  it('uses speaking agent names as primary labels and keeps technical ids in details', () => {
    const technicalId = 'personality-pers-1777902878654-89d3uh'
    const { container } = render(<CrewLiveMonitor live={createLive([
      createEntry(1, {
        agentId: technicalId,
        rawAgentId: technicalId,
        agentName: 'Kreativer',
        category: 'agent',
        title: 'Kreativer bereit',
        detail: `Technische ID: ${technicalId}\nRole: custom`,
      }),
    ])} />)

    expect(container.querySelector('.crew-live-focus-name')).toHaveTextContent('Kreativer')
    expect(container.querySelector('.crew-live-focus-name')).not.toHaveTextContent(technicalId)
    expect(screen.getAllByText('Kreativer').length).toBeGreaterThan(0)
    expect(screen.getAllByText(technicalId).length).toBeGreaterThan(0)
  })

  it('shows handoffs with source, target, model, task and filter support', () => {
    render(<CrewLiveMonitor live={createLive([
      createEntry(1, {
        category: 'handoff',
        action: 'task_handoff',
        title: 'Analyst -> Executor',
        summary: 'Task uebergeben',
        sourceAgent: 'Analyst',
        targetAgent: 'Executor',
        agentName: 'Executor',
        provider: 'openrouter',
        model: 'anthropic/claude',
        taskTitle: 'Spiel verbessern',
        detail: 'Status: gestartet',
      }),
      createEntry(2, {
        category: 'thinking',
        action: 'thinking_phase',
        title: 'Arbeitsprozess: Executor',
        detail: 'Arbeitsprotokoll: Executor prueft den Task.',
      }),
    ])} />)

    expect(screen.getAllByText(/Analyst -> Executor/).length).toBeGreaterThan(0)
    expect(screen.getAllByText(/Modell anthropic\/claude/).length).toBeGreaterThan(0)

    fireEvent.click(screen.getByRole('button', { name: 'Filter Uebergabe' }))
    const log = screen.getByLabelText('Crew-Live-Log')
    expect(within(log).getByText(/Analyst -> Executor/).closest('.crew-live-line')).toHaveClass('tone-handoff')
    expect(within(log).queryByText('Arbeitsprozess: Executor')).not.toBeInTheDocument()
  })

  it('collapses long details and expands them on demand', () => {
    const longDetail = `${'A'.repeat(340)} UNIQUE_LONG_SUFFIX`
    render(<CrewLiveMonitor live={createLive([
      createEntry(1, {
        title: 'Lange Ausgabe',
        detail: `Output: ${longDetail}`,
      }),
    ])} />)

    const log = screen.getByLabelText('Crew-Live-Log')
    expect(within(log).queryByText(/UNIQUE_LONG_SUFFIX/)).not.toBeInTheDocument()

    fireEvent.click(screen.getAllByText('Mehr anzeigen')[0])

    expect(within(log).getByText(/UNIQUE_LONG_SUFFIX/)).toBeInTheDocument()
  })
})
