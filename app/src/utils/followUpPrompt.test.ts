import { describe, expect, it } from 'vitest'
import {
  buildClarificationContinuationPrompt,
  inferClarificationContext,
  isLikelyClarifyingQuestion,
  isLikelyShortFollowUpAnswer,
} from './followUpPrompt'

describe('followUpPrompt', () => {
  it('detects short follow-up answers', () => {
    expect(isLikelyShortFollowUpAnswer('alphabetisch')).toBe(true)
    expect(isLikelyShortFollowUpAnswer('ja')).toBe(true)
    expect(isLikelyShortFollowUpAnswer('')).toBe(false)
    expect(isLikelyShortFollowUpAnswer('a'.repeat(200))).toBe(false)
  })

  it('detects clarifying questions', () => {
    expect(isLikelyClarifyingQuestion('Bitte geben Sie an, nach welchem Kriterium die Ordner sortiert werden sollen.')).toBe(true)
    expect(isLikelyClarifyingQuestion('Nach welchem Kriterium soll ich sortieren?')).toBe(true)
    expect(isLikelyClarifyingQuestion('Ich habe die Ordner verschoben.')).toBe(false)
  })

  it('infers clarification context from previous chat messages', () => {
    const context = inferClarificationContext([
      { role: 'user', content: 'Sortiere alle Ordner in 2 neue Ordner.' },
      { role: 'assistant', content: 'Bitte geben Sie an, nach welchem Kriterium die Ordner sortiert werden sollen.' },
    ], 'alphabetisch')

    expect(context).toEqual({
      originalTask: 'Sortiere alle Ordner in 2 neue Ordner.',
      assistantQuestion: 'Bitte geben Sie an, nach welchem Kriterium die Ordner sortiert werden sollen.',
    })
  })

  it('builds a continuation prompt that keeps the original task', () => {
    const prompt = buildClarificationContinuationPrompt(
      'Sortiere alle Ordner in 2 neue Ordner.',
      'Bitte geben Sie an, nach welchem Kriterium die Ordner sortiert werden sollen.',
      'alphabetisch',
    )

    expect(prompt).toContain('Urspruengliche Aufgabe:')
    expect(prompt).toContain('Sortiere alle Ordner in 2 neue Ordner.')
    expect(prompt).toContain('Antwort des Nutzers:')
    expect(prompt).toContain('alphabetisch')
    expect(prompt).toContain('antworte nicht nur mit einer Liste verfuegbarer Tools')
  })
})
