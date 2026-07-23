// ── Memory System (ported from Claude Code) ─────────────────────────────────
// Mirrors: claude-code-main/src/memory/ + memdir/
// Handles: CLAUDE.md loading and memory entries
//
// Enhanced with:
// - Recursive CLAUDE.md discovery (parent directory walking)
// - Memory hierarchy (chat → project → global)
// - .claude/settings.json preferences loading

import { invoke } from '@tauri-apps/api/core'

// ── Memory Configuration ───────────────────────────────────────────────────

export type MemoryConfig = {
  /** Project root directory */
  projectDir: string
  /** Global memory directory */
  globalMemoryDir?: string
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
  scope: 'global' | 'project' | 'chat'
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
  return 'chat'
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
  threadId: string
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
  chatEntries?: Array<{
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
  const chatEntries = (snapshot.chatEntries ?? [])
    .filter((entry) => !['run_input', 'run_output', 'draft_knowledge'].includes(entry.category))
    .slice(0, 24)
    .map((entry) => `[${entry.category}] ${entry.key}: ${entry.content.trim()}`)
    .filter(Boolean)
  const userEntries = snapshot.userProfile
    .map((entry) => entry.value.trim())
    .filter(Boolean)

  return [
    renderMemorySection('MEMORY (curated agent notes)', agentEntries, MEMORY_CHAR_LIMIT),
    renderMemorySection('USER PROFILE', userEntries, USER_CHAR_LIMIT),
    chatEntries.length > 0
      ? `CHAT MEMORY [${chatEntries.length} entries]\n${chatEntries.join('\nÂ§\n')}`
      : '',
    sharedEntries.length > 0
      ? `SHARED KNOWLEDGE SNAPSHOT [${sharedEntries.length} entries]\n${sharedEntries.join('\n§\n')}`
      : '',
  ].filter(Boolean).join('\n\n---\n\n')
}

export async function loadFrozenMemorySnapshot(threadId?: string): Promise<FrozenMemorySnapshot | null> {
  try {
    return threadId
      ? await invoke<FrozenMemorySnapshot>('chat_memory_snapshot', { threadId })
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
  sourceRunId?: string,
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
      sourceRunId: sourceRunId ?? null,
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
      sourceRunId: null,
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
