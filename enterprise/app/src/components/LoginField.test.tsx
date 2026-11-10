import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import LoginField from './LoginField'

describe('LoginField explanations', () => {
  it('toggles help without submitting and preserves an existing accessible description', () => {
    render(<>
      <p id="password-rule">At least 12 characters.</p>
      <LoginField label="Password" help="Use your server account password." helpButtonLabel="Help for Password">
        <input type="password" aria-describedby="password-rule" />
      </LoginField>
    </>)
    const input = screen.getByLabelText('Password')
    const info = screen.getByRole('button', { name: 'Help for Password' })
    expect(info).toHaveAttribute('type', 'button')
    expect(info).toHaveAttribute('aria-expanded', 'false')
    expect(input).toHaveAccessibleDescription('At least 12 characters.')
    fireEvent.click(info)
    expect(info).toHaveAttribute('aria-expanded', 'true')
    expect(input).toHaveAttribute('aria-describedby', `password-rule ${info.getAttribute('aria-controls')}`)
    expect(input).toHaveAccessibleDescription('At least 12 characters. Use your server account password.')
    fireEvent.click(info)
    expect(input).toHaveAccessibleDescription('At least 12 characters.')
    fireEvent.click(info)
    fireEvent.keyDown(info, { key: 'Escape' })
    expect(info).toHaveAttribute('aria-expanded', 'false')
    expect(input).toHaveAttribute('aria-describedby', 'password-rule')
    expect(screen.queryByText('Use your server account password.')).not.toBeInTheDocument()
  })
})
