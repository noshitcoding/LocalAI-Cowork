import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { describe, expect, it, vi } from 'vitest'
import FeaturesView from './FeaturesView'

vi.mock('./McpView', () => ({ default: () => <div>MCP workbench content</div> }))
vi.mock('./MemoryPanel', () => ({ default: () => <div>Knowledge workbench content</div> }))
vi.mock('./SkillPanel', () => ({ default: () => <div>Skills workbench content</div> }))

describe('FeaturesView', () => {
  it('opens the requested workbench without an internal tab bar', () => {
    render(
      <MemoryRouter initialEntries={['/features?tab=knowledge']}>
        <FeaturesView />
      </MemoryRouter>,
    )

    expect(screen.getByText('Knowledge workbench content')).toBeInTheDocument()
    expect(screen.getByRole('region', { name: 'Knowledge base' })).toBeInTheDocument()
    expect(screen.queryByRole('tablist')).not.toBeInTheDocument()
  })

  it.each([
    ['mcp', 'MCP Server', 'MCP workbench content'],
    ['knowledge', 'Knowledge base', 'Knowledge workbench content'],
    ['skills', 'Skills', 'Skills workbench content'],
  ])('supports direct URLs for the %s workbench', (tab, label, content) => {
    render(
      <MemoryRouter initialEntries={[`/features?tab=${tab}`]}>
        <FeaturesView />
      </MemoryRouter>,
    )

    expect(screen.getByRole('region', { name: label })).toBeInTheDocument()
    expect(screen.getByText(content)).toBeInTheDocument()
  })
})
