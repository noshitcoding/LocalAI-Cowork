import { render, screen, fireEvent, within, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter, useLocation } from 'react-router-dom'
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
    case 'chat_search': return Promise.resolve([])
    case 'insights_list': return Promise.resolve([])
    case 'insights_summary': return Promise.resolve({ totalChats: 0, totalRuns: 0, totalEvents: 0 })
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
        provider: 'openai-compatible',
        preset: 'ollama',
        authMode: 'none',
        baseUrl: 'http://localhost:11434/v1',
        model: 'llama3.1:8b',
        apiKey: '',
        hasApiKey: false,
        timeoutMs: 200000,
        verifyTlsCertificates: true,
        contextWindow: 128000,
        temperature: 0.1,
      },
      {
        id: 'default-openai-compatible',
        name: 'OpenAI',
        provider: 'openai-compatible',
        preset: 'openai',
        authMode: 'bearer',
        baseUrl: 'https://api.openai.com/v1',
        model: 'gpt-4.1-mini',
        apiKey: '',
        hasApiKey: false,
        timeoutMs: 600000,
        verifyTlsCertificates: true,
        contextWindow: null,
        temperature: null,
      },
      {
        id: 'default-openrouter',
        name: 'OpenRouter',
        provider: 'openai-compatible',
        preset: 'openrouter',
        authMode: 'bearer',
        baseUrl: 'https://openrouter.ai/api/v1',
        model: '',
        apiKey: '',
        hasApiKey: false,
        timeoutMs: 600000,
        verifyTlsCertificates: true,
        contextWindow: null,
        temperature: null,
      },
    ],
    defaultLlmProfileIds: {
      api: 'default-ollama',
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
    },
    contextWarning: { level: 'none', estimatedTokens: 0 },
    contextCoverage: null,
    checkOllamaStatus: checkOllamaStatusMock,
    fetchOllamaModels: fetchOllamaModelsMock,
  })
}

function renderSettingsView(initialEntries = ['/settings']) {
  return render(
    <MemoryRouter initialEntries={initialEntries}>
      <SettingsView />
      <SettingsLocation />
    </MemoryRouter>
  )
}

