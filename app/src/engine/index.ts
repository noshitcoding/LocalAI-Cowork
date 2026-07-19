// ── LocalAI Cowork Engine (main entry) ─────────────────────────────────────────
// Architecture ported from Claude Code — adapted for Tauri desktop app
//
// This is the single entry point for the engine.
// Import from here in all UI components and stores.

// ── Types ──────────────────────────────────────────────────────────────────
export type {
  // Messages
  UUID,
  ContentBlock,
  ContentBlockText,
  ContentBlockImage,
  ContentBlockToolUse,
  ContentBlockToolResult,
  ContentBlockThinking,
  ContentBlockDelta,
  UserMessage,
  AssistantMessage,
  SystemMessage,
  AttachmentMessage,
  ToolUseSummaryMessage,
  TombstoneMessage,
  ProgressMessage,
  Message,
  StreamEvent,
  RequestStartEvent,
  TokenUsage,
  Terminal,
  ToolProgressData,

  // Tools
  Tool,
  Tools,
  ToolResult,
  ToolProgress,
  ToolCallProgress,
  ToolInputSchema,
  ToolCategory,
  ToolUseContext,
  ToolUIRequest,
  AskQuestionOption,
  ApprovalRequest,
  ApprovalResult,
  CanUseToolFn,
  PermissionMode,
  ToolPermissionRule,
  ToolPermissionContext,
  AppState,

  // Commands
  Command,
  CommandCategory,

  // Agents & MCP
  AgentDefinition,
  MCPConnection,
  MCPToolInfo,
  MCPResourceInfo,
  ThinkingConfig,
} from './types'

// ── Type Helpers ───────────────────────────────────────────────────────────
export {
  EMPTY_USAGE,
  accumulateUsage,
  generateUUID,
  createUserMessage,
  createSystemMessage,
  createAssistantMessage,
  extractTextContent,
  getToolUseBlocks,
  getThinkingContent,
  findToolByName,
  toolMatchesName,
  getEmptyToolPermissionContext,
  createInitialAppState,
} from './types'

// ── API Client ─────────────────────────────────────────────────────────────
export {
  streamMessages,
  sampleMessage,
  calculateCost,
  toAPIMessages,
  toAPIToolDefs,
  AnthropicAPIError,
  withRetry,
  ANTHROPIC_MODELS,
} from './api/anthropicClient'
export type {
  AnthropicConfig,
  StreamCallbacks,
  SampleResult,
} from './api/anthropicClient'

export {
  streamOllamaMessages,
  sampleOllamaMessage,
  toOllamaToolDefs,
  listOllamaModels,
  checkOllamaConnection,
  getOllamaModelInfo,
  detectModelCapabilities,
} from './api/ollamaClient'
export type {
  OllamaEngineConfig,
  OllamaModelCapabilities,
} from './api/ollamaClient'

// ── Query Engine (Core) ────────────────────────────────────────────────────
export {
  QueryEngine,
  createQueryEngine,
} from './core/queryEngine'
export type {
  EngineConfig,
  EngineEvent,
  EngineBackend,
} from './core/queryEngine'

// ── Tool Registry ──────────────────────────────────────────────────────────
export {
  registerTool,
  getAllTools,
  getToolsByCategory,
  getEnabledTools,
  getToolDefinitions,
  registerAllBuiltinTools,
} from './tools/registry'

// ── Command Registry ───────────────────────────────────────────────────────
export {
  registerCommand,
  getAllCommands,
  getCommandsByCategory,
  findCommand,
  parseCommand,
  executeCommand,
  registerBuiltinCommands,
} from './commands/registry'

// ── Memory System ──────────────────────────────────────────────────────────
export {
  loadProjectMemory,
  loadGlobalMemory,
  saveProjectMemory,
  storeMemoryEntry,
  getMemoryEntries,
  estimateConversationTokens,
  compactConversation,
  buildSystemPromptWithMemory,
  loadProjectSettings,
} from './memory/memorySystem'
export type {
  MemoryConfig,
  MemoryEntry,
} from './memory/memorySystem'

// ── Permission System ──────────────────────────────────────────────────────
export {
  buildPermissionContext,
  checkToolPermission,
  isPathAllowed,
  createAllowRule,
  createDenyRule,
  createAskRule,
  getDefaultSecurityRules,
  DEFAULT_PERMISSION_CONFIG,
} from './permissions/permissionEngine'
export type {
  PermissionConfig,
} from './permissions/permissionEngine'

// ── Multi-Agent Coordinator ────────────────────────────────────────────────
export {
  AgentCoordinator,
  DEFAULT_AGENTS,
} from './coordinator/agentCoordinator'
export type {
  AgentInstance,
} from './coordinator/agentCoordinator'

// ── Engine Configuration ───────────────────────────────────────────────────
export {
  DEFAULT_SYSTEM_PROMPT,
  DEFAULT_CONFIG,
  createEngineConfig,
  getAvailableModels,
  getAvailableOllamaModels,
  isValidApiKey,
  toPersistedConfig,
  fromPersistedConfig,
} from './config/engineConfig'
export type {
  FullEngineConfig,
  PersistedConfig,
} from './config/engineConfig'

// ── Services (ported from Claude Code) ─────────────────────────────────────
export {
  // Context Manager
  ContextManager,
  DEFAULT_CONTEXT_MANAGER_CONFIG,
} from './services/contextManager'
export type {
  ContextManagerConfig,
  ContextSnapshot,
} from './services/contextManager'

export {
  // Compact / Token Budget
  createTokenBudget,
  estimateTokens,
  shouldAutoCompact,
  getTokenWarningLevel,
  autoCompact,
  fallbackCompact,
  applyToolResultBudget,
  generateToolUseSummary,
  getMessagesAfterCompactBoundary,
} from './services/compact'
export type {
  TokenBudgetState,
} from './services/compact'

export {
  // Tool Orchestrator
  ToolOrchestrator,
} from './services/toolOrchestrator'
export type {
  ToolOrchestratorConfig,
} from './services/toolOrchestrator'

export {
  // Session Persistence
  createSession,
  endSession,
  loadSession,
  listSessions,
  deleteSession,
  generateSessionTitle,
  autoSaveSession,
} from './services/sessionPersistence'
export type {
  SessionRecord,
  SessionSummary,
} from './services/sessionPersistence'

