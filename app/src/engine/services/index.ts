// ── Services barrel exports ────────────────────────────────────────────────
// New service layer ported from Claude Code

export {
  // Compact
  autoCompact,
  fallbackCompact,
  applyToolResultBudget,
  generateToolUseSummary,
  getMessagesAfterCompactBoundary,
  estimateTokens,
  estimateConversationTokens as estimateConversationTokensCompact,
  shouldAutoCompact,
  getTokenWarningLevel,
  createTokenBudget,
} from './compact'
export type { TokenBudgetState } from './compact'

export {
  // Context Manager
  ContextManager,
  DEFAULT_CONTEXT_MANAGER_CONFIG,
} from './contextManager'
export type { ContextManagerConfig, ContextSnapshot } from './contextManager'

export {
  // Tool Orchestrator
  ToolOrchestrator,
  DEFAULT_ORCHESTRATOR_CONFIG,
} from './toolOrchestrator'
export type {
  ToolExecutionResult,
  ToolExecutionEvent,
  ToolOrchestratorConfig,
} from './toolOrchestrator'

export {
  // Session Persistence
  createSession,
  endSession,
  loadSession,
  listSessions,
  deleteSession,
  generateSessionTitle,
  autoSaveSession,
} from './sessionPersistence'
export type { SessionRecord, SessionSummary } from './sessionPersistence'

