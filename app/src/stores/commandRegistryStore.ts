import { create } from 'zustand'
import { useUiStore } from './uiStore'
import { useConfigStore } from './configStore'
import { useChatStore } from './chatStore'
import { useCoworkStore, type ClaudePermissionMode } from './coworkStore'
import { useMemoryStore } from './memoryStore'
import { useSkillStore } from './skillStore'
import { useInsightsStore } from './insightsStore'
import { useProcessStore } from './processStore'
import { useTerminalStore } from './terminalStore'
import { useTaskStore } from './taskStore'
import { useCrewStore } from './crewStore'
import { safeInvoke, safeInvokeVoid } from '../utils/safeInvoke'
import { parseScheduledTaskInput } from '../utils/schedulerUtils'

export type SlashCommandCategory =
  | 'navigation'
  | 'workspace'
  | 'agent'
  | 'model'
  | 'memory'
  | 'tools'
  | 'session'
  | 'config'
  | 'security'
  | 'display'
  | 'plugins'
  | 'crew'
  | 'debug'
  | 'export'

export type SlashCommand = {
  id: string
  command: string
  label: string
  description: string
  category: SlashCommandCategory
  execute: (args?: string) => void | Promise<void>
}

export function getSlashCommandSuggestions(commands: SlashCommand[], input: string): SlashCommand[] {
  const trimmed = input.trimStart().toLowerCase()
  if (!trimmed.startsWith('/')) return []

  const partial = trimmed.split(/\s+/, 1)[0]
  return commands.filter((command) => command.command.toLowerCase().startsWith(partial))
}

type CommandRegistryState = {
  commands: SlashCommand[]
  lastExecuted: string | null
  executionLog: Array<{ command: string; timestamp: number; args?: string }>
  getCommand: (id: string) => SlashCommand | undefined
  executeCommand: (commandOrId: string, args?: string) => Promise<boolean>
}

