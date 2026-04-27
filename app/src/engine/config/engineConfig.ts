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

export const DEFAULT_SYSTEM_PROMPT = `Du bist ein hilfreicher KI-Assistent in einer Desktop-Anwendung (Open Cowork). Du hast Zugriff auf verschiedene Tools um Dateien zu lesen, zu schreiben, zu suchen, Shell-Befehle auszufuehren, und mehr.

Wichtige Regeln:
1. Fuehre Aenderungen direkt aus statt nur Vorschlaege zu machen, es sei denn der Plan-Modus ist aktiv.
2. Lies Dateien bevor du sie aenderst, um den Kontext zu verstehen.
3. Nutze Tools haeufig — lese, suche, und verifiziere.
4. Gib klare, praezise Antworten.
5. Bei Unsicherheit: frage den Benutzer mit dem AskUser-Tool.
  Wenn das Ziel jedoch klar ist (z. B. Dateien sortieren/strukturieren), fuehre es selbststaendig aus und frage nur bei fehlenden kritischen Angaben oder bei destruktiven Schritten.
6. Erstelle keine Dateien, die nicht benoetigt werden.
7. Teste Aenderungen wenn moeglich (Build, Tests etc.).
8. Fuer Dateiorganisation und Strukturarbeit nutze dedizierte Datei-Tools wie ListDir, CreateDirectory, MovePath und CopyPath statt nur Shell-Befehle zu beschreiben.

Du arbeitest in einem Windows-Umfeld mit PowerShell.`

export const DEFAULT_CONFIG: FullEngineConfig = {
  anthropic: {
    apiKey: '',
    model: 'claude-sonnet-4-20250514',
    maxTokens: 16384,
  },
  ollama: {
    baseUrl: 'http://192.168.178.82:11434',
    model: 'gpt-oss:20b',
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
