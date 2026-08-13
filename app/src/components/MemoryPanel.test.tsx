import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import MemoryPanel from './MemoryPanel'
import { useMemoryStore } from '../stores/memoryStore'
import { safeInvoke } from '../utils/safeInvoke'

vi.mock('../utils/safeInvoke', () => ({
  hasTauriRuntime: () => true,
  safeInvoke: vi.fn(),
}))

const safeInvokeMock = vi.mocked(safeInvoke)

describe('MemoryPanel knowledge import', () => {
  beforeEach(() => {
    safeInvokeMock.mockReset()
    safeInvokeMock.mockImplementation(async (command, _args, fallback) => {
      if (command === 'memory_search') return []
      return fallback
    })
    useMemoryStore.setState({
      entries: [],
      searchResults: [],
      profileEntries: [],
      providers: [],
      hints: [],
      lastSnapshot: null,
      loading: false,
      error: null,
    })
  })

  it('chunks and persists pasted knowledge through the backend contract', async () => {
    render(<MemoryPanel />)

    fireEvent.change(screen.getByRole('textbox', { name: 'Source title' }), {
      target: { value: 'API Handbook' },
    })
    fireEvent.change(screen.getByRole('textbox', { name: 'Content' }), {
      target: { value: 'The scheduler API requires idempotent retries and a rollback contract.' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Import knowledge' }))

    await waitFor(() => {
      expect(safeInvokeMock).toHaveBeenCalledWith(
        'memory_upsert',
        expect.objectContaining({
          scope: 'shared',
          category: 'knowledge',
          key: 'API Handbook',
        }),
        undefined,
      )
    })
    expect(await screen.findByText('1 knowledge chunk imported')).toBeInTheDocument()
  })
})