function SettingsLocation() {
  const location = useLocation()
  return <div data-testid="settings-location" hidden>{`${location.pathname}${location.search}${location.hash}`}</div>
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

  it('renders only the category selected by the URL', () => {
    renderSettingsView(['/settings?section=security'])

    expect(screen.getByLabelText('Security & data')).toBeInTheDocument()
    expect(screen.queryByLabelText('AI & model')).not.toBeInTheDocument()
    expect(screen.queryByRole('tablist')).not.toBeInTheDocument()
    expect(screen.queryByRole('searchbox', { name: 'Search settings' })).not.toBeInTheDocument()
  })

  it('shows sandbox readiness without starting UAC until the setup button is confirmed', async () => {
    ;(window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {}
    invokeMock.mockImplementation((command: string) => {
      if (command === 'sandbox_setup_status') {
        return Promise.resolve({
          supported: true,
          ready: false,
          version: 1,
          account: 'LACoworkOnline',
          group: 'LACoworkSandbox',
          reason: 'not configured',
        })
      }
      if (command === 'sandbox_setup_start') {
        return Promise.resolve({
          supported: true,
          ready: true,
          version: 1,
          account: 'LACoworkOnline',
          group: 'LACoworkSandbox',
          reason: null,
        })
      }
      return defaultInvoke(command)
    })
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true)
    renderSettingsView(['/settings?section=sandbox'])

    await screen.findByText('not configured')
    expect(invokeMock).not.toHaveBeenCalledWith('sandbox_setup_start', expect.anything())

    fireEvent.click(screen.getByRole('button', { name: 'Set up sandbox' }))
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('sandbox_setup_start', undefined))
    expect(confirmSpy).toHaveBeenCalledTimes(1)
    confirmSpy.mockRestore()
  })

  it('keeps the actionable sandbox setup error visible after refreshing readiness', async () => {
    ;(window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {}
    invokeMock.mockImplementation((command: string) => {
      if (command === 'sandbox_setup_status') {
        return Promise.resolve({
          supported: true,
          ready: false,
          version: 1,
          account: 'LACoworkOnline',
          group: 'LACoworkSandbox',
          reason: 'elevated sandbox setup has not completed',
        })
      }
      if (command === 'sandbox_setup_start') {
        return Promise.reject(new Error('failed to store the sandbox setup marker: access denied'))
      }
      return defaultInvoke(command)
    })
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true)
    renderSettingsView(['/settings?section=sandbox'])

    await screen.findByText('elevated sandbox setup has not completed')
    fireEvent.click(screen.getByRole('button', { name: 'Set up sandbox' }))

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'failed to store the sandbox setup marker: access denied',
    )
    expect(invokeMock.mock.calls.filter(([command]) => command === 'sandbox_setup_status')).toHaveLength(2)
    confirmSpy.mockRestore()
  })

  it('localizes the directly selected category', async () => {
    await i18n.changeLanguage('de')
    renderSettingsView(['/settings?section=ui'])

    expect(screen.getByLabelText('Oberfläche')).toBeInTheDocument()
  })

  /* 2. default category is AI & model */
  it('shows AI & model content by default', () => {
    renderSettingsView()
    expect(screen.getByLabelText('AI & model')).toBeInTheDocument()
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
    expect(within(openRouter).getByText('Access key needed')).toBeInTheDocument()
    expect(openRouter).toHaveAttribute('aria-expanded', 'true')
  })

  it('opens a category from the section query parameter', () => {
    renderSettingsView(['/settings?section=security'])
    expect(screen.getByLabelText('Security & data')).toBeInTheDocument()
    expect(screen.queryByRole('tab')).not.toBeInTheDocument()
  })

  it('opens AI Sandbox in its own settings category', async () => {
    ;(window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {}
    invokeMock.mockImplementation((command: string) => {
      if (command === 'sandbox_setup_status') {
        return Promise.resolve({
          supported: true,
          ready: false,
          version: 1,
          account: 'LACoworkOnline',
          group: 'LACoworkSandbox',
          reason: 'not configured',
        })
      }
      return defaultInvoke(command)
    })
    renderSettingsView(['/settings?section=sandbox'])

    expect(screen.getByLabelText('AI Sandbox')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Set up sandbox' })).toBeInTheDocument()
    expect(screen.queryByLabelText('Security & data')).not.toBeInTheDocument()
    await screen.findByText('Not configured')
  })

  it('redirects the legacy sandbox deep link to the Sandbox category', async () => {
    renderSettingsView(['/settings?section=security#ai-sandbox'])

    await waitFor(() => expect(screen.getByLabelText('AI Sandbox')).toBeInTheDocument())
    expect(screen.getByTestId('settings-location')).toHaveTextContent('/settings?section=sandbox')
    expect(screen.queryByLabelText('Security & data')).not.toBeInTheDocument()
  })

  it('opens and focuses a provider requested by the recovery link', async () => {
    renderSettingsView(['/settings?provider=openrouter'])

    const openRouter = screen.getByRole('button', { name: 'Open OpenRouter settings' })
    expect(openRouter).toHaveAttribute('aria-expanded', 'true')
    await waitFor(() => expect(screen.getByLabelText('OpenRouter Access key for the application programming interface')).toHaveFocus())
  })

  it('opens Agent & Skills through its direct URL', () => {
    renderSettingsView(['/settings?section=agent'])
    expect(screen.getByLabelText('Agent & Skills')).toBeInTheDocument()
  })

  /* 4. Interface category */
  it('switches to Interface category', () => {
    renderSettingsView(['/settings?section=ui'])
    expect(screen.getByLabelText('Interface')).toBeInTheDocument()
    expect(screen.getByText('Focus mode')).toBeInTheDocument()
    expect(screen.getByText('Compact mode')).toBeInTheDocument()
  })

  /* 5. security category */
  it('switches to Security & data category', () => {
    renderSettingsView(['/settings?section=security'])
    expect(screen.getByLabelText('Security & data')).toBeInTheDocument()
    expect(screen.getByText('Read-only mode')).toBeInTheDocument()
    expect(screen.queryByLabelText('AI Sandbox')).not.toBeInTheDocument()
  })

  /* 6. System & Info shows runtime info */
  it('switches to System & Info and shows runtime info', () => {
    renderSettingsView(['/settings?section=system'])
    expect(screen.getByLabelText('System & Info')).toBeInTheDocument()
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
    renderSettingsView(['/settings?section=memory'])
    expect(screen.getByLabelText('Memory')).toBeInTheDocument()
  })

  /* 8. Runs & Insights category renders */
  it('switches to Runs & Insights', () => {
    renderSettingsView(['/settings?section=runs'])
    expect(screen.getByLabelText('Runs & Insights')).toBeInTheDocument()
  })

  /* 9. Terminal & Processes category renders */
  it('switches to Terminal & Processes', () => {
    renderSettingsView(['/settings?section=terminal'])
    expect(screen.getByLabelText('Terminal & Processes')).toBeInTheDocument()
    expect(screen.getByText('Terminal dock')).toBeInTheDocument()
    expect(screen.getByRole('combobox', { name: 'Persistence' })).toHaveValue('runtime')
  })

  /* 10. MCP Server category renders */
  it('switches to MCP Server', () => {
    renderSettingsView(['/settings?section=mcp'])
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
    expect(useConfigStore.getState().llmProfiles.find((profile) => profile.id === 'default-ollama')?.baseUrl).toBe('http://localhost:11434/v1')
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
    renderSettingsView(['/settings?section=agent'])
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
    fireEvent.click(screen.getByRole('button', { name: 'Open OpenAI settings' }))
    const profileName = screen.getAllByText('OpenAI', { selector: 'strong' })
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
    renderSettingsView(['/settings?section=agent'])
    const input = screen.getByDisplayValue('12')
    fireEvent.change(input, { target: { value: '25' } })
    expect(useConfigStore.getState().preferences.maxToolCallsPerLoop).toBe(25)
  })

  /* 18. Font scale input in Interface */
  it('updates fontScale preference', () => {
    renderSettingsView(['/settings?section=ui'])
    const input = screen.getByDisplayValue('100')
    fireEvent.change(input, { target: { value: '110' } })
    expect(useConfigStore.getState().preferences.fontScale).toBe(110)
  })

  it('updates visible settings text when the language changes', async () => {
    renderSettingsView(['/settings?section=terminal'])

    expect(screen.getByRole('option', { name: 'Runtime only' })).toBeInTheDocument()

    await i18n.changeLanguage('de')

    await waitFor(() => {
      expect(screen.getByLabelText('Terminal & Prozesse')).toBeInTheDocument()
    })
    expect(screen.getByRole('option', { name: 'Nur Laufzeit' })).toBeInTheDocument()
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
