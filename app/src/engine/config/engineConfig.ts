// ── Engine Configuration (ported from Claude Code) ──────────────────────────
// Mirrors: claude-code-main/src/config/ + context.ts
// Centralizes all configuration for the engine
//
// Enhanced: Ollama-first defaults, context manager config, session persistence

import type { AnthropicConfig } from '../api/anthropicClient'
import { ANTHROPIC_MODELS } from '../api/anthropicClient'
import { listOllamaModels, type OllamaEngineConfig } from '../api/ollamaClient'
import type { PermissionConfig } from '../permissions/permissionEngine'
import { DEFAULT_PERMISSION_CONFIG } from '../permissions/permissionEngine'
import type { AgentDefinition, MCPConnection, ThinkingConfig } from '../types'
import type { ContextManagerConfig } from '../services/contextManager'
import { DEFAULT_CONTEXT_MANAGER_CONFIG } from '../services/contextManager'

// ── Full Engine Configuration ──────────────────────────────────────────────

export type FullEngineConfig = {
  /** Anthropic API configuration (fallback, Ollama preferred) */
  anthropic: AnthropicConfig
  /** Ollama configuration (primary backend) */
  ollama: OllamaEngineConfig
  /** Working directory */
  cwd: string
  /** Base system prompt */
  systemPrompt: string
  /** Maximum agentic turns before stopping */
  maxTurns: number
  /** Maximum budget in USD (0 = unlimited) */
  maxBudgetUsd: number
  /** Debug mode */
  debug: boolean
  /** Permission configuration */
  permissions: PermissionConfig
  /** Agent definitions for sub-agents */
  agentDefinitions: AgentDefinition[]
  /** MCP server connections */
  mcpConnections: MCPConnection[]
  /** Thinking/reasoning configuration */
  thinking: ThinkingConfig
  /** Auto-compaction threshold (tokens) */
  compactionThreshold: number
  /** Context manager configuration */
  contextManager: ContextManagerConfig
  /** Locale for responses */
  locale: string
  /** Custom system prompt to append */
  appendSystemPrompt: string
  /** Global memory directory */
  globalMemoryDir: string
  /** Enable session persistence */
  sessionPersistence: boolean
  /** Enable parent directory walking for CLAUDE.md */
  walkParentDirs: boolean
}

// ── Defaults ───────────────────────────────────────────────────────────────

export const DEFAULT_SYSTEM_PROMPT = `You are a helpful AI assistant in the LocalAI Cowork desktop application.
You can use tools to read, write, search, execute shell commands, work with files, and more.

Guidelines:
1. Answer in English unless the user explicitly requests another language.
2. Read files before changing them so you understand the context.
3. Be concise, direct, and practical.
4. Use tools when they help complete the task instead of only describing steps.
5. If the goal is clear, execute it independently and ask only when critical information is missing or a destructive action needs confirmation.
6. Do not create files that are not needed.
7. Preserve user work and avoid overwriting unrelated changes.
8. For file organization and structure work, use dedicated file tools such as ListDir, CreateDirectory, MovePath, and CopyPath instead of only describing shell commands.`

export const DEFAULT_CONFIG: FullEngineConfig = {
  anthropic: {
    apiKey: '',
    model: 'claude-sonnet-4-20250514',
    maxTokens: 16384,
  },
  ollama: {
    baseUrl: 'http://localhost:11434',
    model: 'llama3.1:8b',
    contextWindow: 128000,
    temperature: 0.1,
    thinkingEnabled: false,
    keepAlive: '30m',
  },
  cwd: '',
  systemPrompt: DEFAULT_SYSTEM_PROMPT,
  maxTurns: 25,
  maxBudgetUsd: 0,
  debug: false,
  permissions: DEFAULT_PERMISSION_CONFIG,
  agentDefinitions: [],
  mcpConnections: [],
  thinking: { type: 'disabled' },
  compactionThreshold: 100000,
  contextManager: DEFAULT_CONTEXT_MANAGER_CONFIG,
  locale: 'de',
  appendSystemPrompt: '',
  globalMemoryDir: '',
  sessionPersistence: true,
  walkParentDirs: true,
}

// ── Config Helpers ─────────────────────────────────────────────────────────

