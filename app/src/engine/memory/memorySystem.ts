// ── Memory System (ported from Claude Code) ─────────────────────────────────
// Mirrors: claude-code-main/src/memory/ + memdir/
// Handles: CLAUDE.md loading, memory entries, conversation compaction
//
// Enhanced with:
// - Recursive CLAUDE.md discovery (parent directory walking)
// - LLM-powered compaction via Ollama
// - Memory hierarchy (session → project → global)
// - .claude/settings.json preferences loading

import { invoke } from '@tauri-apps/api/core'
import type { Message } from '../types'
import { extractTextContent, createSystemMessage } from '../types'

// ── Memory Configuration ───────────────────────────────────────────────────

export type MemoryConfig = {
  /** Project root directory */
  projectDir: string
  /** Global memory directory */
  globalMemoryDir?: string
  /** Maximum context size before compaction */
  maxContextTokens?: number
  /** Enable auto-compaction */
  autoCompact?: boolean
  /** Walk parent directories for CLAUDE.md files */
  walkParents?: boolean
  /** Maximum parent levels to walk */
  maxParentLevels?: number
}

// ── CLAUDE.md / Project Memory ─────────────────────────────────────────────
// Mirrors: claude-code-main/src/memory/claudemd.ts

const MEMORY_FILES = [
  'CLAUDE.md',
  'MEMORY.md',
  'USER.md',
  '.claude/memory.md',
  '.claude/settings.json',
  'AGENTS.md',
]

/** Additional memory files for LocalAI Cowork-specific features */
const COWORK_MEMORY_FILES = [
  '.cowork/memory.md',
  '.cowork/DRAFT_MEMORY.md',
  '.cowork/DRAFT_KNOWLEDGE.md',
  '.cowork/config.json',
  '.cowork/agents.md',
]

/**
 * Load project memory from standard files (CLAUDE.md, .claude/memory.md, etc.)
 * Enhanced: Also loads .cowork/* files and walks parent directories.
 */
export async function loadProjectMemory(
  projectDir: string,
  options?: { walkParents?: boolean; maxParentLevels?: number },
): Promise<string> {
  const parts: string[] = []
  const allFiles = [...MEMORY_FILES, ...COWORK_MEMORY_FILES]

  // Load from project directory
  for (const file of allFiles) {
    const fullPath = `${projectDir}/${file}`
    try {
      const content = await invoke<string>('fs_extract_text', { path: fullPath })
      if (content && content.trim().length > 0) {
        parts.push(`# ${file}\n\n${content.trim()}`)
      }
    } catch {
      // File doesn't exist — skip silently
    }
  }

  // Walk parent directories for CLAUDE.md (Claude Code feature)
  if (options?.walkParents !== false) {
    const maxLevels = options?.maxParentLevels ?? 3
    let currentDir = projectDir

    for (let i = 0; i < maxLevels; i++) {
      // Go up one directory
      const parentDir = getParentDir(currentDir)
      if (!parentDir || parentDir === currentDir) break
      currentDir = parentDir

      try {
        const content = await invoke<string>('fs_extract_text', { path: `${currentDir}/CLAUDE.md` })
        if (content && content.trim().length > 0) {
          parts.push(`# CLAUDE.md (${currentDir})\n\n${content.trim()}`)
        }
      } catch {
        // File doesn't exist — skip
      }
    }
  }

  return parts.join('\n\n---\n\n')
}

/**
 * Load global memory from the user's home directory
 */
export async function loadGlobalMemory(globalDir?: string): Promise<string> {
  if (!globalDir) return ''
  const parts: string[] = []

  for (const file of ['CLAUDE.md', '.cowork/memory.md']) {
    try {
      const content = await invoke<string>('fs_extract_text', { path: `${globalDir}/${file}` })
      if (content?.trim()) {
        parts.push(content.trim())
      }
    } catch {
      // skip
    }
  }

  return parts.join('\n\n')
}