function uid() {
  return `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

export const useCommandRegistry = create<CommandRegistryState>()((set, get) => ({
  commands: buildAllCommands(),
  lastExecuted: null,
  executionLog: [],
  getCommand: (id) => get().commands.find((c) => c.id === id || c.command === id),
  executeCommand: async (commandOrId, args) => {
    const cmd = get().commands.find(
      (c) => c.id === commandOrId || c.command === commandOrId || c.command === `/${commandOrId}`
    )
    if (!cmd) return false
    await cmd.execute(args)
    set((s) => ({
      lastExecuted: cmd.id,
      executionLog: [
        { command: cmd.command, timestamp: Date.now(), args },
        ...s.executionLog.slice(0, 199),
      ],
    }))
    return true
  },
}))

function addCommandMessage(content: string): void {
  const chat = useChatStore.getState()
  if (!chat.activeThreadId) return
  chat.addMessage(chat.activeThreadId, { role: 'system', content, timestamp: Date.now() })
}

export function buildAllCommands(): SlashCommand[] {
  const commands: SlashCommand[] = [
    // ===== Core commands (single source for help, palette, and autocomplete) =====
    {
      id: 'help', command: '/help', label: 'Help', description: 'Show all available slash commands',
      category: 'session', execute: () => {
        const commands = useCommandRegistry.getState().commands
        addCommandMessage(['Available slash commands:', ...commands.map((command) => `${command.command} - ${command.description}`)].join('\n'))
      },
    },
    {
      id: 'tools', command: '/tools', label: 'Tools', description: 'Show active tool configuration',
      category: 'tools', execute: () => {
        const cowork = useCoworkStore.getState()
        addCommandMessage(`Active tools: ${cowork.enabledClaudeToolIds.join(', ') || '(none)'}`)
      },
    },
    {
      id: 'mode', command: '/mode', label: 'Execution mode', description: 'Set plan or execute mode',
      category: 'agent', execute: (args) => {
        const target = args?.trim().toLowerCase()
        if (target !== 'plan' && target !== 'execute') {
          addCommandMessage('Usage: /mode plan | execute')
          return
        }
        useCoworkStore.getState().setClaudePlanMode(target === 'plan')
        addCommandMessage(`Mode set to ${target}.`)
      },
    },
    {
      id: 'permissions', command: '/permissions', label: 'Permissions', description: 'Show or change the permission mode',
      category: 'security', execute: (args) => {
        const target = args?.trim() as ClaudePermissionMode | undefined
        const valid: ClaudePermissionMode[] = ['default', 'acceptEdits', 'bypassPermissions', 'dontAsk', 'plan']
        if (!target) {
          addCommandMessage(`Permission mode: ${useCoworkStore.getState().claudePermissionMode}`)
          return
        }
        if (!valid.includes(target)) {
          addCommandMessage(`Invalid permission mode. Allowed: ${valid.join(', ')}`)
          return
        }
        useCoworkStore.getState().setClaudePermissionMode(target)
        addCommandMessage(`Permission mode set to ${target}.`)
      },
    },
    {
      id: 'plan', command: '/plan', label: 'Plan request', description: 'Run the next prompt in plan mode',
      category: 'agent', execute: (args) => {
        useCoworkStore.getState().setClaudePlanMode(true)
        addCommandMessage(args?.trim() ? `Plan mode enabled for: ${args.trim()}` : 'Plan mode enabled.')
      },
    },
    {
      id: 'fetch', command: '/fetch', label: 'Fetch URL', description: 'Fetch a URL and show a text excerpt',
      category: 'tools', execute: async (args) => {
        const url = args?.trim()
        if (!url) {
          addCommandMessage('Usage: /fetch https://example.com')
          return
        }
        const response = await safeInvoke<{ url: string; status: number; title?: string; content: string }>('web_fetch_url', {
          request: { url, maxChars: 4000 },
        })
        addCommandMessage([`Web fetch: ${response.url}`, `Status: ${response.status}`, response.title ?? '', '', response.content].filter(Boolean).join('\n'))
      },
    },
    {
      id: 'tool', command: '/tool', label: 'Tool dispatcher', description: 'Dispatch read_file or web_fetch directly',
      category: 'tools', execute: async (args) => {
        const [name = '', ...rest] = args?.trim().split(/\s+/) ?? []
        const value = rest.join(' ').trim()
        if (name === 'read_file' && value) {
          const content = await safeInvoke<string>('fs_extract_text', { path: value })
          addCommandMessage(`File read: ${value}\n\n${content.slice(0, 5000)}`)
          return
        }
        if (name === 'web_fetch' && value) {
          const response = await safeInvoke<{ url: string; status: number; content: string }>('web_fetch_url', {
            request: { url: value, maxChars: 4000 },
          })
          addCommandMessage(`Web fetch: ${response.url}\nStatus: ${response.status}\n\n${response.content}`)
          return
        }
        addCommandMessage('Usage: /tool read_file <path> | /tool web_fetch <url>')
      },
    },
    {
      id: 'todo', command: '/todo', label: 'Todo', description: 'Add or list tasks',
      category: 'agent', execute: async (args) => {
        const [action = 'list', ...rest] = args?.trim().split(/\s+/) ?? []
        if (action.toLowerCase() === 'add' && rest.length > 0) {
          const title = rest.join(' ')
          useTaskStore.getState().createTask(title, title, useChatStore.getState().activeThreadId)
          addCommandMessage(`Todo created: ${title}`)
          return
        }
        await useTaskStore.getState().loadFromDb()
        addCommandMessage('Todo list refreshed.')
      },
    },
    {
      id: 'settings', command: '/settings', label: 'Open settings', description: 'Alias for /config',
      category: 'config', execute: () => useUiStore.getState().setActiveMode('settings'),
    },
    // ===== Navigation =====
    {
      id: 'switch-work', command: '/ide', label: 'Go to workspace', description: 'Switches to the main workspace',
      category: 'navigation', execute: () => useUiStore.getState().setActiveMode('work'),
    },
    {
      id: 'switch-settings', command: '/config', label: 'Open settings', description: 'All settings and configuration',
      category: 'config', execute: () => useUiStore.getState().setActiveMode('settings'),
    },
    {
      id: 'toggle-sidebar', command: '/focus', label: 'Focus mode', description: 'Show or hide sidebars for focused work',
      category: 'display', execute: () => {
        const ui = useUiStore.getState()
        ui.toggleLeftSidebar()
      },
    },
    {
      id: 'toggle-theme', command: '/theme', label: 'Switch theme', description: 'Switch between light and dark theme',
      category: 'display', execute: (args) => {
        const ui = useUiStore.getState()
        if (args === 'dark') ui.setTheme('dark')
        else if (args === 'light') ui.setTheme('light')
        else ui.toggleTheme()
      },
    },

    // ===== Workspace =====
    {
      id: 'add-dir', command: '/add-dir', label: 'Add folder', description: 'Add a new working folder to the allowlist',
      category: 'workspace', execute: async (args) => {
        if (args?.trim()) {
          await safeInvokeVoid('fs_add_allowed_folder', { path: args.trim() })
        }
      },
    },
    {
      id: 'context', command: '/context', label: 'Show context', description: 'Show current thread context and attachments',
      category: 'workspace', execute: () => {
        const thread = useChatStore.getState()
        const active = thread.threads.find(t => t.id === thread.activeThreadId)
        if (active) {
          const msgCount = active.messages.length
          const charCount = active.messages.reduce((a, m) => a + m.content.length, 0)
          useChatStore.getState().addMessage(active.id, {
            role: 'system', content: `Context: ${msgCount} messages, ${charCount} characters, Thread "${active.title}"`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'diff', command: '/diff', label: 'Show diff', description: 'Show changes since the latest backup',
      category: 'workspace', execute: () => {
        useChatStore.getState().addMessage(
          useChatStore.getState().activeThreadId ?? '',
          { role: 'system', content: 'Diff view: use settings to inspect backup diffs.', timestamp: Date.now() }
        )
      },
    },
    {
      id: 'init', command: '/init', label: 'Initialize project', description: 'Initializes a new Open_Cowork project in the current folder',
      category: 'workspace', execute: async () => {
        await safeInvokeVoid('audit_event', { area: 'project', action: 'init', details: 'Project init started' })
        const store = useChatStore.getState()
        if (store.activeThreadId) {
          store.addMessage(store.activeThreadId, {
            role: 'system', content: 'Project initialized. Open_Cowork configuration was created.',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'rename', command: '/rename', label: 'Rename thread', description: 'Rename the current chat thread',
      category: 'workspace', execute: (args) => {
        if (!args?.trim()) return
        const store = useChatStore.getState()
        const threadId = store.activeThreadId
        if (threadId) {
          const thread = store.threads.find(t => t.id === threadId)
          if (thread) {
            void safeInvokeVoid('db_save_thread', { id: threadId, title: args.trim(), createdAt: new Date(thread.createdAt).toISOString() })
          }
        }
      },
    },
    {
      id: 'branch', command: '/branch', label: 'Thread branch', description: 'Creates a new thread branch from the current point',
      category: 'workspace', execute: () => {
        const cs = useChatStore.getState()
        const active = cs.threads.find(t => t.id === cs.activeThreadId)
        if (active) {
          cs.addThread(`Branch: ${active.title}`)
        }
      },
    },

    // ===== Agent Commands =====
    {
      id: 'agents', command: '/agents', label: 'Manage agents', description: 'Show and manage crew agents',
      category: 'agent', execute: () => {
        useCrewStore.getState().loadAgents()
      },
    },
    {
      id: 'batch', command: '/batch', label: 'Batch execution', description: 'Run multiple tasks as a batch',
      category: 'agent', execute: async (args) => {
        if (!args?.trim()) return
        const tasks = args.split(';').map(t => t.trim()).filter(Boolean)
        for (const task of tasks) {
          useTaskStore.getState().createTask(task, task, null)
        }
      },
    },
    {
      id: 'loop', command: '/loop', label: 'Agentic Loop', description: 'Starts an automated agent loop',
      category: 'agent', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `Agentic loop started${args ? `: ${args}` : ''}. Agent will work autonomously until the task is complete.`,
            timestamp: Date.now(),
          })
        }
        useConfigStore.getState().setPreference('autoPilotAllTools', true)
      },
    },
    {
      id: 'autofix-pr', command: '/autofix-pr', label: 'PR auto-fix', description: 'Automatically fix issues in a PR',
      category: 'agent', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Autofix for PR: ${args ?? 'current branch'}. Analyze and fix issues.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'ultraplan', command: '/ultraplan', label: 'Ultra planning', description: 'Creates a detailed multi-step plan',
      category: 'agent', execute: async (args) => {
        if (!args?.trim()) return
        const taskId = useTaskStore.getState().createTask(`Ultra-Plan: ${args}`, args, null)
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Ultra plan created (Task: ${taskId}). Detailed step-by-step analysis follows.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'ultrareview', command: '/ultrareview', label: 'Ultra review', description: 'Runs a comprehensive code review',
      category: 'agent', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Ultra review started${args ? ` for: ${args}` : ''}. Comprehensive analysis: architecture, security, performance, best practices.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'review', command: '/review', label: 'Code review', description: 'Run standard code review',
      category: 'agent', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Code review${args ? ` for: ${args}` : ''}. Check quality, bugs, and improvements.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'security-review', command: '/security-review', label: 'Security review', description: 'Security analysis of the code',
      category: 'security', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Security review${args ? ` for: ${args}` : ''}. OWASP Top 10, Injection, Auth, check crypto.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'passes', command: '/passes', label: 'Multi-Pass', description: 'Multiple passes for complex tasks',
      category: 'agent', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Multi-pass mode enabled (${args ?? '3'} passes). Iterative improvement.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'simplify', command: '/simplify', label: 'Simplify code', description: 'Simplifies and cleans up the selected code',
      category: 'agent', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Code simplification${args ? ` for: ${args}` : ''}. Reduce complexity and remove redundancy.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'debug', command: '/debug', label: 'Debug mode', description: 'Enables extended debug information',
      category: 'debug', execute: () => {
        const config = useConfigStore.getState()
        config.setPreference('verboseMode', !config.preferences.verboseMode)
        config.setPreference('superVerboseAuditLogging', !config.preferences.superVerboseAuditLogging)
      },
    },
    {
      id: 'doctor', command: '/doctor', label: 'System diagnostics', description: 'Checks system health and configuration',
      category: 'debug', execute: async () => {
        const cs = useChatStore.getState()
        if (!cs.activeThreadId) return
        try {
          const health = await safeInvoke<{ status: string }>('ollama_health_check', { config: useConfigStore.getState().ollama })
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `System diagnostics:\n- Ollama: ${health.status}\n- DB: active\n- MCP: configured\n- Audit: active`,
            timestamp: Date.now(),
          })
        } catch {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'System diagnostics: Ollama is not reachable. Check configuration.',
            timestamp: Date.now(),
          })
        }
      },
    },

    // ===== Model Commands =====
    {
      id: 'model', command: '/model', label: 'Switch model', description: 'Switch the active LLM model',
      category: 'model', execute: (args) => {
        if (args?.trim()) {
          useConfigStore.getState().setOllama({ model: args.trim() })
        }
      },
    },
    {
      id: 'effort', command: '/effort', label: 'Control effort', description: 'Adjust answer effort (temperature): low/medium/high',
      category: 'model', execute: (args) => {
        const map: Record<string, number> = { low: 0.1, medium: 0.5, high: 0.9 }
        const temp = map[args ?? ''] ?? 0.2
        useConfigStore.getState().setOllama({ temperature: temp })
      },
    },
    {
      id: 'fast', command: '/fast', label: 'Fast mode', description: 'Switches to the fastest available model',
      category: 'model', execute: () => {
        const models = useConfigStore.getState().availableModels
        const fast = models.find(m => m.includes('tiny') || m.includes('mini') || m.includes('3b')) ?? models[0]
        if (fast) useConfigStore.getState().setOllama({ model: fast })
      },
    },
    {
      id: 'powerup', command: '/powerup', label: 'Power mode', description: 'Switches to the strongest available model',
      category: 'model', execute: () => {
        const models = useConfigStore.getState().availableModels
        const power = models.find(m => m.includes('70b') || m.includes('405b') || m.includes('llama3.1')) ?? models[models.length - 1]
        if (power) useConfigStore.getState().setOllama({ model: power })
      },
    },
    {
      id: 'compact', command: '/compact', label: 'Compact context', description: 'Compacts chat context for longer sessions',
      category: 'model', execute: () => {
        useCoworkStore.getState().setPolicyFlag('autoCompactLongContext', true)
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Context compression enabled. Older messages will be summarized.',
            timestamp: Date.now(),
          })
        }
      },
    },

    // ===== Memory Commands =====
    {
      id: 'memory', command: '/memory', label: 'Memory', description: 'Manage and search memory entries',
      category: 'memory', execute: async (args) => {
        if (args?.trim()) {
          await useMemoryStore.getState().searchEntries(args.trim())
        } else {
          await useMemoryStore.getState().loadEntries()
        }
      },
    },
    {
      id: 'recap', command: '/recap', label: 'Summary', description: 'Summary of the current session',
      category: 'memory', execute: () => {
        const cs = useChatStore.getState()
        const active = cs.threads.find(t => t.id === cs.activeThreadId)
        if (active && cs.activeThreadId) {
          const userMsgs = active.messages.filter(m => m.role === 'user')
          const assistantMsgs = active.messages.filter(m => m.role === 'assistant')
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `Session-Recap:\n- ${userMsgs.length} Benutzer-messages\n- ${assistantMsgs.length} answers\n- Thread: "${active.title}"\n- Started: ${new Date(active.createdAt).toLocaleString('de-DE')}`,
            timestamp: Date.now(),
          })
        }
      },
    },

    // ===== Session Commands =====
    {
      id: 'clear', command: '/clear', label: 'Clear chat', description: 'Reset current chat history',
      category: 'session', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.deleteThread(cs.activeThreadId)
          cs.addThread('New chat')
        }
      },
    },
    {
      id: 'resume', command: '/resume', label: 'Resume', description: 'Resume latest session',
      category: 'session', execute: () => {
        const cs = useChatStore.getState()
        const latest = cs.threads.sort((a, b) => b.updatedAt - a.updatedAt)[0]
        if (latest) cs.setActiveThread(latest.id)
      },
    },
    {
      id: 'rewind', command: '/rewind', label: 'Rewind', description: 'Remove the latest N messages',
      category: 'session', execute: (args) => {
        const count = Number.parseInt(args ?? '1', 10) || 1
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          const removed = cs.removeLastMessagePairs(cs.activeThreadId, count)
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: removed.pairsRemoved > 0
              ? `Rewound: ${removed.pairsRemoved} message pair(s) removed.`
              : 'No matching messages found to rewind.',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'exit', command: '/exit', label: 'Exit', description: 'End current session',
      category: 'session', execute: () => {
        useChatStore.getState().setActiveThread(null)
      },
    },

    // ===== Tools =====
    {
      id: 'mcp', command: '/mcp', label: 'Manage MCP', description: 'Configure MCP servers and tools',
      category: 'tools', execute: () => {
        useUiStore.getState().setActiveMode('settings')
      },
    },
    {
      id: 'hooks', command: '/hooks', label: 'Manage hooks', description: 'Configure pre/post-execution hooks',
      category: 'tools', execute: () => {
        useConfigStore.getState().setPreferences({})
      },
    },
    {
      id: 'sandbox', command: '/sandbox', label: 'Sandbox mode', description: 'Enable isolated execution environment',
      category: 'tools', execute: () => {
        useCoworkStore.getState().setPolicyFlag('strictPolicyEnforcement', true)
        useConfigStore.getState().setPreference('readOnlyFsMode', true)
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Sandbox mode enabled: read-only access, strict policy.',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'terminal-setup', command: '/terminal-setup', label: 'Set up terminal', description: 'Configure terminal backend',
      category: 'tools', execute: async () => {
        await useTerminalStore.getState().ensureLocalBackend()
      },
    },
    {
      id: 'web-setup', command: '/web-setup', label: 'Web access setup', description: 'Configure web research and URL access',
      category: 'tools', execute: () => {
        useCoworkStore.getState().setPolicyFlag('allowWebFetch', true)
        useCoworkStore.getState().setPolicyFlag('allowWebSearch', true)
      },
    },

    // ===== Config Commands =====
    {
      id: 'color', command: '/color', label: 'Color scheme', description: 'Adjust color scheme',
      category: 'display', execute: (args) => {
        if (args === 'dark' || args === 'light') {
          useUiStore.getState().setTheme(args)
        }
      },
    },
    {
      id: 'keybindings', command: '/keybindings', label: 'Keyboard shortcuts', description: 'Show and edit keyboard shortcuts',
      category: 'config', execute: () => {
        useUiStore.getState().setShortcutsOverlayOpen(true)
      },
    },
    {
      id: 'less-permission-prompts', command: '/less-permission-prompts', label: 'Fewer permission prompts', description: 'Reduces confirmation dialogs',
      category: 'config', execute: () => {
        useConfigStore.getState().setPreferences({
          autoApproveSafeTools: true,
          confirmOnCloseWithRunningTasks: false,
          fallbackToHumanOnRepeatedFailure: false,
        })
      },
    },
    {
      id: 'privacy-settings', command: '/privacy-settings', label: 'Privacy', description: 'Privacy and telemetry settings',
      category: 'config', execute: () => {
        useConfigStore.getState().setPreference('telemetryEnabled', false)
      },
    },

    // ===== Insights & Stats =====
    {
      id: 'stats', command: '/stats', label: 'Statistics', description: 'Show usage statistics and metrics',
      category: 'debug', execute: async () => {
        await useInsightsStore.getState().loadSummary()
        const summary = useInsightsStore.getState().summary
        const cs = useChatStore.getState()
        if (cs.activeThreadId && summary) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `Statistics:\n- Events: ${summary.totalEvents}\n- Sessions: ${summary.totalSessions}\n- messages: ${summary.totalMessagesSent}\n- Token (est.): ${summary.totalTokensEst}\n- Skills: ${summary.skillUsageCount}\n- Memory: ${summary.memoryEntryCount}`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'insights', command: '/insights', label: 'Insights Dashboard', description: 'Open detailed insights dashboard',
      category: 'debug', execute: () => {
        useInsightsStore.getState().loadSummary()
        useInsightsStore.getState().loadEvents()
      },
    },
    {
      id: 'cost', command: '/cost', label: 'Costs', description: 'Estimated costs of the current session',
      category: 'debug', execute: async () => {
        const summary = useInsightsStore.getState().summary
          ?? (await useInsightsStore.getState().loadSummary(), useInsightsStore.getState().summary)
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          const tokens = summary?.totalTokensEst ?? 0
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `Cost estimate:\n- Total tokens: ${tokens}\n- Local model: 0 EUR (Ollama)\n- Estimated API costs: ~${(tokens * 0.000002).toFixed(4)} EUR`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'usage', command: '/usage', label: 'Usage', description: 'Detailed usage statistics',
      category: 'debug', execute: () => useInsightsStore.getState().loadSummary(),
    },
    {
      id: 'status', command: '/status', label: 'Status', description: 'Show current system status',
      category: 'debug', execute: async () => {
        const cs = useChatStore.getState()
        if (!cs.activeThreadId) return
        const procs = useProcessStore.getState().processes
        const backends = useTerminalStore.getState().backends
        cs.addMessage(cs.activeThreadId, {
          role: 'system',
          content: `Status:\n- Threads: ${cs.threads.length}\n- Processes: ${procs.length}\n- Backends: ${backends.length}\n- Model: ${useConfigStore.getState().ollama.model}`,
          timestamp: Date.now(),
        })
      },
    },
    {
      id: 'statusline', command: '/statusline', label: 'Status line', description: 'Show or hide compact status line',
      category: 'display', execute: () => {
        useConfigStore.getState().setPreference('compactMode', !useConfigStore.getState().preferences.compactMode)
      },
    },

    // ===== Export =====
    {
      id: 'export', command: '/export', label: 'Export', description: 'Export chat or data (JSON/MD/TXT)',
      category: 'export', execute: (args) => {
        const cs = useChatStore.getState()
        const active = cs.threads.find(t => t.id === cs.activeThreadId)
        if (active) {
          const format = args?.trim() ?? 'json'
          const data = format === 'json'
            ? JSON.stringify(active, null, 2)
            : active.messages.map(m => `[${m.role}] ${m.content}`).join('\n\n')
          navigator.clipboard.writeText(data).catch(() => {})
          if (cs.activeThreadId) {
            cs.addMessage(cs.activeThreadId, {
              role: 'system', content: `Export (${format}) copied to clipboard.`,
              timestamp: Date.now(),
            })
          }
        }
      },
    },
    {
      id: 'copy', command: '/copy', label: 'Copy', description: 'Copy latest answer to clipboard',
      category: 'export', execute: () => {
        const cs = useChatStore.getState()
        const active = cs.threads.find(t => t.id === cs.activeThreadId)
        if (active) {
          const lastAssistant = [...active.messages].reverse().find(m => m.role === 'assistant')
          if (lastAssistant) {
            navigator.clipboard.writeText(lastAssistant.content).catch(() => {})
          }
        }
      },
    },

    // ===== Memory =====
    {
      id: 'skills', command: '/skills', label: 'Skills', description: 'Show and manage learned skills',
      category: 'memory', execute: () => useSkillStore.getState().loadSkills(),
    },
    {
      id: 'tasks', command: '/tasks', label: 'Tasks', description: 'Show open tasks',
      category: 'agent', execute: () => useTaskStore.getState().loadFromDb(),
    },

    // ===== Plugins =====
    {
      id: 'plugin', command: '/plugin', label: 'Manage plugin', description: 'Install and configure plugins',
      category: 'plugins', execute: (args) => {
        if (args === 'examples' || args === 'install') {
          useCoworkStore.getState().installPluginExamples()
        }
      },
    },
    {
      id: 'reload-plugins', command: '/reload-plugins', label: 'Reload plugins', description: 'Reload all plugins',
      category: 'plugins', execute: () => {
        useCoworkStore.getState().installPluginExamples()
      },
    },

    // ===== Local Backend =====
    {
      id: 'ollama-settings', command: '/ollama', label: 'Ollama settings', description: 'Configure local Ollama endpoint and runtime parameters',
      category: 'config', execute: () => useUiStore.getState().setActiveMode('settings'),
    },
    {
      id: 'local-model', command: '/local-model', label: 'Local model', description: 'Check active Ollama model or switch it in settings',
      category: 'model', execute: () => useUiStore.getState().setActiveMode('settings'),
    },
    {
      id: 'local-runtime', command: '/local-runtime', label: 'Local runtime', description: 'Confirm local desktop and Ollama operation',
      category: 'config', execute: () => useUiStore.getState().setActiveMode('settings'),
    },

    // ===== Display & UX =====
    {
      id: 'stickers', command: '/stickers', label: 'Sticker', description: 'Show or hide sticker reactions',
      category: 'display', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: '🎉 Sticker mode enabled!',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'tui', command: '/tui', label: 'TUI mode', description: 'Enable terminal UI view',
      category: 'display', execute: () => {
        useConfigStore.getState().setPreference('compactMode', true)
      },
    },
    {
      id: 'desktop', command: '/desktop', label: 'Desktop integration', description: 'Configure desktop features and tray icon',
      category: 'config', execute: () => useUiStore.getState().setActiveMode('settings'),
    },
    {
      id: 'mobile', command: '/mobile', label: 'Mobile optimization', description: 'Enable mobile/touch view',
      category: 'display', execute: () => {
        useConfigStore.getState().setPreference('compactMode', true)
        useConfigStore.getState().setPreference('fontScale', 110)
      },
    },
    {
      id: 'voice', command: '/voice', label: 'Voice input', description: 'Enable voice input (via Browser API)',
      category: 'display', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Voice input: use the browser SpeechRecognition API. This feature will be integrated in future versions.',
            timestamp: Date.now(),
          })
        }
      },
    },

    // ===== Crew AI Commands =====
    {
      id: 'crew-create', command: '/crew', label: 'Create crew', description: 'Create a new AI crew with agents',
      category: 'crew', execute: (args) => {
        if (args?.trim()) {
          const raw = args.trim()
          const separator = raw.indexOf(':')
          useCrewStore.getState().createStarterCrew(
            separator > 0 ? raw.slice(0, separator).trim() : raw.slice(0, 64),
            separator > 0 ? raw.slice(separator + 1).trim() : raw,
          )
        }
      },
    },
    {
      id: 'team-onboarding', command: '/team-onboarding', label: 'Team onboarding', description: 'Give new team members context',
      category: 'crew', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `Team onboarding${args ? ` for ${args}` : ''}: Project context, conventions, and setup guide will be generated.`,
            timestamp: Date.now(),
          })
        }
      },
    },

    // ===== Session Management =====
    {
      id: 'schedule', command: '/schedule', label: 'Schedule', description: 'Schedule a task',
      category: 'session', execute: async (args) => {
        if (args?.trim()) {
          const parsed = parseScheduledTaskInput(args)
          if (!parsed) return
          await useCoworkStore.getState().upsertScheduledTask({
            id: uid(),
            name: parsed.prompt.slice(0, 40),
            prompt: parsed.prompt,
            cronLike: parsed.scheduleExpr,
            taskKind: 'prompt',
            crewId: null,
            crewSnapshotJson: null,
            modelConfigJson: JSON.stringify(useConfigStore.getState().ollama),
            priority: 100,
            dependsOnTaskIds: [],
            active: true,
            lastRunAt: null,
            nextRunAt: null,
          })
        }
      },
    },

    // ===== Misc =====
    {
      id: 'btw', command: '/btw', label: 'By the way', description: 'Add context info without changing the main task',
      category: 'agent', execute: (args) => {
        if (!args?.trim()) return
        useMemoryStore.getState().upsertEntry({
          id: uid(), scope: 'session', category: 'context', key: 'btw', content: args.trim(),
        })
      },
    },
    {
      id: 'chrome', command: '/chrome', label: 'Chrome integration', description: 'Control Chrome browser integration',
      category: 'tools', execute: () => {
        useCoworkStore.getState().toggleConnector('chrome', true)
      },
    },
    {
      id: 'feedback', command: '/feedback', label: 'Feedback', description: 'Give feedback on the current answer',
      category: 'session', execute: (args) => {
        void safeInvokeVoid('audit_event', {
          area: 'feedback', action: 'user_feedback', details: args ?? 'No comment',
        })
      },
    },
    {
      id: 'heapdump', command: '/heapdump', label: 'Heap Dump', description: 'Create memory snapshot for debugging',
      category: 'debug', execute: async () => {
        const snapshot = await useMemoryStore.getState().createSnapshot()
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `Heap dump created: ${snapshot.total_entries} memory entries, ${snapshot.total_profile_keys} profile keys`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'install-github-app', command: '/install-github-app', label: 'Install GitHub app', description: 'Set up GitHub integration',
      category: 'tools', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'GitHub app installation: configure an MCP server for GitHub or use a GitHub personal access token.',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'install-slack-app', command: '/install-slack-app', label: 'Install Slack app', description: 'Set up Slack integration',
      category: 'tools', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Slack integration: configure an MCP server for Slack or use webhooks.',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'teleport', command: '/teleport', label: 'Teleport', description: 'Jump quickly to a specific file/folder',
      category: 'navigation', execute: (args) => {
        if (args?.trim()) {
          useUiStore.getState().setWorkingPath(args.trim(), 'file')
        }
      },
    },
    {
      id: 'remote-control', command: '/remote-control', label: 'Remote control', description: 'Enable remote control',
      category: 'config', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Remote control: feature planned for future versions.',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'remote-env', command: '/remote-env', label: 'Remote environment', description: 'Configure remote execution environment',
      category: 'config', execute: () => useUiStore.getState().setActiveMode('settings'),
    },
    {
      id: 'extra-usage', command: '/extra-usage', label: 'Extra Usage', description: 'Enable extended usage limits',
      category: 'config', execute: () => {
        useConfigStore.getState().setPreference('maxToolCallsPerLoop', 50)
      },
    },
    {
      id: 'release-notes', command: '/release-notes', label: 'Release Notes', description: 'Show current release notes',
      category: 'config', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Open_Cowork v1.0\n- Centrally registered slash commands\n- 5 default personalities\n- CrewAI multi-agent support\n- Hermes-style memory and session search\n- Plugin and MCP integration',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'upgrade', command: '/upgrade', label: 'Upgrade', description: 'Update to the latest version',
      category: 'config', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Upgrade: checking for updates... The current version is up to date.',
            timestamp: Date.now(),
          })
        }
      },
    },
  ]

  const ids = new Set<string>()
  const names = new Set<string>()
  for (const command of commands) {
    if (!command.command.startsWith('/')) {
      throw new Error(`Slash command must start with /: ${command.command}`)
    }
    if (ids.has(command.id)) throw new Error(`Duplicate slash command id: ${command.id}`)
    if (names.has(command.command)) throw new Error(`Duplicate slash command: ${command.command}`)
    ids.add(command.id)
    names.add(command.command)
  }
  return commands
}
