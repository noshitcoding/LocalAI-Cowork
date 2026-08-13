export type FollowUpPromptMessage = {
  role: 'user' | 'assistant' | 'system'
  content: string
}

type ClarificationContext = {
  originalTask: string
  assistantQuestion: string
}

const CLARIFICATION_CONTINUATION_INTRO = 'Continue the running task with the following question.'
const CLARIFICATION_USER_ANSWER_MARKER = '\n\nUser answer:\n'
const CLARIFICATION_CONTINUATION_INSTRUCTION = 'Continue the original task now. Use suitable tools directly and do not only answer with a list of available tools.'
const ASK_USER_ANSWER_PREFIX = 'Answer to question:\nQuestion: '
const ASK_USER_ANSWER_MARKER = '\nAnswer: '

const CLARIFYING_QUESTION_PATTERNS = [
  /please specify/i,
  /which criterion/i,
  /what criterion/i,
  /which .+ should/i,
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
    CLARIFICATION_CONTINUATION_INTRO,
    '',
    'Original task:',
    originalTask.trim(),
    '',
    'Assistant question:',
    assistantQuestion.trim(),
    '',
    'User answer:',
    answer.trim(),
    '',
    CLARIFICATION_CONTINUATION_INSTRUCTION,
  ].join('\n')
}

/**
 * Recovers the actual user entry from prompts that older app versions persisted
 * with their internal continuation context in the visible message content.
 */
export function resolveUserFacingPromptContent(content: string): string {
  const trimmed = content.trim()
  const continuationSuffix = `\n\n${CLARIFICATION_CONTINUATION_INSTRUCTION}`

  if (
    trimmed.startsWith(CLARIFICATION_CONTINUATION_INTRO)
    && trimmed.endsWith(continuationSuffix)
  ) {
    const answerMarkerIndex = trimmed.indexOf(CLARIFICATION_USER_ANSWER_MARKER)
    if (answerMarkerIndex >= 0) {
      const answerStart = answerMarkerIndex + CLARIFICATION_USER_ANSWER_MARKER.length
      const answer = trimmed.slice(answerStart, -continuationSuffix.length).trim()
      if (answer) return answer
    }
  }

  if (trimmed.startsWith(ASK_USER_ANSWER_PREFIX)) {
    const answerMarkerIndex = trimmed.lastIndexOf(ASK_USER_ANSWER_MARKER)
    if (answerMarkerIndex >= 0) {
      const answer = trimmed.slice(answerMarkerIndex + ASK_USER_ANSWER_MARKER.length).trim()
      if (answer) return answer
    }
  }

  return content
}