/**
 * Load .claude/settings.json preferences
 */
export async function loadProjectSettings(projectDir: string): Promise<Record<string, unknown> | null> {
  try {
    const content = await invoke<string>('fs_extract_text', { path: `${projectDir}/.claude/settings.json` })
    if (content) return JSON.parse(content)
  } catch {
    // no settings file
  }
  return null
}

/**
 * Save content to a project memory file
 */
export async function saveProjectMemory(projectDir: string, filename: string, content: string): Promise<void> {
  const fullPath = `${projectDir}/${filename}`
  await invoke('fs_write_text_file', {
    path: fullPath,
    content,
    createBackup: true,
  })
}

// ── Database-backed Memory ─────────────────────────────────────────────────

export type MemoryEntry = {
  id: string
  scope: 'global' | 'project' | 'session'
  key: string
  content: string
  category: string
  confidence: number
  createdAt: number
  updatedAt: number
}

type MemoryEntryRow = {
  id: string
  scope: string
  category: string
  key: string
  content: string
  confidence: number
  created_at: string
  updated_at: string
}

function toBackendMemoryScope(scope: MemoryEntry['scope'] | string | undefined): string | undefined {
  if (!scope) return undefined
  if (scope === 'project') return 'agent'
  if (scope === 'global') return 'shared'
  return scope
}

function fromBackendMemoryScope(scope: string): MemoryEntry['scope'] {
  if (scope === 'agent') return 'project'
  if (scope === 'shared') return 'global'
  return 'session'
}

export type RuntimeInstruction = {
  id: string
  scopeType: string
  scopeRef: string | null
  title: string
  content: string
  enabled: boolean
  priority: number
}

export type FrozenMemorySnapshot = {
  sessionId: string
  agentEntries: Array<{
    id: string
    scope: string
    category: string
    key: string
    content: string
    confidence: number
  }>
  sharedEntries: Array<{
    id: string
    scope: string
    category: string
    key: string
    content: string
    confidence: number
  }>
  userProfile: Array<{
    id: string
    key: string
    value: string
    source: string
    confidence: number
  }>
  createdAt: string
}

export type AutomaticMemoryCandidate = {
  target: 'memory' | 'user'
  content: string
}

const MEMORY_CHAR_LIMIT = 2200
const USER_CHAR_LIMIT = 1375
const DRAFT_KNOWLEDGE_FILE = '.cowork/DRAFT_KNOWLEDGE.md'
const DRAFT_HEADER = `# Draft Knowledge Base

Automatically captured high-signal memory candidates. Review, edit, or promote these through the Memory tool. This file is included as project context, but entries remain drafts until curated.

## Candidates`

function countCharacters(entries: string[]): number {
  return entries.reduce((total, entry) => total + Array.from(entry).length, 0)
    + Math.max(0, entries.length - 1) * 3
}

function renderMemorySection(title: string, entries: string[], limit: number): string {
  if (entries.length === 0) return ''
  const used = countCharacters(entries)
  const percent = Math.min(100, Math.round((used / limit) * 100))
  return [
    `${title} [${percent}% - ${used}/${limit} chars]`,
    entries.join('\n§\n'),
  ].join('\n')
}

export function renderFrozenMemorySnapshot(snapshot: FrozenMemorySnapshot): string {
  const agentEntries = snapshot.agentEntries
    .filter((entry) => entry.category === 'curated')
    .map((entry) => entry.content.trim())
    .filter(Boolean)
  const sharedEntries = snapshot.sharedEntries
    .filter((entry) => entry.category !== 'draft_knowledge')
    .slice(0, 24)
    .map((entry) => `[${entry.category}] ${entry.key}: ${entry.content.trim()}`)
    .filter(Boolean)
  const userEntries = snapshot.userProfile
    .map((entry) => entry.value.trim())
    .filter(Boolean)

  return [
    renderMemorySection('MEMORY (curated agent notes)', agentEntries, MEMORY_CHAR_LIMIT),
    renderMemorySection('USER PROFILE', userEntries, USER_CHAR_LIMIT),
    sharedEntries.length > 0
      ? `SHARED KNOWLEDGE SNAPSHOT [${sharedEntries.length} entries]\n${sharedEntries.join('\n§\n')}`
      : '',
  ].filter(Boolean).join('\n\n---\n\n')
}

