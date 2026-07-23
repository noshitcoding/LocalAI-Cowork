import type { Message } from '../types'
import { estimateConversationTokens } from './compact'
import {
  type TokenBudgetState,
  createTokenBudget,
  getTokenWarningLevel,
  applyToolResultBudget,
} from './compact'

export type ContextManagerConfig = {
  maxContextTokens: number
  toolResultBudgetEnabled: boolean
  maxPromptTooLongRetries: number
}

export const DEFAULT_CONTEXT_MANAGER_CONFIG: ContextManagerConfig = {
  maxContextTokens: 120000,
  toolResultBudgetEnabled: true,
  maxPromptTooLongRetries: 3,
}

export type ContextSnapshot = {
  totalTokens: number
  warningLevel: 'none' | 'warning' | 'critical'
  messageCount: number
  activeMessageCount: number
}

export type ContextTrimResult = {
  messages: Message[]
  droppedCount: number
  estimatedTokens: number
  maxInputTokens: number
  fits: boolean
}

function isHumanUserMessage(message: Message | undefined): boolean {
  return message?.type === 'user'
    && !message.content.some((block) => block.type === 'tool_result')
}

function findProtectedUserIndex(messages: Message[]): number {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    if (isHumanUserMessage(messages[index])) return index
  }
  return Math.max(0, messages.length - 1)
}

function oldestRemovableTurnEnd(messages: Message[], protectedUserIndex: number): number {
  if (protectedUserIndex <= 0) return 0

  const firstUser = messages.findIndex((message, index) => (
    index < protectedUserIndex && isHumanUserMessage(message)
  ))
  if (firstUser < 0) return protectedUserIndex
  if (firstUser > 0) return firstUser

  for (let index = firstUser + 1; index < protectedUserIndex; index += 1) {
    if (isHumanUserMessage(messages[index])) return index
  }
  return protectedUserIndex
}

export class ContextManager {
  private config: ContextManagerConfig
  private budget: TokenBudgetState

  constructor(
    config: Partial<ContextManagerConfig> = {},
    maxContextTokens?: number,
  ) {
    this.config = { ...DEFAULT_CONTEXT_MANAGER_CONFIG, ...config }
    if (maxContextTokens) {
      this.config.maxContextTokens = maxContextTokens
    }
    this.budget = createTokenBudget(this.config.maxContextTokens)
  }

  getSnapshot(messages: Message[]): ContextSnapshot {
    const totalTokens = estimateConversationTokens(messages)
    return {
      totalTokens,
      warningLevel: getTokenWarningLevel(messages, this.budget),
      messageCount: messages.length,
      activeMessageCount: messages.length,
    }
  }

  trimToBudget(
    messages: Message[],
    fixedOverheadTokens: number,
    inputRatio = 0.8,
  ): ContextTrimResult {
    const maxInputTokens = Math.max(1, Math.floor(this.config.maxContextTokens * inputRatio))
    const originalLength = messages.length
    let kept = [...messages]
    let protectedUserIndex = findProtectedUserIndex(kept)

    const estimate = () => fixedOverheadTokens + estimateConversationTokens(kept)
    while (kept.length > 0 && estimate() > maxInputTokens) {
      const removeEnd = oldestRemovableTurnEnd(kept, protectedUserIndex)
      if (removeEnd <= 0) break
      kept = kept.slice(removeEnd)
      protectedUserIndex -= removeEnd
    }

    const estimatedTokens = estimate()
    return {
      messages: kept,
      droppedCount: originalLength - kept.length,
      estimatedTokens,
      maxInputTokens,
      fits: estimatedTokens <= maxInputTokens,
    }
  }

  applyBudget(messages: Message[]): Message[] {
    if (!this.config.toolResultBudgetEnabled) return messages
    return applyToolResultBudget(messages)
  }

  resetPromptTooLongCount(): void {
    this.budget = { ...this.budget, promptTooLongCount: 0 }
  }

  updateMaxTokens(maxTokens: number): void {
    this.config.maxContextTokens = maxTokens
    this.budget = createTokenBudget(maxTokens)
  }
}
