export type FollowUpPromptMessage = {
  role: 'user' | 'assistant' | 'system'
  content: string
}

type ClarificationContext = {
  originalTask: string
  assistantQuestion: string
}

const CLARIFYING_QUESTION_PATTERNS = [
  /bitte geben sie an/i,
  /nach welchem kriterium/i,
  /welches kriterium/i,
  /welcher kriterium/i,
  /wie soll(?:en)?/i,
  /welche(?:n|r|s)?\s+/i,
]

export function isLikelyShortFollowUpAnswer(input: string): boolean {
  const trimmed = input.trim()
  if (!trimmed) return false
  if (trimmed.length > 160) return false
  if (trimmed.split(/\r?\n/).length > 3) return false
  return true
}

export function isLikelyClarifyingQuestion(input: string): boolean {
  const trimmed = input.trim()
  if (!trimmed) return false
  if (trimmed.endsWith('?')) return true
  return CLARIFYING_QUESTION_PATTERNS.some((pattern) => pattern.test(trimmed))
}

export function inferClarificationContext(
  messages: FollowUpPromptMessage[],
  candidateReply: string,
): ClarificationContext | null {
  if (!isLikelyShortFollowUpAnswer(candidateReply)) {
    return null
  }

  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const assistantMessage = messages[index]
    if (assistantMessage.role !== 'assistant') continue

    const assistantQuestion = assistantMessage.content.trim()
    if (!isLikelyClarifyingQuestion(assistantQuestion)) {
      return null
    }

    for (let previousIndex = index - 1; previousIndex >= 0; previousIndex -= 1) {
      const previousUserMessage = messages[previousIndex]
      if (previousUserMessage.role !== 'user') continue

      const originalTask = previousUserMessage.content.trim()
      if (!originalTask) return null

      return {
        originalTask,
        assistantQuestion,
      }
    }

    return null
  }

  return null
}

export function buildClarificationContinuationPrompt(
  originalTask: string,
  assistantQuestion: string,
  answer: string,
): string {
  return [
    'Setze die laufende Aufgabe mit der folgenden Rueckfrage fort.',
    '',
    'Urspruengliche Aufgabe:',
    originalTask.trim(),
    '',
    'Rueckfrage des Assistenten:',
    assistantQuestion.trim(),
    '',
    'Antwort des Nutzers:',
    answer.trim(),
    '',
    'Fuehre die urspruengliche Aufgabe jetzt weiter aus. Nutze passende Tools direkt und antworte nicht nur mit einer Liste verfuegbarer Tools.',
  ].join('\n')
}
