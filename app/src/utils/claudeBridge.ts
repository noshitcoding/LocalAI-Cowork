import type { ClaudePermissionMode } from '../stores/coworkStore'

export type ParsedSlashCommand = {
  command: string
  args: string
} | null

export type SlashCommandDefinition = {
  command: string
  args?: string
  description: string
}

export const SLASH_COMMAND_DEFINITIONS: SlashCommandDefinition[] = [
  { command: '/help', description: 'Show this help' },
  { command: '/tools', description: 'Show active tool configuration' },
  { command: '/mode', args: 'plan|execute', description: 'Toggle plan mode' },
  { command: '/permissions', args: '<mode>', description: 'default | acceptEdits | bypassPermissions | dontAsk | plan' },
  { command: '/plan', args: '<prompt>', description: 'Run prompt as a planning request' },
  { command: '/fetch', args: '<url>', description: 'Load URL and show text excerpt' },
  { command: '/tool', args: '<name> <args>', description: 'Use tool dispatcher' },
  { command: '/todo', args: 'add <titel> | list', description: 'Simple todo control' },
]

export function parseSlashCommand(input: string): ParsedSlashCommand {
  const trimmed = input.trim()
  if (!trimmed.startsWith('/')) return null

  const firstSpace = trimmed.indexOf(' ')
  if (firstSpace < 0) {
    return {
      command: trimmed.slice(1).toLowerCase(),
      args: '',
    }
  }

  return {
    command: trimmed.slice(1, firstSpace).toLowerCase(),
    args: trimmed.slice(firstSpace + 1).trim(),
  }
}

export function buildClaudeSystemAddendum(input: {
  globalInstruction: string
  planMode: boolean
  permissionMode: ClaudePermissionMode
  enabledTools: string[]
}): string {
  const sections: string[] = []

  if (input.globalInstruction.trim()) {
    sections.push(`Project instruction: ${input.globalInstruction.trim()}`)
  }

  if (input.planMode) {
    sections.push('Plan mode is active: output only plan/analysis, no execution instructions with destructive steps.')
  }

  if (input.permissionMode !== 'default') {
    sections.push(`Permission-Mode: ${input.permissionMode}`)
  }

  if (sections.length === 0) return ''

  return `[SYSTEM-CONTEXT]\n${sections.join('\n')}\n[/SYSTEM-CONTEXT]`
}

export function buildSlashHelpText(pluginSkillLines: string[] = []): string {
  const baseLines = [
    'Verfuegbare slash commands:',
    ...SLASH_COMMAND_DEFINITIONS.map((definition) => {
      const usage = [definition.command, definition.args].filter(Boolean).join(' ')
      return `${usage} - ${definition.description}`
    }),
  ]

  if (pluginSkillLines.length > 0) {
    baseLines.push('', 'Active Plugin-Skills:')
    pluginSkillLines.forEach((line) => baseLines.push(line))
  }

  return baseLines.join('\n')
}

function wildcardToRegex(pattern: string): RegExp {
  const escaped = pattern
    .replace(/[.+?^${}()|[\]\\]/g, '\\$&')
    .replace(/\*/g, '.*')
  return new RegExp(`^${escaped}$`, 'i')
}

export function isToolDeniedByRules(toolId: string, target: string | null, rules: string[]): boolean {
  const normalizedTarget = (target ?? '').trim()

  return rules.some((rule) => {
    const trimmed = rule.trim()
    if (!trimmed) return false

    if (trimmed.includes(':')) {
      const [ruleTool, rulePattern] = trimmed.split(':', 2)
      if (ruleTool.trim().toLowerCase() !== toolId.toLowerCase()) return false
      if (!normalizedTarget) return true
      return wildcardToRegex(rulePattern.trim()).test(normalizedTarget)
    }

    return trimmed.toLowerCase() === toolId.toLowerCase()
  })
}
