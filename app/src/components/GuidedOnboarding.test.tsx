import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import GuidedOnboarding, { GUIDED_ONBOARDING_STORAGE_KEY, STARTER_PROMPT } from './GuidedOnboarding'

const baseProps = {
  providerLabel: 'Ollama',
  model: 'llama3.1:8b',
  providerConfigured: true,
  workingFolder: 'C:\\workspace',
  permissionLabel: 'Standard',
  onChooseFolder: vi.fn(),
  onOpenSettings: vi.fn(),
  onUseStarterTask: vi.fn(),
}

describe('GuidedOnboarding', () => {
  beforeEach(() => {
    window.localStorage.removeItem(GUIDED_ONBOARDING_STORAGE_KEY)
    Object.values(baseProps).forEach((value) => {
      if (typeof value === 'function') value.mockClear()
    })
  })

  it('guides the user to a real starter task and persists completion', () => {
    render(<GuidedOnboarding {...baseProps} />)

    expect(screen.getByRole('heading', { name: 'Set up LocalAI Cowork' })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }))
    expect(screen.getByText('Choose how the work is powered')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }))
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }))
    fireEvent.click(screen.getByRole('button', { name: 'Use starter task' }))

    expect(baseProps.onUseStarterTask).toHaveBeenCalledWith(STARTER_PROMPT)
    expect(window.localStorage.getItem(GUIDED_ONBOARDING_STORAGE_KEY)).toBe('completed')
    expect(screen.getByRole('button', { name: 'Open getting started' })).toBeInTheDocument()
  })

  it('can be dismissed and reopened without losing discoverability', () => {
    render(<GuidedOnboarding {...baseProps} model="" providerConfigured={false} workingFolder={null} />)

    expect(screen.getAllByText('Needs setup').length).toBeGreaterThan(0)
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss onboarding' }))
    expect(window.localStorage.getItem(GUIDED_ONBOARDING_STORAGE_KEY)).toBe('dismissed')

    fireEvent.click(screen.getByRole('button', { name: 'Open getting started' }))
    expect(screen.getByRole('heading', { name: 'Set up LocalAI Cowork' })).toBeInTheDocument()
  })

  it('links model and folder steps to the existing application actions', () => {
    render(<GuidedOnboarding {...baseProps} />)

    fireEvent.click(screen.getByRole('button', { name: 'Model' }))
    fireEvent.click(screen.getByRole('button', { name: 'Open model settings' }))
    expect(baseProps.onOpenSettings).toHaveBeenCalledTimes(1)

    fireEvent.click(screen.getByRole('button', { name: 'Context' }))
    fireEvent.click(screen.getByRole('button', { name: 'Choose another folder' }))
    expect(baseProps.onChooseFolder).toHaveBeenCalledTimes(1)
  })

  it('keeps an unconfigured cloud model out of the ready state', () => {
    window.localStorage.setItem(GUIDED_ONBOARDING_STORAGE_KEY, 'dismissed')
    render(
      <GuidedOnboarding
        {...baseProps}
        providerLabel="OpenRouter"
        model="nvidia/nemotron-3-super-120b-a12b:free"
        providerConfigured={false}
        workingFolder={null}
      />,
    )

    expect(screen.getByText('Needs setup')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Open model settings' }))
    expect(baseProps.onOpenSettings).toHaveBeenCalledTimes(1)
  })

  it('routes incomplete setup back to the model step before using the starter task', () => {
    render(<GuidedOnboarding {...baseProps} providerConfigured={false} workingFolder={null} />)

    fireEvent.click(screen.getByRole('button', { name: 'Control' }))
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }))
    expect(screen.getByText('Choose how the work is powered')).toBeInTheDocument()
    expect(baseProps.onUseStarterTask).not.toHaveBeenCalled()
  })
})
