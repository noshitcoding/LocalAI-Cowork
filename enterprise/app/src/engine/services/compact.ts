import type { Message } from '../types'
import { extractTextContent } from '../types'

export type TokenBudgetState = {
  estimatedContextTokens: number
  inputThreshold: number
  warningThreshold: number
  promptTooLongCount: number
}

export function createTokenBudget(maxTokens: number): TokenBudgetState {
  return {
    estimatedContextTokens: 0,
    inputThreshold: Math.floor(maxTokens * 0.8),
    warningThreshold: 0.7,
    promptTooLongCount: 0,
  }
}

export function estimateTokens(text: string): number {
  return Math.ceil(text.length / 4)
}

export function estimateConversationTokens(messages: Message[]): number {
  return messages.reduce((sum, message) => sum + estimateTokens(extractTextContent(message)), 0)
}

export function getTokenWarningLevel(
  messages: Message[],
  budget: TokenBudgetState,
): 'none' | 'warning' | 'critical' {
  const ratio = estimateConversationTokens(messages) / budget.inputThreshold
  if (ratio >= 1) return 'critical'
  if (ratio >= budget.warningThreshold) return 'warning'
  return 'none'
}

const MAX_TOOL_RESULT_CHARS = 30000
const TRUNCATION_NOTICE = '\n\n[... result truncated because it exceeds the tool-result budget ...]'

export function applyToolResultBudget(messages: Message[]): Message[] {
  return messages.map((message) => {
    if (message.type !== 'user') return message
    if (!message.content.some((block) => block.type === 'tool_result')) return message

    return {
      ...message,
      content: message.content.map((block) => {
        if (block.type !== 'tool_result' || block.content.length <= MAX_TOOL_RESULT_CHARS) {
          return block
        }
        return {
          ...block,
          content: block.content.slice(0, MAX_TOOL_RESULT_CHARS) + TRUNCATION_NOTICE,
        }
      }),
    }
  })
}

export function generateToolUseSummary(
  toolName: string,
  input: Record<string, unknown>,
  result: string,
): string {
  const inputSnippet = JSON.stringify(input).slice(0, 100)
  const resultSnippet = result.slice(0, 200)
  return `${toolName}(${inputSnippet}) -> ${resultSnippet}${result.length > 200 ? '...' : ''}`
}
