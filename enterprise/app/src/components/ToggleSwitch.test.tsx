import { fireEvent, render, screen } from '@testing-library/react'
import ToggleSwitch from './ToggleSwitch'

describe('ToggleSwitch', () => {
  it('shows an explicit off state and requests activation', () => {
    const onChange = vi.fn()
    render(<ToggleSwitch checked={false} onChange={onChange} ariaLabel="Launch at system startup" />)

    const toggle = screen.getByRole('switch', { name: 'Launch at system startup' })
    expect(toggle).toHaveAttribute('aria-checked', 'false')
    expect(toggle).toHaveTextContent('OFF')
    fireEvent.click(toggle)
    expect(onChange).toHaveBeenCalledWith(true)
  })

  it('shows an explicit on state and requests deactivation', () => {
    const onChange = vi.fn()
    render(<ToggleSwitch checked onChange={onChange} ariaLabel="Launch at system startup" />)

    const toggle = screen.getByRole('switch', { name: 'Launch at system startup' })
    expect(toggle).toHaveAttribute('aria-checked', 'true')
    expect(toggle).toHaveTextContent('ON')
    fireEvent.click(toggle)
    expect(onChange).toHaveBeenCalledWith(false)
  })
})
