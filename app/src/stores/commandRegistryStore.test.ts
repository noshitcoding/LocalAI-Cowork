import { describe, expect, it, vi } from 'vitest'
import { getSlashCommandSuggestions, type SlashCommand } from './commandRegistryStore'

vi.mock('./uiStore', () => ({
  useUiStore: {
    getState: () => ({
      setActiveMode: vi.fn(),
      toggleLeftSidebar: vi.fn(),
      setTheme: vi.fn(),
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
      createCrew: vi.fn(),
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
