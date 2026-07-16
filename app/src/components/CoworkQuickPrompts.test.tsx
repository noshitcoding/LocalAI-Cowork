import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../i18n'
import CoworkQuickPrompts from './CoworkQuickPrompts'

describe('CoworkQuickPrompts', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en')
  })

  it('presents starter tasks as a labeled, actionable group', () => {
    const onSelect = vi.fn()
    const prompts = ['Plan the work.', 'Review the risks.', 'Prioritize the next steps.']
    render(<CoworkQuickPrompts prompts={prompts} onSelect={onSelect} />)

    expect(screen.getByRole('group', { name: 'Use starter task' })).toBeInTheDocument()
    expect(screen.getAllByRole('button')).toHaveLength(3)

    fireEvent.click(screen.getByRole('button', { name: 'Review the risks.' }))
    expect(onSelect).toHaveBeenCalledWith('Review the risks.')
  })
})
