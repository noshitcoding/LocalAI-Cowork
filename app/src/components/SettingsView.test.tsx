import { render, screen, fireEvent, within, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter } from 'react-router-dom'
import SettingsView from './SettingsView'
import { useConfigStore } from '../stores/configStore'
import { useEngineStore } from '../stores/engineStore'
import i18n from '../i18n'

const invokeMock = vi.fn()
const saveDialogMock = vi.fn()
const checkOllamaStatusMock = vi.fn()
const fetchOllamaModelsMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

vi.mock('@tauri-apps/plugin-dialog', () => ({
  save: (...args: unknown[]) => saveDialogMock(...args),
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
    case 'startup_recovery_status': return Promise.resolve({
      recoveredAt: '2026-07-10T12:00:00Z',
      engineRuns: 0,
      legacyTasks: 0,
      taskSteps: 0,
      workTasks: 0,
      scheduledRuns: 0,
      crewRuns: 0,
      workerSandboxes: 0,
      managedProcesses: 0,
      terminalBackends: 0,
    })
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

function renderSettingsView(initialEntries = ['/settings']) {
  return render(
    <MemoryRouter initialEntries={initialEntries}>
      <SettingsView />
    </MemoryRouter>
  )
}

describe('SettingsView', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en')
    delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
    invokeMock.mockReset()
    invokeMock.mockImplementation(defaultInvoke)
    saveDialogMock.mockReset()
    saveDialogMock.mockResolvedValue(null)
    resetConfigStore()
    resetEngineStore()
  })

  /* 1. sidebar renders all 9 categories */
  it('renders all 9 category buttons in sidebar', () => {
    renderSettingsView()
    const tabs = screen.getByRole('tablist', { name: 'Settings categories' })
    const buttons = tabs.querySelectorAll('.settings-nav-item')
    expect(buttons.length).toBe(9)
  })

  it('filters settings categories by label and description', () => {
    renderSettingsView()
    const search = screen.getByRole('searchbox', { name: 'Search settings' })

    fireEvent.change(search, { target: { value: 'file access' } })

    const tabs = screen.getAllByRole('tab')
    expect(tabs).toHaveLength(1)
    expect(tabs[0]).toHaveTextContent('Security & data')

    fireEvent.click(tabs[0])
    expect(screen.getByRole('heading', { level: 1, name: 'Security & data' })).toBeInTheDocument()

    fireEvent.change(search, { target: { value: 'definitely missing' } })
    expect(screen.getByRole('status')).toHaveTextContent('No settings sections match your search')
    expect(screen.queryByRole('heading', { level: 1, name: 'Security & data' })).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Clear' }))
    expect(screen.getAllByRole('tab')).toHaveLength(9)
    expect(screen.getByRole('heading', { level: 1, name: 'Security & data' })).toBeInTheDocument()
  })

  it('finds the category for a concrete setting instead of only category copy', () => {
    renderSettingsView(['/settings?section=ui'])

    fireEvent.change(screen.getByRole('searchbox', { name: 'Search settings' }), { target: { value: 'API key' } })

    const tabs = screen.getAllByRole('tab')
    expect(tabs).toHaveLength(1)
    expect(tabs[0]).toHaveTextContent('AI & model')
    expect(screen.getByRole('heading', { level: 1, name: 'AI & model' })).toBeInTheDocument()
  })

  it('matches German setting terms and umlaut spellings', async () => {
    await i18n.changeLanguage('de')
    renderSettingsView(['/settings?section=ui'])
    const search = screen.getByRole('searchbox', { name: 'Einstellungen durchsuchen' })

    fireEvent.change(search, { target: { value: 'API-Schlüssel' } })
    expect(screen.getAllByRole('tab')).toHaveLength(1)
    expect(screen.getByRole('heading', { level: 1, name: 'KI & Modell' })).toBeInTheDocument()

    fireEvent.change(search, { target: { value: 'oberflaeche' } })
    expect(screen.getAllByRole('tab')).toHaveLength(1)
    expect(screen.getByRole('heading', { level: 1, name: 'Oberfläche' })).toBeInTheDocument()
  })

  /* 2. default category is AI & model */
  it('shows AI & model content by default', () => {
    renderSettingsView()
    expect(screen.getByRole('heading', { level: 1, name: 'AI & model' })).toBeInTheDocument()
  })

  it('summarizes provider readiness and highlights OpenRouter free models', () => {
    useConfigStore.getState().updateLlmProfile('default-openrouter', {
      model: 'nvidia/nemotron-3-super-120b-a12b:free',
    })

    renderSettingsView()

    const overview = screen.getByRole('group', { name: 'Provider overview' })
    expect(within(overview).getAllByRole('button')).toHaveLength(3)
    expect(within(overview).getByText('Free model')).toBeInTheDocument()
    const openRouter = within(overview).getByRole('button', { name: 'Open OpenRouter settings' })
    expect(within(openRouter).getByText('API key needed')).toBeInTheDocument()
    expect(openRouter).toHaveAttribute('aria-expanded', 'true')
  })

  it('opens a category from the section query parameter', () => {
    renderSettingsView(['/settings?section=security'])
    expect(screen.getByRole('heading', { level: 1, name: 'Security & data' })).toBeInTheDocument()
    expect(screen.getByRole('tab', { name: 'Security & data' })).toHaveAttribute('aria-selected', 'true')
  })

  it('opens and focuses a provider requested by the recovery link', async () => {
    renderSettingsView(['/settings?provider=openrouter'])

    const openRouter = screen.getByRole('button', { name: 'Open OpenRouter settings' })
    expect(openRouter).toHaveAttribute('aria-expanded', 'true')
    await waitFor(() => expect(screen.getByLabelText('OpenRouter API Key')).toHaveFocus())
  })

  /* 3. navigation switches categories */
  it('switches to Agent & Skills when clicked', () => {
    renderSettingsView()
    fireEvent.click(screen.getByRole('tab', { name: 'Agent & Skills' }))
    expect(screen.getByRole('heading', { level: 1, name: 'Agent & Skills' })).toBeInTheDocument()
  })

  it('switches categories through the compact selector', () => {
    renderSettingsView()
    const selector = screen.getByRole('combobox', { name: 'Settings categories' })

    expect(selector).toHaveValue('ai')
    fireEvent.change(selector, { target: { value: 'security' } })

    expect(selector).toHaveValue('security')
    expect(screen.getByRole('heading', { level: 1, name: 'Security & data' })).toBeInTheDocument()
  })

  /* 4. Interface category */
  it('switches to Interface category', () => {
    renderSettingsView()
    fireEvent.click(screen.getByRole('tab', { name: 'Interface' }))
    expect(screen.getByRole('heading', { level: 1, name: 'Interface' })).toBeInTheDocument()
    expect(screen.getByText('Focus mode')).toBeInTheDocument()
    expect(screen.getByText('Compact mode')).toBeInTheDocument()
  })

  /* 5. security category */
  it('switches to Security & data category', () => {
    renderSettingsView()
    fireEvent.click(screen.getByRole('tab', { name: 'Security & data' }))
    expect(screen.getByRole('heading', { level: 1, name: 'Security & data' })).toBeInTheDocument()
    expect(screen.getByText('Read-only mode')).toBeInTheDocument()
  })

  /* 6. System & Info shows runtime info */
  it('switches to System & Info and shows runtime info', () => {
    renderSettingsView()
    fireEvent.click(screen.getByRole('tab', { name: 'System & Info' }))
    expect(screen.getByRole('heading', { level: 1, name: 'System & Info' })).toBeInTheDocument()
    expect(screen.getByText('Local LLM endpoint')).toBeInTheDocument()
    expect(screen.getByText('http://localhost:11434')).toBeInTheDocument()
    expect(screen.getByText('Default model')).toBeInTheDocument()
    expect(screen.getByText('llama3.1:8b')).toBeInTheDocument()
    expect(screen.getByText('Creator')).toBeInTheDocument()
    expect(screen.getByText('noshitcoding')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'github.com/noshitcoding/LocalAI Cowork' })).toHaveAttribute('href', 'https://github.com/noshitcoding/LocalAI-Cowork')
    expect(screen.getByText('Disclaimer')).toBeInTheDocument()
    expect(screen.getByText(/Use it at your own risk/)).toBeInTheDocument()
  })

  /* 7. Memory category renders */
  it('switches to Memory category', () => {
    renderSettingsView()
    fireEvent.click(screen.getByRole('tab', { name: 'Memory' }))
    expect(screen.getByRole('heading', { level: 1, name: 'Memory' })).toBeInTheDocument()
  })

  /* 8. Sessions & Insights category renders */
  it('switches to Sessions & Insights', () => {
    renderSettingsView()
    fireEvent.click(screen.getByRole('tab', { name: 'Sessions & Insights' }))
    expect(screen.getByRole('heading', { level: 1, name: 'Sessions & Insights' })).toBeInTheDocument()
  })

  /* 9. Terminal & Processes category renders */
  it('switches to Terminal & Processes', () => {
    renderSettingsView()
    fireEvent.click(screen.getByRole('tab', { name: 'Terminal & Processes' }))
    expect(screen.getByRole('heading', { level: 1, name: 'Terminal & Processes' })).toBeInTheDocument()
    expect(screen.getByText('Terminal dock')).toBeInTheDocument()
    expect(screen.getByRole('combobox', { name: 'Persistence' })).toHaveValue('runtime')
  })

  /* 10. MCP Server category renders */
  it('switches to MCP Server', () => {
    renderSettingsView()
    fireEvent.click(screen.getByRole('tab', { name: 'MCP Server' }))
    // McpView also has an h1 "MCP Server", so check for the settings toggle instead
    expect(screen.getByText('Auto-reconnect')).toBeInTheDocument()
    expect(screen.getByText('Verbose logging')).toBeInTheDocument()
  })

  /* 11. Legacy Ollama config section removed */
  it('does not render the legacy Ollama configuration section', () => {
    renderSettingsView()
    expect(screen.queryByRole('heading', { level: 2, name: /Ollama configuration/ })).not.toBeInTheDocument()
  })

  it('does not render OpenAI Computer Use settings', () => {
    renderSettingsView()
    expect(screen.queryByRole('heading', { level: 2, name: /OpenAI Computer Use/ })).not.toBeInTheDocument()
    expect(screen.queryByText('Safety Checks automatisch bestaetigen')).not.toBeInTheDocument()
  })

  /* 12. Default Ollama profile endpoint updates store */
  it('updates default Ollama profile endpoint on input change', () => {
    renderSettingsView()
    const profileCard = screen.getByText('Lokales Ollama', { selector: 'strong' }).closest('.card') as HTMLElement
    const endpointInput = within(profileCard).getByLabelText('Endpoint')
    fireEvent.change(endpointInput, { target: { value: 'http://localhost:11434' } })
    expect(useConfigStore.getState().ollama.baseUrl).toBe('http://localhost:11434')
    expect(useConfigStore.getState().llmProfiles.find((profile) => profile.id === 'default-ollama')?.baseUrl).toBe('http://localhost:11434')
  })

  /* 13. Default Ollama profile model updates store */
  it('updates default Ollama profile model on input change', () => {
    useConfigStore.getState().setLlmProfileModels('default-ollama', ['llama3.1:8b', 'mistral:7b'])
    renderSettingsView()
    const profileCard = screen.getByText('Lokales Ollama', { selector: 'strong' }).closest('.card') as HTMLElement
    const modelControl = within(profileCard).getByLabelText('Model')
    expect(modelControl.tagName).toBe('SELECT')
    fireEvent.change(modelControl, { target: { value: 'mistral:7b' } })
    expect(useConfigStore.getState().ollama.model).toBe('mistral:7b')
    expect(useConfigStore.getState().llmProfiles.find((profile) => profile.id === 'default-ollama')?.model).toBe('mistral:7b')
  })

  /* 14. Toggle updates preference */
  it('toggles autoApproveSafeTools preference', () => {
    renderSettingsView()
    fireEvent.click(screen.getByRole('tab', { name: 'Agent & Skills' }))
    const toggleBtn = screen.getByText('Automatically approve safe tools').closest('.toggle-row')!.querySelector('button[role="switch"]')!
    expect(toggleBtn.getAttribute('aria-checked')).toBe('true')
    fireEvent.click(toggleBtn)
    expect(useConfigStore.getState().preferences.autoApproveSafeTools).toBe(false)
  })

  /* 15. Model dropdown with Ollama profile models */
  it('renders model dropdown when Ollama profile models are set', () => {
    useConfigStore.getState().setLlmProfileModels('default-ollama', ['llama3.1:8b', 'mistral:7b', 'codellama:13b'])
    renderSettingsView()
    const profileCard = screen.getByText('Lokales Ollama', { selector: 'strong' }).closest('.card') as HTMLElement
    const modelControl = within(profileCard).getByLabelText('Model')
    expect(modelControl.tagName).toBe('SELECT')
  })

  it('uses exact external model id returned by the provider model list', async () => {
    ;(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {}
    useConfigStore.getState().updateLlmProfile('default-openai-compatible', {
      baseUrl: 'https://mlis.example.test/v1/models',
      model: 'Hy3-preview-nvfp4',
    })
    useConfigStore.setState((state) => ({
      llmProfiles: state.llmProfiles.map((profile) => (
        profile.id === 'default-openai-compatible' ? { ...profile, apiKey: 'sk-test' } : profile
      )),
    }))
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'crew_provider_models_list') {
        return Promise.resolve({
          endpoint: 'https://mlis.example.test/v1/models',
          models: ['0xSero/Hy3-preview-nvfp4'],
        })
      }
      return defaultInvoke(cmd)
    })

    renderSettingsView()
    fireEvent.click(screen.getByRole('button', { name: 'Open OpenAI-compatible settings' }))
    const profileName = screen.getAllByText('OpenAI-compatible', { selector: 'strong' })
      .find((element) => element.closest('.llm-profile-card'))
    const profileCard = profileName?.closest('.llm-profile-card') as HTMLElement
    fireEvent.click(within(profileCard).getByRole('button', { name: 'Load models' }))

    await waitFor(() => {
      expect(useConfigStore.getState().llmProfiles.find((profile) => profile.id === 'default-openai-compatible')?.model)
        .toBe('0xSero/Hy3-preview-nvfp4')
    })
    expect(await within(profileCard).findByText('Model automatically set to 0xSero/Hy3-preview-nvfp4.')).toBeInTheDocument()
  })

  it('does not report cached external models as freshly loaded after a refresh fails', async () => {
    ;(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {}
    useConfigStore.getState().updateLlmProfile('default-openrouter', {
      model: 'openai/gpt-4o-mini',
    })
    useConfigStore.getState().setLlmProfileModels('default-openrouter', [
      'openai/gpt-4o-mini',
      'google/gemini-2.5-pro',
    ])
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'crew_provider_models_list') {
        return Promise.reject(new Error('error sending request for url (https://openrouter.ai/api/v1/models)'))
      }
      return defaultInvoke(cmd)
    })

    renderSettingsView(['/settings?provider=openrouter'])
    const profileName = screen.getAllByText('OpenRouter', { selector: 'strong' })
      .find((element) => element.closest('.llm-profile-card'))
    const profileCard = profileName?.closest('.llm-profile-card') as HTMLElement

    expect(within(profileCard).getByText(/2 model\(s\) loaded/i)).toBeInTheDocument()
    fireEvent.click(within(profileCard).getByRole('button', { name: 'Load models' }))

    expect(await within(profileCard).findByText(/error sending request for url/i)).toBeInTheDocument()
    expect(within(profileCard).queryByText(/2 model\(s\) loaded/i)).not.toBeInTheDocument()
  })

  /* 17. Number input for maxToolCalls */
  it('updates maxToolCallsPerLoop preference', () => {
    renderSettingsView()
    fireEvent.click(screen.getByRole('tab', { name: 'Agent & Skills' }))
    const input = screen.getByDisplayValue('12')
    fireEvent.change(input, { target: { value: '25' } })
    expect(useConfigStore.getState().preferences.maxToolCallsPerLoop).toBe(25)
  })

  /* 18. Font scale input in Interface */
  it('updates fontScale preference', () => {
    renderSettingsView()
    fireEvent.click(screen.getByRole('tab', { name: 'Interface' }))
    const input = screen.getByDisplayValue('100')
    fireEvent.change(input, { target: { value: '110' } })
    expect(useConfigStore.getState().preferences.fontScale).toBe(110)
  })

  /* 19. active category button gets active class */
  it('highlights the active category button', () => {
    renderSettingsView()
    const tabs = screen.getByRole('tablist', { name: 'Settings categories' })
    const aiBtn = tabs.querySelector('.settings-nav-item.active')!
    expect(aiBtn.textContent).toContain('AI & model')

    fireEvent.click(screen.getByRole('tab', { name: 'Interface' }))
    const uiBtn = tabs.querySelector('.settings-nav-item.active')!
    expect(uiBtn.textContent).toContain('Interface')
  })

  /* 20. sidebar has navigation role */
  it('sidebar has proper tablist role', () => {
    renderSettingsView()
    expect(screen.getByRole('tablist', { name: 'Settings categories' })).toBeInTheDocument()
  })

  it('updates visible settings text when the language changes', async () => {
    renderSettingsView()
    fireEvent.click(screen.getByRole('tab', { name: 'Terminal & Processes' }))

    expect(screen.getByRole('option', { name: 'Runtime only' })).toBeInTheDocument()

    await i18n.changeLanguage('de')

    await waitFor(() => {
      expect(screen.getByRole('tablist', { name: 'Einstellungskategorien' })).toBeInTheDocument()
    })
    expect(screen.getByRole('option', { name: 'Nur Laufzeit' })).toBeInTheDocument()

    fireEvent.click(screen.getByRole('tab', { name: 'KI & Modell' }))
    expect(screen.getByText('Mehrere Endpunkte parallel verwalten und pro Provider ein globales Standardprofil für Auswahllisten und Rückfälle festlegen.')).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Persönlichkeiten verwalten' })).toBeInTheDocument()
    expect(await screen.findByText('Entwickle wartbare Software mit verifizierten Ergebnissen.')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Persönlichkeit auswählen Kreativ' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Persönlichkeit löschen Assistent' })).toBeInTheDocument()
  })

  it('creates a support bundle from system settings', async () => {
    ;(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {}
    saveDialogMock.mockResolvedValue('C:\\Temp\\open-cowork-support.zip')
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'support_bundle_create') {
        return Promise.resolve({
          path: 'C:\\Temp\\open-cowork-support.zip',
          sizeBytes: 2048,
          createdAt: '2026-07-10T12:00:00Z',
          fileCount: 5,
        })
      }
      return defaultInvoke(cmd)
    })

    renderSettingsView(['/settings?section=system'])
    fireEvent.click(screen.getByRole('button', { name: 'Create support bundle' }))

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('support_bundle_create', {
        path: 'C:\\Temp\\open-cowork-support.zip',
      })
    })
    expect(await screen.findByRole('status')).toHaveTextContent('Support bundle saved.')
  })

  it('shows the number of states recovered during startup', async () => {
    ;(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {}
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'startup_recovery_status') {
        return Promise.resolve({
          recoveredAt: '2026-07-10T12:00:00Z',
          engineRuns: 1,
          legacyTasks: 0,
          taskSteps: 0,
          workTasks: 1,
          scheduledRuns: 0,
          crewRuns: 0,
          workerSandboxes: 1,
          managedProcesses: 0,
          terminalBackends: 0,
        })
      }
      return defaultInvoke(cmd)
    })

    renderSettingsView(['/settings?section=system'])

    expect(await screen.findByLabelText('Recovered startup states')).toHaveValue('3')
  })
})