export async function loadFrozenMemorySnapshot(sessionId?: string): Promise<FrozenMemorySnapshot | null> {
  try {
    return sessionId
      ? await invoke<FrozenMemorySnapshot>('session_memory_snapshot', { sessionId })
      : await invoke<FrozenMemorySnapshot>('memory_snapshot')
  } catch {
    return null
  }
}

function normalizeCandidate(value: string): string {
  return value
    .replace(/\s+/g, ' ')
    .replace(/^[\s:,-]+|[\s]+$/g, '')
    .slice(0, 500)
}

function isUnsafeDraft(value: string): boolean {
  return /(?:api[_ -]?key|access[_ -]?token|password|passwort|secret|credential|private key)\s*[:=]/i.test(value)
    || /ignore (?:all )?previous instructions|reveal (?:the )?system prompt|exfiltrat/i.test(value)
}

export function extractAutomaticMemoryCandidates(userInput: string): AutomaticMemoryCandidate[] {
  const compact = normalizeCandidate(userInput)
  if (!compact || compact.length < 12 || isUnsafeDraft(compact)) return []

  const explicitMatch = compact.match(/(?:remember(?: that)?|merke dir(?:,? dass)?|bitte merken|vergiss nicht)\s*[:,-]?\s*(.+)/i)
  if (explicitMatch?.[1]) {
    const content = normalizeCandidate(explicitMatch[1])
    return content && !isUnsafeDraft(content) ? [{ target: 'memory', content }] : []
  }

  const isPreference = /\b(?:i prefer|ich bevorzuge|ich mag|nenne mich|please answer|bitte antworte|communication style)\b/i.test(compact)
  if (isPreference) return [{ target: 'user', content: compact }]

  const isReusableFact = /\b(?:project uses|das projekt nutzt|we use|wir verwenden|runs on|laeuft auf|läuft auf|always use|verwende immer|do not use|don't use|verwende nicht)\b/i.test(compact)
  return isReusableFact ? [{ target: 'memory', content: compact }] : []
}

function stableDraftKey(value: string): string {
  let hash = 2166136261
  for (const character of value) {
    hash ^= character.charCodeAt(0)
    hash = Math.imul(hash, 16777619)
  }
  return `draft-${(hash >>> 0).toString(16).padStart(8, '0')}`
}

export async function captureAutomaticMemoryDraft(
  projectDir: string,
  userInput: string,
  sourceSessionId?: string,
): Promise<AutomaticMemoryCandidate[]> {
  const candidates = extractAutomaticMemoryCandidates(userInput)
  if (candidates.length === 0) return []

  const draftPath = `${projectDir}/${DRAFT_KNOWLEDGE_FILE}`
  let existing = ''
  try {
    existing = await invoke<string>('fs_extract_text', { path: draftPath })
  } catch {
    // The draft file is created lazily on the first high-signal candidate.
  }

  const lines = existing.trim() ? existing.trim().split(/\r?\n/) : DRAFT_HEADER.split('\n')
  let changed = false
  for (const candidate of candidates) {
    const line = `- [${candidate.target}] ${candidate.content}`
    if (!lines.some((existingLine) => existingLine.trim().toLowerCase() === line.toLowerCase())) {
      lines.push(line)
      changed = true
    }
    await invoke('memory_upsert', {
      id: crypto.randomUUID(),
      scope: 'shared',
      category: 'draft_knowledge',
      key: stableDraftKey(`${candidate.target}:${candidate.content}`),
      content: candidate.content,
      sourceSessionId: sourceSessionId ?? null,
      confidence: 0.6,
    })
  }

  if (changed) {
    const bounded = lines.join('\n').slice(-20_000)
    await invoke('fs_write_text_file', {
      path: draftPath,
      content: bounded.startsWith('# Draft Knowledge Base') ? bounded : `${DRAFT_HEADER}\n${bounded}`,
      createBackup: true,
    })
  }
  return candidates
}

/**
 * Store a memory entry in the database
 */
export async function storeMemoryEntry(entry: Omit<MemoryEntry, 'id' | 'createdAt' | 'updatedAt'>): Promise<string> {
  const id = crypto.randomUUID()
  await invoke('memory_upsert', {
    id,
    scope: toBackendMemoryScope(entry.scope),
    key: entry.key,
    content: entry.content,
    category: entry.category,
    sourceSessionId: null,
    confidence: entry.confidence,
  })
  return id
}

/**
 * Retrieve memory entries from the database
 */
export async function getMemoryEntries(scope?: string, category?: string): Promise<MemoryEntry[]> {
  try {
    const rows = await invoke<MemoryEntryRow[]>('memory_search', {
      scope: toBackendMemoryScope(scope),
      category: category ?? null,
      keyword: null,
      limit: 200,
    })
    return rows.map((row) => ({
      id: row.id,
      scope: fromBackendMemoryScope(row.scope),
      key: row.key,
      content: row.content,
      category: row.category,
      confidence: row.confidence,
      createdAt: Date.parse(row.created_at),
      updatedAt: Date.parse(row.updated_at),
    }))
  } catch {
    return []
  }
}

export async function loadEffectiveRuntimeInstructions(projectDir: string): Promise<RuntimeInstruction[]> {
  try {
    return await invoke<RuntimeInstruction[]>('runtime_instruction_effective', { cwd: projectDir })
  } catch {
    return []
  }
}

export async function recallRelevantMemory(query: string, limit: number = 6): Promise<string[]> {
  const trimmed = query.trim()
  if (!trimmed) return []

  try {
    const rows = await invoke<Array<{ key: string; content: string; category: string }>>('memory_search', {
      scope: null,
      category: null,
      keyword: trimmed,
      limit: Math.max(limit * 3, 12),
    })
    return rows
      .filter((row) => !['run_input', 'run_output', 'context', 'draft_knowledge'].includes(row.category))
      .slice(0, limit)
      .map((row) => `[${row.category}] ${row.key}: ${row.content}`)
      .filter(Boolean)
  } catch {
    return []
  }
}

// ── Conversation Compaction ────────────────────────────────────────────────
// Now delegates to services/compact.ts for LLM-powered compaction.
// This function is kept for backward compatibility.
// For advanced compaction, use ContextManager from services/contextManager.ts.

/**
 * Estimate token count for a message (rough: ~4 chars per token)
 */
function estimateTokens(text: string): number {
  return Math.ceil(text.length / 4)
}

/**
 * Estimate total token count for a conversation
 */
export function estimateConversationTokens(messages: Message[]): number {
  return messages.reduce((sum, msg) => {
    return sum + estimateTokens(extractTextContent(msg))
  }, 0)
}

/**
 * Compact a conversation by summarizing older messages.
 * Returns a new message list with a summary system message replacing older messages.
 *
 * Note: For LLM-powered compaction, use autoCompact() from services/compact.ts
 */
export function compactConversation(
  messages: Message[],
  maxTokens: number = 100000,
): Message[] {
  const totalTokens = estimateConversationTokens(messages)
  if (totalTokens <= maxTokens) return messages

  // Find the split point — keep at least the last 10 messages
  const keepCount = Math.max(10, Math.floor(messages.length * 0.3))
  const toSummarize = messages.slice(0, messages.length - keepCount)
  const toKeep = messages.slice(messages.length - keepCount)

  // Summarize the older messages
  const summaryParts: string[] = []
  const toolUseSummaries: string[] = []

  for (const msg of toSummarize) {
    if (msg.type === 'assistant') {
      const text = extractTextContent(msg)
      if (text.length > 100) {
        summaryParts.push(`[Assistant]: ${text.slice(0, 200)}...`)
      }
      // Track tool uses
      const toolUses = msg.content.filter(b => b.type === 'tool_use')
      for (const tu of toolUses) {
        if (tu.type === 'tool_use') {
          toolUseSummaries.push(`${tu.name}(${JSON.stringify(tu.input).slice(0, 100)})`)
        }
      }
    } else if (msg.type === 'user') {
      const text = extractTextContent(msg)
      if (text.length > 50) {
        summaryParts.push(`[User]: ${text.slice(0, 150)}...`)
      }
    }
  }

  const summary = [
    `[Komprimierter Chatverlauf — ${toSummarize.length} Messages summarized]`,
    '',
    'Summary der bisherigen Konversation:',
    ...summaryParts.slice(0, 20),
    '',
    toolUseSummaries.length > 0 ? `Ausgefuehrte Tools: ${toolUseSummaries.slice(0, 15).join(', ')}` : '',
  ].filter(Boolean).join('\n')

  const summaryMessage = createSystemMessage(summary, 'compact_boundary')

  return [summaryMessage, ...toKeep]
}

// ── System Prompt Builder ──────────────────────────────────────────────────

/**
 * Build the full system prompt including project memory and context.
 * Enhanced: Also loads .cowork/* files and project settings.
 */
export async function buildSystemPromptWithMemory(
  projectDir: string,
  basePrompt: string,
  options?: {
    globalDir?: string
    userInput?: string
    frozenSnapshot?: FrozenMemorySnapshot | null
  },
): Promise<{
  systemPrompt: string
  memoryContent: string
  settings: Record<string, unknown> | null
  runtimeInstructions: RuntimeInstruction[]
  recalledMemory: string[]
}> {
  const [projectMemory, globalMemory, settings, runtimeInstructions, recalledMemory, liveSnapshot] = await Promise.all([
    loadProjectMemory(projectDir),
    loadGlobalMemory(options?.globalDir),
    loadProjectSettings(projectDir),
    loadEffectiveRuntimeInstructions(projectDir),
    recallRelevantMemory(options?.userInput ?? ''),
    options?.frozenSnapshot === undefined ? loadFrozenMemorySnapshot() : Promise.resolve(null),
  ])
  const frozenSnapshot = options?.frozenSnapshot ?? liveSnapshot
  const frozenMemoryBlock = frozenSnapshot ? renderFrozenMemorySnapshot(frozenSnapshot) : ''

  const instructionBlock = runtimeInstructions.length > 0
    ? runtimeInstructions
      .map((item) => `# ${item.title}\n${item.content}`)
      .join('\n\n---\n\n')
    : ''

  const recallBlock = recalledMemory.length > 0
    ? recalledMemory.join('\n')
    : ''

  const memoryContent = [frozenMemoryBlock, globalMemory, projectMemory, instructionBlock, recallBlock]
    .filter(Boolean)
    .join('\n\n---\n\n')

  // QueryEngine owns the single <memory> injection point.
  const systemPrompt = basePrompt

  return { systemPrompt, memoryContent, settings, runtimeInstructions, recalledMemory }
}

// ── Helper ─────────────────────────────────────────────────────────────────

function getParentDir(dir: string): string | null {
  // Windows path
  const winParts = dir.split('\\')
  if (winParts.length > 1) {
    winParts.pop()
    return winParts.join('\\')
  }
  // Unix path fallback
  const parts = dir.split('/')
  if (parts.length > 1) {
    parts.pop()
    return parts.join('/') || '/'
  }
  return null
}
