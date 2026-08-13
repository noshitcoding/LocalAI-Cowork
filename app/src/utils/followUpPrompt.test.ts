import { describe, expect, it } from 'vitest'
import {
  buildClarificationContinuationPrompt,
  inferClarificationContext,
  isLikelyClarifyingQuestion,
  isLikelyShortFollowUpAnswer,
  resolveUserFacingPromptContent,
} from './followUpPrompt'

describe('followUpPrompt', () => {
  it('detects short follow-up answers', () => {
    expect(isLikelyShortFollowUpAnswer('alphabetisch')).toBe(true)
    expect(isLikelyShortFollowUpAnswer('ja')).toBe(true)
    expect(isLikelyShortFollowUpAnswer('')).toBe(false)
    expect(isLikelyShortFollowUpAnswer('a'.repeat(200))).toBe(false)
  })

  it('detects clarifying questions', () => {
    expect(isLikelyClarifyingQuestion('Please specify which criterion should be used to sort the folders.')).toBe(true)
    expect(isLikelyClarifyingQuestion('Nach welchem Kriterium soll ich sortieren?')).toBe(true)
    expect(isLikelyClarifyingQuestion('I moved the folders.')).toBe(false)
  })

  it('infers clarification context from previous chat messages', () => {
    const context = inferClarificationContext([
      { role: 'user', content: 'Sort all folders into 2 new folders.' },
      { role: 'assistant', content: 'Please specify which criterion should be used to sort the folders.' },
    ], 'alphabetisch')

    expect(context).toEqual({
      originalTask: 'Sort all folders into 2 new folders.',
      assistantQuestion: 'Please specify which criterion should be used to sort the folders.',
    })
  })

  it('builds a continuation prompt that keeps the original task', () => {
    const prompt = buildClarificationContinuationPrompt(
      'Sort all folders into 2 new folders.',
      'Please specify which criterion should be used to sort the folders.',
      'alphabetisch',
    )

    expect(prompt).toContain('Original task:')
    expect(prompt).toContain('Sort all folders into 2 new folders.')
    expect(prompt).toContain('User answer:')
    expect(prompt).toContain('alphabetisch')
    expect(prompt).toContain('do not only answer with a list of available tools')
  })

  it('shows only the user answer for persisted internal continuation prompts', () => {
    const prompt = buildClarificationContinuationPrompt(
      'Sort all folders into 2 new folders.',
      'Please specify which criterion should be used to sort the folders.',
      'alphabetisch',
    )

    expect(resolveUserFacingPromptContent(prompt)).toBe('alphabetisch')
  })

  it('shows only the user answer for persisted ask-user prompts', () => {
    const prompt = 'Answer to question:\nQuestion: Which folder should I use?\nAnswer: C:\\workspace'

    expect(resolveUserFacingPromptContent(prompt)).toBe('C:\\workspace')
  })

  it('leaves ordinary user messages unchanged', () => {
    const prompt = 'Can you test this by reading the file?'

    expect(resolveUserFacingPromptContent(prompt)).toBe(prompt)
  })
})
