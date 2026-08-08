import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import ChatDropdown from './ChatDropdown'

const OPTIONS = [
  { value: 'alpha', label: 'Alpha' },
  { value: 'beta', label: 'Beta' },
  { value: 'gamma', label: 'Gamma' },
]

describe('ChatDropdown', () => {
  it('opens as a listbox and selects an option with the keyboard', async () => {
    const onChange = vi.fn()
    render(
      <ChatDropdown
        value="alpha"
        options={OPTIONS}
        onChange={onChange}
        ariaLabel="Model"
        className="chat-compact-select"
      />,
    )

    const toggle = screen.getByRole('combobox', { name: 'Model' })
    toggle.focus()
    fireEvent.keyDown(toggle, { key: 'ArrowDown' })

    const alpha = await screen.findByRole('option', { name: 'Alpha' })
    await waitFor(() => expect(alpha).toHaveFocus())
    fireEvent.keyDown(alpha, { key: 'ArrowDown' })

    const beta = screen.getByRole('option', { name: 'Beta' })
    await waitFor(() => expect(beta).toHaveFocus())
    fireEvent.keyDown(beta, { key: 'Enter' })

    expect(onChange).toHaveBeenCalledWith('beta')
    expect(screen.queryByRole('listbox', { name: 'Model' })).not.toBeInTheDocument()
    await waitFor(() => expect(toggle).toHaveFocus())
  })

  it('closes when the user clicks outside', () => {
    render(
      <ChatDropdown
        value="alpha"
        options={OPTIONS}
        onChange={vi.fn()}
        ariaLabel="Provider"
      />,
    )

    fireEvent.click(screen.getByRole('combobox', { name: 'Provider' }))
    expect(screen.getByRole('listbox', { name: 'Provider' })).toBeInTheDocument()

    fireEvent.pointerDown(document.body)
    expect(screen.queryByRole('listbox', { name: 'Provider' })).not.toBeInTheDocument()
  })
})