export function createEngineConfig(overrides: Partial<FullEngineConfig>): FullEngineConfig {
  return {
    ...DEFAULT_CONFIG,
    ...overrides,
    anthropic: {
      ...DEFAULT_CONFIG.anthropic,
      ...overrides.anthropic,
    },
    ollama: {
      ...DEFAULT_CONFIG.ollama,
      ...overrides.ollama,
    },
    permissions: {
      ...DEFAULT_CONFIG.permissions,
      ...overrides.permissions,
    },
    contextManager: {
      ...DEFAULT_CONFIG.contextManager,
      ...overrides.contextManager,
    },
  }
}

export function getAvailableModels(): Array<{ id: string; name: string; tier: string }> {
  return [...ANTHROPIC_MODELS]
}

/** Get Ollama models from the configured server */
export async function getAvailableOllamaModels(
  baseUrl: string = DEFAULT_CONFIG.ollama.baseUrl
): Promise<Array<{ id: string; name: string; size: number }>> {
  const models = await listOllamaModels(baseUrl)
  return models.map((model) => ({
    id: model.name,
    name: model.name,
    size: model.size,
  }))
}

export function isValidApiKey(key: string): boolean {
  return key.startsWith('sk-ant-') && key.length > 30
}

// ── Config Persistence (via Store) ─────────────────────────────────────────

export type PersistedConfig = {
  apiKey: string
  model: string
  maxTurns: number
  maxBudgetUsd: number
  permissionMode: string
  thinkingEnabled: boolean
  thinkingBudget: number
  autoCompact: boolean
  appendSystemPrompt: string
  // Ollama-specific
  ollamaBaseUrl: string
  ollamaModel: string
  ollamaContextWindow: number
  ollamaThinkingEnabled: boolean
  ollamaKeepAlive: string
  // Session
  sessionPersistence: boolean
  walkParentDirs: boolean
}

export function toPersistedConfig(config: FullEngineConfig): PersistedConfig {
  return {
    apiKey: config.anthropic.apiKey,
    model: config.anthropic.model,
    maxTurns: config.maxTurns,
    maxBudgetUsd: config.maxBudgetUsd,
    permissionMode: config.permissions.mode,
    thinkingEnabled: config.thinking.type === 'enabled',
    thinkingBudget: config.thinking.type === 'enabled' ? config.thinking.budgetTokens : 0,
    autoCompact: config.compactionThreshold > 0,
    appendSystemPrompt: config.appendSystemPrompt,
    ollamaBaseUrl: config.ollama.baseUrl,
    ollamaModel: config.ollama.model,
    ollamaContextWindow: config.ollama.contextWindow ?? 128000,
    ollamaThinkingEnabled: config.ollama.thinkingEnabled ?? false,
    ollamaKeepAlive: config.ollama.keepAlive ?? '30m',
    sessionPersistence: config.sessionPersistence,
    walkParentDirs: config.walkParentDirs,
  }
}

export function fromPersistedConfig(persisted: PersistedConfig): Partial<FullEngineConfig> {
  return {
    anthropic: {
      apiKey: persisted.apiKey,
      model: persisted.model,
    },
    ollama: {
      baseUrl: persisted.ollamaBaseUrl || DEFAULT_CONFIG.ollama.baseUrl,
      model: persisted.ollamaModel || DEFAULT_CONFIG.ollama.model,
      contextWindow: persisted.ollamaContextWindow || 128000,
      thinkingEnabled: persisted.ollamaThinkingEnabled ?? false,
      keepAlive: persisted.ollamaKeepAlive || '30m',
    },
    maxTurns: persisted.maxTurns,
    maxBudgetUsd: persisted.maxBudgetUsd,
    permissions: {
      ...DEFAULT_PERMISSION_CONFIG,
      mode: persisted.permissionMode as FullEngineConfig['permissions']['mode'],
    },
    thinking: persisted.thinkingEnabled
      ? { type: 'enabled', budgetTokens: persisted.thinkingBudget || 10000 }
      : { type: 'disabled' },
    appendSystemPrompt: persisted.appendSystemPrompt,
    sessionPersistence: persisted.sessionPersistence ?? true,
    walkParentDirs: persisted.walkParentDirs ?? true,
  }
}
