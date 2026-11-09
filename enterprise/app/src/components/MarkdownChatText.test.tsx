import { render, screen } from '@testing-library/react'
import { MarkdownChatText } from './MarkdownChatText'

vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: vi.fn().mockResolvedValue(undefined),
}))

describe('MarkdownChatText', () => {
  it('renders common assistant Markdown instead of showing its control characters', () => {
    const { container } = render(
      <MarkdownChatText content={'# Result\n\n**Bold** and `code`\n\n- one\n- two\n\n| A | B |\n| - | - |\n| 1 | 2 |'} />,
    )

    expect(screen.getByRole('heading', { name: 'Result' })).toBeInTheDocument()
    expect(screen.getByText('Bold').tagName).toBe('STRONG')
    expect(screen.getByText('code').tagName).toBe('CODE')
    expect(screen.getAllByRole('listitem')).toHaveLength(2)
    expect(container.querySelector('table')).toBeInTheDocument()
    expect(container).not.toHaveTextContent('**Bold**')
  })

  it('does not turn model-provided HTML into live DOM elements', () => {
    const { container } = render(
      <MarkdownChatText content={'Safe text\n\n<script>alert("unsafe")</script>\n\n[unsafe](javascript:alert(1))'} />,
    )

    expect(container).toHaveTextContent(/^Safe text/)
    expect(container.querySelector('script')).not.toBeInTheDocument()
    expect(screen.getByText('unsafe').closest('a')).toBeNull()
  })

  it('keeps extracted web-search sources as source buttons', () => {
    render(
      <MarkdownChatText content={'**Answer**\n\nSources:\n1. Example\nhttps://example.com'} />,
    )

    expect(screen.getByText('Answer').tagName).toBe('STRONG')
    expect(screen.getByRole('button', { name: /Example/ })).toBeInTheDocument()
  })
})
