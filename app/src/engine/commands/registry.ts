// ── Command Registry (ported from Claude Code) ─────────────────────────────
// Mirrors: claude-code-main/src/commands.ts
// Slash commands that users type in the chat: /help, /clear, /model, etc.

import type { Command, CommandCategory, ToolUseContext } from '../types'

// ── Command Registry ───────────────────────────────────────────────────────

const commandRegistry: Command[] = []

export function registerCommand(command: Command): void {
  commandRegistry.push(command)
}

export function getAllCommands(): Command[] {
  return commandRegistry
}

export function getCommandsByCategory(category: CommandCategory): Command[] {
  return commandRegistry.filter(c => c.category === category)
}

export function findCommand(name: string): Command | undefined {
  return commandRegistry.find(c =>
    c.name === name || c.name === `/${name}`,
  )
}

export function parseCommand(input: string): { command: string; args: string } | null {
  const trimmed = input.trim()
  if (!trimmed.startsWith('/')) return null
  const spaceIdx = trimmed.indexOf(' ')
  if (spaceIdx === -1) {
    return { command: trimmed.slice(1), args: '' }
  }
  return {
    command: trimmed.slice(1, spaceIdx),
    args: trimmed.slice(spaceIdx + 1).trim(),
  }
}

export async function executeCommand(input: string, context: ToolUseContext): Promise<string | null> {
  const parsed = parseCommand(input)
  if (!parsed) return null

  const cmd = findCommand(parsed.command)
  if (!cmd) return `Unknown Command: /${parsed.command}. Type /help for alle commands.`

  if (cmd.isAvailable && !cmd.isAvailable()) {
    return `Command /${parsed.command} is currently not available.`
  }

  const result = await cmd.call(parsed.args, context)
  return result ?? null
}

// ── Built-in Commands ──────────────────────────────────────────────────────

const helpCommand: Command = {
  name: '/help',
  description: 'Shows all available commands.',
  shortDescription: 'Help',
  category: 'session',
  examples: ['/help', '/help config'],
  async call(args) {
    const commands = args
      ? getCommandsByCategory(args as CommandCategory)
      : getAllCommands()

    if (commands.length === 0) {
      return `No commands found in category "${args}".`
    }

    const grouped = new Map<string, Command[]>()
    for (const cmd of commands) {
      const cat = cmd.category
      if (!grouped.has(cat)) grouped.set(cat, [])
      grouped.get(cat)!.push(cmd)
    }

    const lines: string[] = ['### Verfuegbare commands\n']
    for (const [category, cmds] of grouped) {
      lines.push(`**${category}**`)
      for (const cmd of cmds) {
        lines.push(`  ${cmd.name} — ${cmd.shortDescription ?? cmd.description}`)
      }
      lines.push('')
    }
    return lines.join('\n')
  },
}

const clearCommand: Command = {
  name: '/clear',
  description: 'Deletes the chat history of the current session.',
  shortDescription: 'Clear chat',
  category: 'session',
  async call(_args, context) {
    // The UI layer handles the actual clearing
    context.setAppState(prev => ({
      ...prev,
      turnCount: 0,
    }))
    return 'Chat geleert.'
  },
}

const modelCommand: Command = {
  name: '/model',
  description: 'Shows or changes the current model.',
  shortDescription: 'Show/change model',
  category: 'config',
  examples: ['/model', '/model claude-sonnet-4-20250514'],
  async call(args, context) {
    if (!args) {
      return `Current model: ${context.model}`
    }
    // Model switch is handled by the engine config
    return `Model changed to: ${args}`
  },
}

const statusCommand: Command = {
  name: '/status',
  description: 'Shows the current session status.',
  shortDescription: 'Session status',
  category: 'session',
  async call(_args, context) {
    const state = context.getAppState()
    return [
      '### Session status',
      `- **Model:** ${context.model}`,
      `- **CWD:** ${state.cwd}`,
      `- **Turns:** ${state.turnCount}`,
      `- **Tokens:** ${state.totalTokens.input} input / ${state.totalTokens.output} output`,
      `- **Costs:** $${state.totalCostUsd.toFixed(4)}`,
      `- **Plan mode:** ${state.planMode ? 'Active' : 'Inactive'}`,
      `- **Active Tasks:** ${state.activeTasks.length}`,
    ].join('\n')
  },
}

const planCommand: Command = {
  name: '/plan',
  description: 'Switches to plan mode (suggestions only, no execution).',
  shortDescription: 'Plan-Mode',
  category: 'planning',
  async call(_args, context) {
    const state = context.getAppState()
    const newPlanMode = !state.planMode
    context.setAppState(prev => ({ ...prev, planMode: newPlanMode }))
    return newPlanMode
      ? '📋 Plan mode enabled. Changes will only be suggested.'
      : '🔧 Plan mode disabled. Changes will be executed.'
  },
}

const compactCommand: Command = {
  name: '/compact',
  description: 'Compacts chat history to save tokens.',
  shortDescription: 'Compact chat',
  category: 'session',
  async call(_args) {
    // Compaction is handled by the memory system
    return 'Chat history is being compacted...'
  },
}

