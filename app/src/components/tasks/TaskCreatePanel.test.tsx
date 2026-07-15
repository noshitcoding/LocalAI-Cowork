import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import TaskCreatePanel from './TaskCreatePanel'

describe('TaskCreatePanel', () => {
  it('keeps optional setup compact until the user asks for it', () => {
    render(
      <TaskCreatePanel
        crews={[]}
        defaultModel="llama3.1:8b"
        title=""
        prompt=""
        expectedOutput=""
        workDir=""
        runner="model"
        crewId=""
        model=""
        canCreateTask={false}
        onTitleChange={vi.fn()}
        onPromptChange={vi.fn()}
        onExpectedOutputChange={vi.fn()}
        onWorkDirChange={vi.fn()}
        onRunnerChange={vi.fn()}
        onCrewIdChange={vi.fn()}
        onModelChange={vi.fn()}
        onPickWorkDir={vi.fn()}
        onCreateTask={vi.fn()}
      />,
    )

    const advancedToggle = screen.getByRole('button', { name: /Advanced setup/i })
    expect(advancedToggle).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByLabelText('Expected output (optional)')).not.toBeInTheDocument()

    fireEvent.click(advancedToggle)

    expect(advancedToggle).toHaveAttribute('aria-expanded', 'true')
    expect(screen.getByLabelText('Expected output (optional)')).toBeInTheDocument()
    expect(screen.getByLabelText(/Working folder \(optional, absolute\)/)).toBeInTheDocument()
  })
})
