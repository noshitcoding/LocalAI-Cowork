import { render, screen, fireEvent, waitFor, act } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import SessionSearchPanel from './SessionSearchPanel'
import { useEngineStore } from '../stores/engineStore'
import { useChatStore } from '../stores/chatStore'
import type { SessionRecord } from '../engine'

const sessionSummary = {
  id: 'session-1',
  title: 'Build Review',
  cwd: 'C:/workspace',
  messageCount: 2,
  createdAt: 1000,
  updatedAt: 2000,
}

const sessionRecord: SessionRecord = {
  id: 'session-1',
  title: 'Build Review',
  cwd: 'C:/workspace',
  createdAt: 1000,
  updatedAt: 2000,
  totalUsage: { input_tokens: 12, output_tokens: 34 },
  totalCostUsd: 0,
  appState: {},
  messages: [
    {
      type: 'user',
      uuid: 'user-1',
      timestamp: 1001,
      content: [{
        type: 'text',
        text: [
          'Bitte pruefe den Build.',
          '',
          'Verbundene Pfade (2):',
          '1. Datei: C:/workspace/build.log',
          '2. Ordner: C:/workspace/src',
        ].join('\n'),
      }],
    } as never,
    {
      type: 'assistant',
      uuid: 'assistant-1',
      timestamp: 1002,
      content: [{ type: 'text', text: 'Build ist gruen.' }],
      model: 'llama3.1:8b',
      usage: { input_tokens: 12, output_tokens: 34 },
    } as never,
  ],
}

describe('SessionSearchPanel', () => {
  beforeEach(() => {
    useChatStore.setState({
      threads: [],
      activeThreadId: null,
      pendingApproval: [],
      busy: false,
      error: null,
    })

    useEngineStore.setState({
      currentSessionId: null,
      getSessions: vi.fn().mockResolvedValue([sessionSummary]),
      loadSessionById: vi.fn().mockResolvedValue(sessionRecord),
      deleteSessionById: vi.fn().mockResolvedValue(undefined),
    })
  })

  it('loads and displays saved sessions', async () => {
    await act(async () => { render(<SessionSearchPanel />) })
    await waitFor(() => expect(screen.getByText('Build Review')).toBeInTheDocument())
    expect(screen.getByText('C:/workspace')).toBeInTheDocument()
  })

  it('hydrates the chat thread when loading a session', async () => {
    await act(async () => { render(<SessionSearchPanel />) })
    await waitFor(() => expect(screen.getByText('Build Review')).toBeInTheDocument())

    await act(async () => {
      fireEvent.click(screen.getByText('Laden'))
    })

    await waitFor(() => {
      const state = useChatStore.getState()
      expect(state.activeThreadId).toBe('session-1')
      expect(state.threads[0]?.messages[0]?.content).toBe('Bitte pruefe den Build.')
      expect(state.threads[0]?.messages[0]?.attachments).toEqual([
        { path: 'C:/workspace/build.log', kind: 'file' },
        { path: 'C:/workspace/src', kind: 'folder' },
      ])
    })
  })

  it('removes a session from the list after deletion', async () => {
    await act(async () => { render(<SessionSearchPanel />) })
    await waitFor(() => expect(screen.getByText('Build Review')).toBeInTheDocument())

    await act(async () => {
      fireEvent.click(screen.getByText('Loeschen'))
    })

    await waitFor(() => expect(screen.queryByText('Build Review')).not.toBeInTheDocument())
  })

  it('ignores malformed session entries and stays stable', async () => {
    useEngineStore.setState({
      getSessions: vi.fn().mockResolvedValue([
        null,
        { id: 'session-2' },
        { id: 'session-3', title: 'Valid', cwd: '/tmp', messageCount: 1, createdAt: 1000, updatedAt: 2000 },
      ]),
    })

    await act(async () => { render(<SessionSearchPanel />) })

    await waitFor(() => expect(screen.getByText('Valid')).toBeInTheDocument())
    expect(screen.getByText('Unbenannte Session')).toBeInTheDocument()
  })
})
