import { render, screen, fireEvent, within, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import SettingsView from './SettingsView'
import { useConfigStore } from '../stores/configStore'
import { useEngineStore } from '../stores/engineStore'
import i18n from '../i18n'

const invokeMock = vi.fn()
const checkOllamaStatusMock = vi.fn()
const fetchOllamaModelsMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

/* Default invoke handler: return safe defaults for all known commands */
function defaultInvoke(cmd: string) {
  switch (cmd) {
    case 'personality_list': return Promise.resolve([])
    case 'memory_search': return Promise.resolve([])
    case 'memory_hints': return Promise.resolve([])
    case 'user_profile_list': return Promise.resolve([])
    case 'memory_provider_list': return Promise.resolve([])
    case 'skill_list': return Promise.resolve([])
    case 'learning_list': return Promise.resolve([])
    case 'pipeline_list': return Promise.resolve([])
    case 'tool_gateway_list': return Promise.resolve([])
    case 'session_list': return Promise.resolve([])
    case 'session_search': return Promise.resolve([])
    case 'insights_list': return Promise.resolve([])
    case 'insights_summary': return Promise.resolve({ totalSessions: 0, totalEvents: 0 })
    case 'backend_list': return Promise.resolve([])
    case 'backend_ensure_local': return Promise.resolve(null)
    case 'process_list': return Promise.resolve([])
    case 'mcp_list_servers': return Promise.resolve([])
    case 'mcp_probe': return Promise.resolve({ tools: [] })
    default: return Promise.resolve(null)
  }
}

/* Reset stores before each test */
function resetConfigStore() {
  useConfigStore.setState({
    ollama: {
      baseUrl: 'http://localhost:11434',
      model: 'llama3.1:8b',
      timeoutMs: 200000,
      contextWindow: 128000,
      temperature: 0.1,
    },
    llmProfiles: [
      {
        id: 'default-ollama',
        name: 'Lokales Ollama',
        provider: 'ollama',
        baseUrl: 'http://localhost:11434',
        model: 'llama3.1:8b',
        apiKey: '',
        timeoutMs: 200000,
        verifyTlsCertificates: true,
        contextWindow: 128000,
        temperature: 0.1,
      },
      {
        id: 'default-openai-compatible',
        name: 'OpenAI-compatible',
        provider: 'openai-compatible',
        baseUrl: 'https://api.openai.com/v1',
        model: 'gpt-4.1-mini',
        apiKey: '',
        timeoutMs: 600000,
        verifyTlsCertificates: true,
        contextWindow: null,
        temperature: null,
      },
      {
        id: 'default-openrouter',
        name: 'OpenRouter',
        provider: 'openrouter',
        baseUrl: 'https://openrouter.ai/api/v1',
        model: '',
        apiKey: '',
        timeoutMs: 600000,
        verifyTlsCertificates: true,
        contextWindow: null,
        temperature: null,
      },
    ],
    defaultLlmProfileIds: {
      ollama: 'default-ollama',
      'openai-compatible': 'default-openai-compatible',
      openrouter: 'default-openrouter',
    },
    llmProfileModels: {
      'default-ollama': [],
      'default-openai-compatible': [],
      'default-openrouter': [],
    },
    preferences: {
      autoApproveSafeTools: true,
      autoPilotAllTools: false,
      readOnlyFsMode: false,
      commandWhitelist: '',
      commandBlacklist: '',
      maxToolCallsPerLoop: 12,
      fallbackToHumanOnRepeatedFailure: true,
      confirmOnCloseWithRunningTasks: true,
      telemetryEnabled: true,
      notificationsEnabled: true,
      soundsEnabled: false,
      launchAtStartup: false,
      showTimestamps: true,
      defaultStartView: 'last',
      focusMode: false,
      compactMode: false,
      verboseMode: false,
      limitThinkingWindow: true,
      superVerboseAuditLogging: false,
      fontScale: 100,
      shortcutOverlayEnabled: true,
      syncThemeWithSystem: false,
      chatRetentionDays: 30,
      autoBackupDb: true,
      dbBackupIntervalHours: 24,
      workspaceDefaultPath: '',
      mcpAutoReconnect: true,
      mcpVerboseLogging: false,
      mcpEnvEditorEnabled: true,
      mcpAllowManualImport: true,
      ollamaStreamAutosave: true,
      dbCleanupOnStart: false,
      taskBatchMultiSelectEnabled: true,
      terminalPersistenceMode: 'runtime',
    },
    availableModels: [],
    mcpServer: { name: '', command: '', args: '', env: {} },
    mcpServers: [],
    activeMcpServerName: '',
  })
}

function resetEngineStore() {
  checkOllamaStatusMock.mockReset()
  fetchOllamaModelsMock.mockReset()
  checkOllamaStatusMock.mockResolvedValue(true)
  fetchOllamaModelsMock.mockResolvedValue([
    { id: 'llama3.1:8b', name: 'llama3.1:8b', size: 1 },
  ])
  useEngineStore.setState({
    config: {
      ...useEngineStore.getState().config,
      maxTurns: 25,
      permissionMode: 'default',
      appendSystemPrompt: '',
      sessionPersistence: true,
    },
    contextWarning: { level: 'none', estimatedTokens: 0 },
    compactionCount: 0,
    currentSessionId: null,
    checkOllamaStatus: checkOllamaStatusMock,
    fetchOllamaModels: fetchOllamaModelsMock,
  })
}

describe('SettingsView', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en')
    delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
    invokeMock.mockReset()
    invokeMock.mockImplementation(defaultInvoke)
    resetConfigStore()
    resetEngineStore()
  })

  /* ── 1. sidebar renders all 9 categories ── */
  it('renders all 9 category buttons in sidebar', () => {
    render(<SettingsView />)
    const nav = screen.getByRole('navigation', { name: 'Settings categories' })
    const buttons = nav.querySelectorAll('.settings-nav-item')
    expect(buttons.length).toBe(9)
  })

  /* ── 2. default category is AI & model ── */
  it('shows AI & model content by default', () => {
    render(<SettingsView />)
    expect(screen.getByRole('heading', { level: 1, name: 'AI & model' })).toBeInTheDocument()
  })

  /* ── 3. navigation switches categories ── */
  it('switches to Agent & Skills when clicked', () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByText('Agent & Skills'))
    expect(screen.getByRole('heading', { level: 1, name: 'Agent & Skills' })).toBeInTheDocument()
  })

  /* ── 4. Interface category ── */
  it('switches to Interface category', () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByText('Interface'))
    expect(screen.getByRole('heading', { level: 1, name: 'Interface' })).toBeInTheDocument()
    expect(screen.getByText('Focus mode')).toBeInTheDocument()
    expect(screen.getByText('Compact mode')).toBeInTheDocument()
  })

  /* ── 5. security category ── */
  it('switches to Security & data category', () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByText('Security & data'))
    expect(screen.getByRole('heading', { level: 1, name: 'Security & data' })).toBeInTheDocument()
    expect(screen.getByText('Read-only mode')).toBeInTheDocument()
  })

  /* ── 6. System & Info shows runtime info ── */
  it('switches to System & Info and shows runtime info', () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByText('System & Info'))
    expect(screen.getByRole('heading', { level: 1, name: 'System & Info' })).toBeInTheDocument()
    expect(screen.getByText('Local LLM endpoint')).toBeInTheDocument()
    expect(screen.getByText('http://localhost:11434')).toBeInTheDocument()
    expect(screen.getByText('Default model')).toBeInTheDocument()
    expect(screen.getByText('llama3.1:8b')).toBeInTheDocument()
    expect(screen.getByText('Creator')).toBeInTheDocument()
    expect(screen.getByText('noshitcoding')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'github.com/noshitcoding/Open_Cowork' })).toHaveAttribute('href', 'https://github.com/noshitcoding/Open_Cowork')
    expect(screen.getByText('Disclaimer')).toBeInTheDocument()
    expect(screen.getByText(/Use it at your own risk/)).toBeInTheDocument()
  })

  /* ── 7. Memory category renders ── */
  it('switches to Memory category', () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByText('Memory'))
    expect(screen.getByRole('heading', { level: 1, name: 'Memory' })).toBeInTheDocument()
  })

  /* ── 8. Sessions & Insights category renders ── */
  it('switches to Sessions & Insights', () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByText('Sessions & Insights'))
    expect(screen.getByRole('heading', { level: 1, name: 'Sessions & Insights' })).toBeInTheDocument()
  })

  /* ── 9. Terminal & Processes category renders ── */
  it('switches to Terminal & Processes', () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByText('Terminal & Processes'))
    expect(screen.getByRole('heading', { level: 1, name: 'Terminal & Processes' })).toBeInTheDocument()
    expect(screen.getByText('Terminal dock')).toBeInTheDocument()
    expect(screen.getByRole('combobox', { name: 'Persistence' })).toHaveValue('runtime')
  })

  /* ── 10. MCP Server category renders ── */
  it('switches to MCP Server', () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByText('MCP Server'))
    // McpView also has an h1 "MCP Server", so check for the settings toggle instead
    expect(screen.getByText('Auto-reconnect')).toBeInTheDocument()
    expect(screen.getByText('Verbose logging')).toBeInTheDocument()
  })

  /* ── 11. Legacy Ollama config section removed ── */
  it('does not render the legacy Ollama configuration section', () => {
    render(<SettingsView />)
    expect(screen.queryByRole('heading', { level: 2, name: /Ollama configuration/ })).not.toBeInTheDocument()
  })

  it('does not render OpenAI Computer Use settings', () => {
    render(<SettingsView />)
    expect(screen.queryByRole('heading', { level: 2, name: /OpenAI Computer Use/ })).not.toBeInTheDocument()
    expect(screen.queryByText('Safety Checks automatisch bestaetigen')).not.toBeInTheDocument()
  })

  /* ── 12. Default Ollama profile endpoint updates store ── */
  it('updates default Ollama profile endpoint on input change', () => {
    render(<SettingsView />)
    const profileCard = screen.getByText('Lokales Ollama', { selector: 'strong' }).closest('.card') as HTMLElement
    const endpointInput = within(profileCard).getByLabelText('Endpoint')
    fireEvent.change(endpointInput, { target: { value: 'http://localhost:11434' } })
    expect(useConfigStore.getState().ollama.baseUrl).toBe('http://localhost:11434')
    expect(useConfigStore.getState().llmProfiles.find((profile) => profile.id === 'default-ollama')?.baseUrl).toBe('http://localhost:11434')
  })

  /* ── 13. Default Ollama profile model updates store ── */
  it('updates default Ollama profile model on input change', () => {
    useConfigStore.getState().setLlmProfileModels('default-ollama', ['llama3.1:8b', 'mistral:7b'])
    render(<SettingsView />)
    const profileCard = screen.getByText('Lokales Ollama', { selector: 'strong' }).closest('.card') as HTMLElement
    const modelControl = within(profileCard).getByLabelText('Model')
    expect(modelControl.tagName).toBe('SELECT')
    fireEvent.change(modelControl, { target: { value: 'mistral:7b' } })
    expect(useConfigStore.getState().ollama.model).toBe('mistral:7b')
    expect(useConfigStore.getState().llmProfiles.find((profile) => profile.id === 'default-ollama')?.model).toBe('mistral:7b')
  })

  /* ── 14. Toggle updates preference ── */
  it('toggles autoApproveSafeTools preference', () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByText('Agent & Skills'))
    const toggleBtn = screen.getByText('Automatically approve safe tools').closest('.toggle-row')!.querySelector('button[role="switch"]')!
    expect(toggleBtn.getAttribute('aria-checked')).toBe('true')
    fireEvent.click(toggleBtn)
    expect(useConfigStore.getState().preferences.autoApproveSafeTools).toBe(false)
  })

  /* ── 15. Model dropdown with Ollama profile models ── */
  it('renders model dropdown when Ollama profile models are set', () => {
    useConfigStore.getState().setLlmProfileModels('default-ollama', ['llama3.1:8b', 'mistral:7b', 'codellama:13b'])
    render(<SettingsView />)
    const profileCard = screen.getByText('Lokales Ollama', { selector: 'strong' }).closest('.card') as HTMLElement
    const modelControl = within(profileCard).getByLabelText('Model')
    expect(modelControl.tagName).toBe('SELECT')
  })

  it('uses exact external model id returned by the provider model list', async () => {
    ;(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {}
    useConfigStore.getState().updateLlmProfile('default-openai-compatible', {
      baseUrl: 'https://mlis.example.test/v1/models',
      apiKey: 'sk-test',
      model: 'Hy3-preview-nvfp4',
    })
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'crew_provider_models_list') {
        return Promise.resolve({
          endpoint: 'https://mlis.example.test/v1/models',
          models: ['0xSero/Hy3-preview-nvfp4'],
        })
      }
      return defaultInvoke(cmd)
    })

    render(<SettingsView />)
    const profileCards = screen.getAllByText('OpenAI-compatible', { selector: 'strong' })
    const profileCard = profileCards[1].closest('.card') as HTMLElement
    fireEvent.click(within(profileCard).getByRole('button', { name: 'Load models' }))

    await waitFor(() => {
      expect(useConfigStore.getState().llmProfiles.find((profile) => profile.id === 'default-openai-compatible')?.model)
        .toBe('0xSero/Hy3-preview-nvfp4')
    })
    expect(await within(profileCard).findByText('Model automatically set to 0xSero/Hy3-preview-nvfp4.')).toBeInTheDocument()
  })

  /* ── 17. Number input for maxToolCalls ── */
  it('updates maxToolCallsPerLoop preference', () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByText('Agent & Skills'))
    const input = screen.getByDisplayValue('12')
    fireEvent.change(input, { target: { value: '25' } })
    expect(useConfigStore.getState().preferences.maxToolCallsPerLoop).toBe(25)
  })

  /* ── 18. Font scale input in Interface ── */
  it('updates fontScale preference', () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByText('Interface'))
    const input = screen.getByDisplayValue('100')
    fireEvent.change(input, { target: { value: '110' } })
    expect(useConfigStore.getState().preferences.fontScale).toBe(110)
  })

  /* ── 19. active category button gets active class ── */
  it('highlights the active category button', () => {
    render(<SettingsView />)
    const nav = screen.getByRole('navigation', { name: 'Settings categories' })
    const aiBtn = nav.querySelector('.settings-nav-item.active')!
    expect(aiBtn.textContent).toContain('AI & model')

    fireEvent.click(screen.getByText('Interface'))
    const uiBtn = nav.querySelector('.settings-nav-item.active')!
    expect(uiBtn.textContent).toContain('Interface')
  })

  /* ── 20. sidebar has navigation role ── */
  it('sidebar has proper navigation role', () => {
    render(<SettingsView />)
    expect(screen.getByRole('navigation', { name: 'Settings categories' })).toBeInTheDocument()
  })

  it('updates visible settings text when the language changes', async () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByText('Terminal & Processes'))

    expect(screen.getByRole('option', { name: 'Runtime only' })).toBeInTheDocument()

    await i18n.changeLanguage('de')

    await waitFor(() => {
      expect(screen.getByRole('navigation', { name: 'Einstellungskategorien' })).toBeInTheDocument()
    })
    expect(screen.getByRole('option', { name: 'Nur Laufzeit' })).toBeInTheDocument()
  })
})