const costCommand: Command = {
  name: '/cost',
  description: 'Shows the API costs accumulated in this session.',
  shortDescription: 'Show costs',
  category: 'session',
  async call(_args, context) {
    const state = context.getAppState()
    return [
      '### API costs',
      `- **Total:** $${state.totalCostUsd.toFixed(4)}`,
      `- **Input tokens:** ${state.totalTokens.input.toLocaleString()}`,
      `- **Output tokens:** ${state.totalTokens.output.toLocaleString()}`,
    ].join('\n')
  },
}

const debugCommand: Command = {
  name: '/debug',
  description: 'Enables/disables debug mode.',
  shortDescription: 'Debug-Mode',
  category: 'debug',
  async call(_args, context) {
    const newDebug = !context.debug
    return `Debug mode: ${newDebug ? 'enabled' : 'disabled'}`
  },
}

const cwdCommand: Command = {
  name: '/cwd',
  description: 'Shows or changes the working directory.',
  shortDescription: 'Working directory',
  category: 'navigation',
  examples: ['/cwd', '/cwd /home/user/project'],
  async call(args, context) {
    if (!args) {
      return `Working directory: ${context.cwd}`
    }
    context.setAppState(prev => ({ ...prev, cwd: args }))
    return `Working directory changed to: ${args}`
  },
}

const agentsCommand: Command = {
  name: '/agents',
  description: 'Lists available agents.',
  shortDescription: 'Show agents',
  category: 'agents',
  async call(_args, context) {
    if (context.agentDefinitions.length === 0) {
      return 'No agents configured.'
    }
    const lines = context.agentDefinitions.map(a =>
      `- **${a.name}** (${a.type}): ${a.description}`,
    )
    return `### Available agents\n${lines.join('\n')}`
  },
}

const toolsCommand: Command = {
  name: '/tools',
  description: 'Lists available tools.',
  shortDescription: 'Show tools',
  category: 'tools',
  async call(_args, context) {
    const lines = context.tools.map(t =>
      `- **${t.name}** [${t.category}] (${t.riskLevel ?? 'low'}): ${t.description}`,
    )
    return `### Verfuegbare Tools (${context.tools.length})\n${lines.join('\n')}`
  },
}

const mcpCommand: Command = {
  name: '/mcp',
  description: 'Shows MCP server connections and tools.',
  shortDescription: 'MCP servers',
  category: 'mcp',
  async call(_args, context) {
    if (context.mcpConnections.length === 0) {
      return 'No MCP servers connected.'
    }
    const lines: string[] = ['### MCP servers']
    for (const conn of context.mcpConnections) {
      lines.push(`\n**${conn.name}** (${conn.serverType}) — ${conn.connected ? '✅ Connected' : '❌ Getrennt'}`)
      if (conn.tools.length > 0) {
        lines.push('  Tools: ' + conn.tools.map(t => t.name).join(', '))
      }
    }
    return lines.join('\n')
  },
}

const memoryCommand: Command = {
  name: '/memory',
  description: 'Shows or edits the memory store.',
  shortDescription: 'Memory',
  category: 'memory',
  examples: ['/memory', '/memory add key=value'],
  async call(args, context) {
    if (!args) {
      return context.memoryContent
        ? `### Memory content\n${context.memoryContent}`
        : 'No memory content loaded.'
    }
    return `Memory command: ${args}`
  },
}

const permissionsCommand: Command = {
  name: '/permissions',
  description: 'Shows current permission rules.',
  shortDescription: 'Permissions',
  category: 'security',
  async call(_args, context) {
    const p = context.permissionContext
    const lines = [
      `### Permissions (mode: ${p.mode})`,
      `\n**Allowed directories:** ${p.allowedDirectories.length > 0 ? p.allowedDirectories.join(', ') : 'No'}`,
    ]
    if (p.allowRules.length > 0) {
      lines.push(`\n**Allow-Regeln:** ${p.allowRules.map(r => `${r.pattern} (${r.source})`).join(', ')}`)
    }
    if (p.denyRules.length > 0) {
      lines.push(`\n**Deny-Regeln:** ${p.denyRules.map(r => `${r.pattern} (${r.source})`).join(', ')}`)
    }
    return lines.join('\n')
  },
}

const exportCommand: Command = {
  name: '/export',
  description: 'Exports the current chat as Markdown.',
  shortDescription: 'Export chat',
  category: 'export',
  async call(_args, context) {
    const lines: string[] = ['# Chat-Export\n']
    for (const msg of context.messages) {
      if (msg.type === 'user') {
        const text = msg.content
          .filter((b): b is { type: 'text'; text: string } => b.type === 'text')
          .map(b => b.text)
          .join('\n')
        lines.push(`## User\n${text}\n`)
      } else if (msg.type === 'assistant') {
        const text = msg.content
          .filter((b): b is { type: 'text'; text: string } => b.type === 'text')
          .map(b => b.text)
          .join('\n')
        lines.push(`## Assistant (${msg.model})\n${text}\n`)
      }
    }
    return lines.join('\n')
  },
}

// ── Register All Built-in Commands ─────────────────────────────────────────

export function registerBuiltinCommands(): void {
  const commands = [
    helpCommand,
    clearCommand,
    modelCommand,
    statusCommand,
    planCommand,
    compactCommand,
    costCommand,
    debugCommand,
    cwdCommand,
    agentsCommand,
    toolsCommand,
    mcpCommand,
    memoryCommand,
    permissionsCommand,
    exportCommand,
  ]
  for (const cmd of commands) {
    registerCommand(cmd)
  }
}
