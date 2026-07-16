import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import SecureCredentialInput from './SecureCredentialInput'

describe('SecureCredentialInput', () => {
  it('keeps edits local until blur commits them', async () => {
    const onCommit = vi.fn().mockResolvedValue(undefined)
    render(<SecureCredentialInput value="old-secret" onCommit={onCommit} ariaLabel="Credential" />)
    const input = screen.getByLabelText('Credential')

    fireEvent.change(input, { target: { value: 'new-secret' } })
    expect(onCommit).not.toHaveBeenCalled()
    fireEvent.blur(input)

    await waitFor(() => expect(onCommit).toHaveBeenCalledWith('new-secret'))
  })

  it('surfaces native storage failures without discarding the draft', async () => {
    const onCommit = vi.fn().mockRejectedValue(new Error('storage unavailable'))
    render(<SecureCredentialInput value="" onCommit={onCommit} ariaLabel="Credential" />)
    const input = screen.getByLabelText('Credential')

    fireEvent.change(input, { target: { value: 'unsaved-secret' } })
    fireEvent.blur(input)

    expect(await screen.findByRole('alert')).toHaveTextContent('Secure value could not be saved.')
    expect(input).toHaveAttribute('aria-invalid', 'true')
    expect(input).toHaveValue('unsaved-secret')
  })

  it('discards an uncommitted draft with Escape', () => {
    const onCommit = vi.fn().mockResolvedValue(undefined)
    render(<SecureCredentialInput value="stored-secret" onCommit={onCommit} ariaLabel="Credential" />)
    const input = screen.getByLabelText('Credential')

    fireEvent.change(input, { target: { value: 'draft-secret' } })
    fireEvent.keyDown(input, { key: 'Escape' })

    expect(input).toHaveValue('stored-secret')
    expect(onCommit).not.toHaveBeenCalled()
  })
})
