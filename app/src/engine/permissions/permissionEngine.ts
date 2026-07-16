// ── Permission System (ported from Claude Code) ─────────────────────────────
// Mirrors: claude-code-main/src/permissions/
// Handles: tool permission rules, file access controls, approval flow

import type {
  ToolPermissionContext,
  ToolPermissionRule,
  PermissionMode,
  Tool,
} from '../types'
import { getEmptyToolPermissionContext } from '../types'

// ── Permission Configuration ───────────────────────────────────────────────

export type PermissionConfig = {
  mode: PermissionMode
  allowedDirectories: string[]
  rules: ToolPermissionRule[]
  autoApproveReadOnly: boolean
  autoApproveTimeout: number // ms, 0 = never
}

export const DEFAULT_PERMISSION_CONFIG: PermissionConfig = {
  mode: 'default',
  allowedDirectories: [],
  rules: [],
  autoApproveReadOnly: true,
  autoApproveTimeout: 0,
}

// ── Build Permission Context ───────────────────────────────────────────────

export function buildPermissionContext(config: PermissionConfig): ToolPermissionContext {
  const ctx = getEmptyToolPermissionContext()
  ctx.mode = config.mode
  ctx.allowedDirectories = config.allowedDirectories

  for (const rule of config.rules) {
    switch (rule.decision) {
      case 'allow':
        ctx.allowRules.push(rule)
        break
      case 'deny':
        ctx.denyRules.push(rule)
        break
      case 'ask':
        ctx.askRules.push(rule)
        break
    }
  }

  return ctx
}

// ── Permission Checking ────────────────────────────────────────────────────

export function checkToolPermission(
  tool: Tool,
  input: Record<string, unknown>,
  context: ToolPermissionContext,
): 'allow' | 'deny' | 'ask' {
  // Bypass mode: always allow
  if (context.mode === 'bypass') return 'allow'

  // Check deny rules first (highest priority)
  for (const rule of context.denyRules) {
    if (matchesRule(tool.name, rule)) return 'deny'
  }

  // Check allow rules
  for (const rule of context.allowRules) {
    if (matchesRule(tool.name, rule)) return 'allow'
  }

  // Check ask rules
  for (const rule of context.askRules) {
    if (matchesRule(tool.name, rule)) return 'ask'
  }

  // Default behaviors by mode
  switch (context.mode) {
    case 'strict':
      return tool.isReadOnly?.(input) ? 'allow' : 'ask'
    case 'plan':
      return tool.isReadOnly?.(input) ? 'allow' : 'ask'
    case 'default':
      if (tool.isReadOnly?.(input)) return 'allow'
      if (tool.riskLevel === 'low') return 'allow'
      if (tool.riskLevel === 'high') return 'ask'
      return 'ask'
    default:
      return 'ask'
  }
}

// ── File Access Control ────────────────────────────────────────────────────

export function isPathAllowed(
  path: string,
  allowedDirectories: string[],
): boolean {
  if (allowedDirectories.length === 0) return true // No restrictions

  const normalizedPath = normalizePath(path)
  return allowedDirectories.some(dir => {
    const normalizedDir = normalizePath(dir)
    return normalizedPath.startsWith(normalizedDir)
  })
}

// ── Rule Builder ───────────────────────────────────────────────────────────

export function createAllowRule(pattern: string, source: string): ToolPermissionRule {
  return { pattern, decision: 'allow', source }
}

export function createDenyRule(pattern: string, source: string): ToolPermissionRule {
  return { pattern, decision: 'deny', source }
}

export function createAskRule(pattern: string, source: string): ToolPermissionRule {
  return { pattern, decision: 'ask', source }
}

// ── Default Security Rules ─────────────────────────────────────────────────

export function getDefaultSecurityRules(): ToolPermissionRule[] {
  return [
    // Always allow read-only operations
    createAllowRule('Read|Glob|Grep|WebFetch|WebSearch|TaskList|MemoryRead|SessionSearch', 'default-security'),
    // Ask for write operations
    createAskRule('Write|Edit|Bash|Agent|MCPTool', 'default-security'),
    // Ask for destructive operations
    createAskRule('TaskCreate|MemoryWrite|Skill', 'default-security'),
  ]
}

// ── Helpers ────────────────────────────────────────────────────────────────

function matchesRule(toolName: string, rule: ToolPermissionRule): boolean {
  try {
    return new RegExp(rule.pattern, 'i').test(toolName)
  } catch {
    return toolName.toLowerCase().includes(rule.pattern.toLowerCase())
  }
}

function normalizePath(path: string): string {
  return path.replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase()
}
