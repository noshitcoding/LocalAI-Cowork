import { fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { describe, expect, it, vi } from 'vitest'
import FeaturesView from './FeaturesView'

vi.mock('./McpView', () => ({ default: () => <div>MCP workbench content</div> }))
vi.mock('./MemoryPanel', () => ({ default: () => <div>Knowledge workbench content</div> }))
vi.mock('./SkillPanel', () => ({ default: () => <div>Skills workbench content</div> }))

describe('FeaturesView', () => {
  it('opens the requested operational tab instead of a feature-status page', () => {
    render(
      <MemoryRouter initialEntries={['/features?tab=knowledge']}>
        <FeaturesView />
      </MemoryRouter>,
    )

    expect(screen.getByText('Knowledge workbench content')).toBeInTheDocument()
    expect(screen.getByRole('tab', { name: 'Knowledge base' })).toHaveAttribute('aria-selected', 'true')
  })

  it('switches directly between MCP, knowledge, and skills tools', () => {
    render(
      <MemoryRouter initialEntries={['/features?tab=mcp']}>
        <FeaturesView />
      </MemoryRouter>,
    )

    expect(screen.getByText('MCP workbench content')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('tab', { name: 'Skills' }))
    expect(screen.getByText('Skills workbench content')).toBeInTheDocument()
  })

  it('supports arrow-key navigation across capability tabs', () => {
    render(
      <MemoryRouter initialEntries={['/features?tab=mcp']}>
        <FeaturesView />
      </MemoryRouter>,
    )

    fireEvent.keyDown(screen.getByRole('tab', { name: 'MCP Server' }), { key: 'ArrowRight' })

    expect(screen.getByRole('tab', { name: 'Knowledge base' })).toHaveAttribute('aria-selected', 'true')
    expect(screen.getByText('Knowledge workbench content')).toBeInTheDocument()
  })
})
