import { describe, expect, it, vi } from 'vitest'
import { buildAllCommands, getSlashCommandSuggestions, useCommandRegistry, type SlashCommand } from './commandRegistryStore'
import { SLASH_COMMAND_DEFINITIONS } from '../utils/claudeBridge'

vi.mock('./uiStore', () => ({
  useUiStore: {
    getState: () => ({
      setActiveMode: vi.fn(),
      toggleLeftSidebar: vi.fn(),
      setTheme: vi.fn(),
      toggleTheme: vi.fn(),
      setShortcutsOverlayOpen: vi.fn(),
      setWorkingPath: vi.fn(),
    }),
  },
}))

vi.mock('./configStore', () => ({
  useConfigStore: {
    getState: () => ({
      preferences: {
        verboseMode: false,
        superVerboseAuditLogging: false,
        compactMode: false,
      },
      availableModels: [],
      ollama: { model: 'test-model' },
      setPreference: vi.fn(),
      setPreferences: vi.fn(),
      setOllama: vi.fn(),
    }),
  },
}))

vi.mock('./chatStore', () => ({
  useChatStore: {
    getState: () => ({
      activeThreadId: 'thread-1',
      threads: [],
      addMessage: vi.fn(),
      addThread: vi.fn(),
      deleteThread: vi.fn(),
      setActiveThread: vi.fn(),
      removeLastMessagePairs: vi.fn(() => ({ pairsRemoved: 0 })),
    }),
  },
}))

vi.mock('./coworkStore', () => ({
  useCoworkStore: {
    getState: () => ({
      setPolicyFlag: vi.fn(),
      toggleConnector: vi.fn(),
      installPluginExamples: vi.fn(),
      upsertScheduledTask: vi.fn(),
      enabledClaudeToolIds: ['Read', 'MemoryWrite', 'SessionSearch'],
      claudePermissionMode: 'default',
      setClaudePlanMode: vi.fn(),
      setClaudePermissionMode: vi.fn(),
    }),
  },
}))

vi.mock('./memoryStore', () => ({
  useMemoryStore: {
    getState: () => ({
      searchEntries: vi.fn(),
      loadEntries: vi.fn(),
      upsertEntry: vi.fn(),
      createSnapshot: vi.fn(async () => ({ total_entries: 0, total_profile_keys: 0 })),
    }),
  },
}))

vi.mock('./skillStore', () => ({
  useSkillStore: {
    getState: () => ({
      loadSkills: vi.fn(),
    }),
  },
}))

vi.mock('./insightsStore', () => ({
  useInsightsStore: {
    getState: () => ({
      summary: null,
      loadSummary: vi.fn(),
      loadEvents: vi.fn(),
    }),
  },
}))

vi.mock('./processStore', () => ({
  useProcessStore: {
    getState: () => ({
      processes: [],
    }),
  },
}))

vi.mock('./terminalStore', () => ({
  useTerminalStore: {
    getState: () => ({
      backends: [],
      ensureLocalBackend: vi.fn(),
    }),
  },
}))

vi.mock('./taskStore', () => ({
  useTaskStore: {
    getState: () => ({
      createTask: vi.fn(),
      loadFromDb: vi.fn(),
    }),
  },
}))

vi.mock('./crewStore', () => ({
  useCrewStore: {
    getState: () => ({
      loadAgents: vi.fn(),
      createStarterCrew: vi.fn(),
    }),
  },
}))

vi.mock('../utils/safeInvoke', () => ({
  safeInvoke: vi.fn(),
  safeInvokeVoid: vi.fn(),
}))

const commands: SlashCommand[] = [
  {
    id: 'model',
    command: '/model',
    label: 'Model',
    description: 'Model wechseln',
    category: 'model',
    execute: vi.fn(),
  },
  {
    id: 'memory',
    command: '/memory',
    label: 'Memory',
    description: 'Memory open',
    category: 'memory',
    execute: vi.fn(),
  },
  {
    id: 'clear',
    command: '/clear',
    label: 'Clear',
    description: 'Clear chat',
    category: 'session',
    execute: vi.fn(),
  },
]

describe('getSlashCommandSuggestions', () => {
  it('returns all slash commands when only / is entered', () => {
    expect(getSlashCommandSuggestions(commands, '/')).toEqual(commands)
  })

  it('filters by the typed command token', () => {
    expect(getSlashCommandSuggestions(commands, '/me')).toEqual([commands[1]])
    expect(getSlashCommandSuggestions(commands, '/model llama')).toEqual([commands[0]])
  })

  it('returns no suggestions for non-command input', () => {
    expect(getSlashCommandSuggestions(commands, 'hello')).toEqual([])
  })
})

describe('slash command registry integrity', () => {
  it('contains unique valid commands and every model-facing Claude bridge command', () => {
    const registered = buildAllCommands()
    const ids = registered.map((command) => command.id)
    const names = registered.map((command) => command.command)

    expect(new Set(ids).size).toBe(ids.length)
    expect(new Set(names).size).toBe(names.length)
    expect(names.every((command) => command.startsWith('/'))).toBe(true)
    for (const definition of SLASH_COMMAND_DEFINITIONS) {
      expect(names).toContain(definition.command)
    }
  })

  it('can invoke every registered command without arguments without throwing', async () => {
    for (const command of buildAllCommands()) {
      try {
        await command.execute()
      } catch (error) {
        throw new Error(`${command.command} threw during smoke execution: ${String(error)}`)
      }
    }
  })

  it('awaits async execution and does not record failed commands as successful', async () => {
    const original = useCommandRegistry.getState().commands
    let release = () => {}
    const barrier = new Promise<void>((resolve) => { release = resolve })
    useCommandRegistry.setState({
      commands: [{
        id: 'async-test',
        command: '/async-test',
        label: 'Async test',
        description: 'Test command',
        category: 'debug',
        execute: () => barrier,
      }],
      lastExecuted: null,
      executionLog: [],
    })

    const execution = useCommandRegistry.getState().executeCommand('/async-test')
    expect(useCommandRegistry.getState().lastExecuted).toBeNull()
    release()
    await expect(execution).resolves.toBe(true)
    expect(useCommandRegistry.getState().lastExecuted).toBe('async-test')

    useCommandRegistry.setState({
      commands: [{
        id: 'fail-test',
        command: '/fail-test',
        label: 'Fail test',
        description: 'Test failure',
        category: 'debug',
        execute: async () => { throw new Error('expected failure') },
      }],
      lastExecuted: null,
      executionLog: [],
    })
    await expect(useCommandRegistry.getState().executeCommand('/fail-test')).rejects.toThrow('expected failure')
    expect(useCommandRegistry.getState().lastExecuted).toBeNull()
    expect(useCommandRegistry.getState().executionLog).toEqual([])
    useCommandRegistry.setState({ commands: original })
  })
})
