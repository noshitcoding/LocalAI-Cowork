import { useEffect, useMemo, useRef, useState } from 'react'
import { useConfigStore, type OllamaConfig } from '../stores/configStore'
import { useCoworkStore } from '../stores/coworkStore'
import { useCrewStore, type AgentRole, type CrewExternalProviderConfig, type CrewOutputMode, type CrewProcess, type CrewProviderKind } from '../stores/crewStore'
import { safeInvoke } from '../utils/safeInvoke'

const ROLE_OPTIONS: AgentRole[] = ['researcher', 'writer', 'reviewer', 'planner', 'executor', 'analyst', 'custom']
const PROCESS_OPTIONS: Array<{ value: CrewProcess; label: string }> = [
  { value: 'sequential', label: 'Sequenziell' },
  { value: 'parallel', label: 'Parallel' },
  { value: 'hierarchical', label: 'Hierarchisch' },
]
const PROVIDER_OPTIONS: Array<{ value: CrewProviderKind; label: string }> = [
  { value: 'ollama', label: 'Ollama' },
  { value: 'openai-compatible', label: 'OpenAI-kompatibel' },
  { value: 'openrouter', label: 'OpenRouter' },
]

function toggleStringValue(values: string[], nextValue: string): string[] {
  return values.includes(nextValue)
    ? values.filter((value) => value !== nextValue)
    : [...values, nextValue]
}

function agentUsesCrewDefaultProvider(
  agent: { providerKind?: CrewProviderKind; modelOverride?: string | null },
  crewDefaultProvider: CrewProviderKind,
): boolean {
  return (agent.providerKind ?? crewDefaultProvider) === crewDefaultProvider && !agent.modelOverride?.trim()
}

function resolveCrewRuntimeConfig(
  runtimeConfig: { enabled: boolean; baseUrl: string; model: string; timeoutMs: number } | undefined,
  fallbackConfig: OllamaConfig,
): OllamaConfig {
  if (!runtimeConfig?.enabled) {
    return fallbackConfig
  }

  return {
    ...fallbackConfig,
    baseUrl: runtimeConfig.baseUrl.trim() || fallbackConfig.baseUrl,
    model: runtimeConfig.model.trim() || fallbackConfig.model,
    timeoutMs: Math.max(1000, runtimeConfig.timeoutMs || fallbackConfig.timeoutMs),
  }
}

function resolveExternalProviderConfig(
  config: CrewExternalProviderConfig | undefined,
  fallbackConfig: { baseUrl?: string; model?: string; apiKey?: string } | undefined,
  fallbackBaseUrl: string,
) {
  if (!config?.enabled) {
    return undefined
  }

  return {
    baseUrl: config.baseUrl.trim() || fallbackConfig?.baseUrl?.trim() || fallbackBaseUrl,
    model: config.model.trim() || fallbackConfig?.model?.trim() || '',
    apiKey: config.apiKey.trim() || fallbackConfig?.apiKey?.trim() || '',
    timeoutMs: Math.max(1000, config.timeoutMs || 600000),
  }
}

function downloadCrewJson(fileName: string, payload: unknown) {
  const blob = new Blob([JSON.stringify(payload, null, 2)], { type: 'application/json' })
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = fileName
  anchor.click()
  URL.revokeObjectURL(url)
}

function buildDefaultCrewName(existingNames: string[]): string {
  let index = 1
  while (existingNames.some((name) => name.trim().toLowerCase() === `crew ${index}`.toLowerCase())) {
    index += 1
  }
  return `Crew ${index}`
}

function getProviderKey(providerKind: CrewProviderKind): 'openAICompatible' | 'openRouter' | null {
  if (providerKind === 'openai-compatible') return 'openAICompatible'
  if (providerKind === 'openrouter') return 'openRouter'
  return null
}

type CrewProviderModelsResult = {
  endpoint: string
  models: string[]
}
function getCrewDiagnostics(crew: {
  process: CrewProcess
  managerAgentId: string | null
  agents: Array<{ id: string; name: string; enabled: boolean; providerKind?: CrewProviderKind; modelOverride?: string | null }>
  providerProfiles?: {
    openAICompatible: CrewExternalProviderConfig
    openRouter: CrewExternalProviderConfig
  }
},
openAIProfile?: { baseUrl?: string; model?: string; apiKey?: string },
openRouterProfile?: { baseUrl?: string; model?: string; apiKey?: string }) {
  const errors: string[] = []
  const warnings: string[] = []
  const enabledAgentIds = new Set(crew.agents.filter((agent) => agent.enabled).map((agent) => agent.id))
  const enabledAgents = crew.agents.filter((agent) => agent.enabled)

  if (enabledAgentIds.size === 0) {
    errors.push('Keine aktiven Crew-Mitglieder vorhanden.')
  }

  const openAiAgents = enabledAgents.filter((agent) => agent.providerKind === 'openai-compatible')
  if (openAiAgents.length > 0) {
    const profile = crew.providerProfiles?.openAICompatible
    const effectiveApiKey = profile?.apiKey.trim() || openAIProfile?.apiKey?.trim() || ''
    const effectiveBaseUrl = profile?.baseUrl.trim() || openAIProfile?.baseUrl?.trim() || ''
    const effectiveModel = profile?.model.trim() || openAIProfile?.model?.trim() || ''
    const needsFallbackModel = openAiAgents.some((agent) => !agent.modelOverride?.trim())
    if (!profile?.enabled) {
      errors.push('Aktive Crew-Mitglieder nutzen OpenAI-kompatibles Routing, aber das Crew-Profil ist nicht aktiviert.')
    }
    if (!effectiveApiKey) {
      errors.push('OpenAI-kompatibles Crew-Profil hat keinen API-Key.')
    }
    if (!effectiveBaseUrl) {
      errors.push('OpenAI-kompatibles Crew-Profil hat keinen Endpoint.')
    }
    if (needsFallbackModel && !effectiveModel) {
      errors.push('OpenAI-kompatibles Crew-Profil hat kein Modell und mindestens ein Agent besitzt keinen Model-Override.')
    }
  }

  const openRouterAgents = enabledAgents.filter((agent) => agent.providerKind === 'openrouter')
  if (openRouterAgents.length > 0) {
    const profile = crew.providerProfiles?.openRouter
    const effectiveApiKey = profile?.apiKey.trim() || openRouterProfile?.apiKey?.trim() || ''
    const effectiveBaseUrl = profile?.baseUrl.trim() || openRouterProfile?.baseUrl?.trim() || 'https://openrouter.ai/api/v1'
    const effectiveModel = profile?.model.trim() || openRouterProfile?.model?.trim() || ''
    const needsFallbackModel = openRouterAgents.some((agent) => !agent.modelOverride?.trim())
    if (!profile?.enabled) {
      errors.push('Aktive Crew-Mitglieder nutzen OpenRouter, aber das Crew-Profil ist nicht aktiviert.')
    }
    if (!effectiveApiKey) {
      errors.push('OpenRouter-Crew-Profil hat keinen API-Key.')
    }
    if (!effectiveBaseUrl) {
      errors.push('OpenRouter-Crew-Profil hat keinen Endpoint.')
    }
    if (needsFallbackModel && !effectiveModel) {
      errors.push('OpenRouter-Crew-Profil hat kein Modell und mindestens ein Agent besitzt keinen Model-Override.')
    }
  }

  if (crew.process === 'hierarchical') {
    if (!crew.managerAgentId) {
      errors.push('Hierarchische Crew benoetigt einen Manager-Agenten.')
    } else if (!enabledAgentIds.has(crew.managerAgentId)) {
      errors.push('Der gewaehlte Manager-Agent ist deaktiviert oder fehlt.')
    }
  }

  return { errors, warnings }
}

export default function CrewPanel() {
  const {
    crews,
    activeCrewId,
    createCrew,
    updateCrew,
    deleteCrew,
    setActiveCrew,
    loadAgents,
    installDefaultAgents,
    updateCrewAgent,
  } = useCrewStore()
  const { availableModels, defaultLlmProfileIds, llmProfiles, mcpServer, mcpServers, ollama } = useConfigStore()
  const claudeTools = useCoworkStore((state) => state.claudeTools)

  const [crewName, setCrewName] = useState('')
  const [providerModelOptions, setProviderModelOptions] = useState<Record<string, { loading: boolean; endpoint?: string; models: string[]; error?: string }>>({})
  const [pendingScrollCrewId, setPendingScrollCrewId] = useState<string | null>(null)
  const [openSections, setOpenSections] = useState<Record<string, boolean>>({ general: true, execution: false, provider: false, diagnostics: true })
  const [expandedAgents, setExpandedAgents] = useState<Record<string, boolean>>({})
  const importCrewInputRef = useRef<HTMLInputElement | null>(null)
  const activeCrewDetailsRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    loadAgents()
    installDefaultAgents()
  }, [installDefaultAgents, loadAgents])

  const activeCrew = useMemo(
    () => crews.find((crew) => crew.id === activeCrewId) ?? crews[0],
    [activeCrewId, crews],
  )
  const defaultOpenAICompatibleProfile = useMemo(
    () => llmProfiles.find((profile) => profile.id === defaultLlmProfileIds['openai-compatible'] && profile.provider === 'openai-compatible')
      ?? llmProfiles.find((profile) => profile.provider === 'openai-compatible'),
    [defaultLlmProfileIds, llmProfiles],
  )
  const defaultOpenRouterProfile = useMemo(
    () => llmProfiles.find((profile) => profile.id === defaultLlmProfileIds.openrouter && profile.provider === 'openrouter')
      ?? llmProfiles.find((profile) => profile.provider === 'openrouter'),
    [defaultLlmProfileIds.openrouter, llmProfiles],
  )
  const activeCrewDiagnostics = useMemo(
    () => (activeCrew ? getCrewDiagnostics(activeCrew, defaultOpenAICompatibleProfile, defaultOpenRouterProfile) : { errors: [], warnings: [] }),
    [activeCrew, defaultOpenAICompatibleProfile, defaultOpenRouterProfile],
  )
  const resolvedActiveCrewConfig = useMemo(
    () => (activeCrew ? resolveCrewRuntimeConfig(activeCrew.runtimeConfig, ollama) : ollama),
    [activeCrew, ollama],
  )
  const resolvedActiveProviderConfigs = useMemo(
    () => activeCrew ? {
      openAICompatible: resolveExternalProviderConfig(
        activeCrew.providerProfiles.openAICompatible,
        defaultOpenAICompatibleProfile,
        defaultOpenAICompatibleProfile?.baseUrl || activeCrew.providerProfiles.openAICompatible.baseUrl || 'https://api.openai.com/v1',
      ),
      openRouter: resolveExternalProviderConfig(
        activeCrew.providerProfiles.openRouter,
        defaultOpenRouterProfile,
        defaultOpenRouterProfile?.baseUrl || activeCrew.providerProfiles.openRouter.baseUrl || 'https://openrouter.ai/api/v1',
      ),
    } : { openAICompatible: undefined, openRouter: undefined },
    [activeCrew, defaultOpenAICompatibleProfile, defaultOpenRouterProfile],
  )
  const configuredMcpServers = mcpServers.length > 0 ? mcpServers : [mcpServer]

  const getProviderModelCatalog = (providerKind: CrewProviderKind) => {
    if (providerKind === 'ollama') {
      return {
        models: availableModels,
        authoritative: true,
      }
    }

    const providerState = providerKind === 'openai-compatible'
      ? providerModelOptions.openAICompatible
      : providerModelOptions.openRouter

    return {
      models: providerState?.models ?? [],
      authoritative: Boolean(providerState && !providerState.loading && !providerState.error),
    }
  }

  const getCrewDefaultModelLabel = (providerKind: CrewProviderKind) => {
    if (activeCrew?.defaultProvider === providerKind && activeCrew.defaultModel?.trim()) {
      return activeCrew.defaultModel.trim()
    }

    if (providerKind === 'ollama') {
      return resolvedActiveCrewConfig.model || ollama.model || 'nicht gesetzt'
    }

    if (providerKind === 'openai-compatible') {
      return resolvedActiveProviderConfigs.openAICompatible?.model || defaultOpenAICompatibleProfile?.model || 'nicht gesetzt'
    }

    return resolvedActiveProviderConfigs.openRouter?.model || 'nicht gesetzt'
  }

  const getCrewDefaultModelOptions = () => {
    if (!activeCrew) return []
    const catalog = getProviderModelCatalog(activeCrew.defaultProvider || 'ollama')
    const defaultModel = activeCrew.defaultModel?.trim()
    if (!defaultModel || catalog.models.includes(defaultModel)) {
      return catalog.models
    }

    return [defaultModel, ...catalog.models]
  }

  const isProviderEnabledForCrew = (providerKind: CrewProviderKind) => {
    if (providerKind === 'ollama') {
      return true
    }

    if (!activeCrew) {
      return false
    }

    return providerKind === 'openai-compatible'
      ? activeCrew.providerProfiles.openAICompatible.enabled
      : activeCrew.providerProfiles.openRouter.enabled
  }

  const getAgentModelOptions = (agent: { providerKind: CrewProviderKind; modelOverride?: string | null }) => {
    const catalog = getProviderModelCatalog(agent.providerKind)
    const modelOverride = agent.modelOverride?.trim()

    if (!modelOverride || catalog.authoritative || catalog.models.includes(modelOverride)) {
      return catalog.models
    }

    return [modelOverride, ...catalog.models]
  }

  const handleCrewDefaultProviderChange = (providerKind: CrewProviderKind) => {
    if (providerKind !== 'ollama' && !isProviderEnabledForCrew(providerKind)) return

    const currentDefaultProvider = activeCrew?.defaultProvider || 'ollama'

    updateActiveCrew({
      defaultProvider: providerKind,
      defaultModel: '',
      agents: activeCrew?.agents.map((agent) => ({
        ...agent,
        providerKind: agentUsesCrewDefaultProvider(agent, currentDefaultProvider) ? providerKind : agent.providerKind,
      })),
    })

    const providerKey = getProviderKey(providerKind)
    if (providerKey) {
      const providerState = providerModelOptions[providerKey]
      if (!providerState?.loading && !providerState?.models.length) {
        void handleLoadProviderModels(providerKey)
      }
    }
  }

  const handleAgentProviderKindChange = (agentId: string, providerKind: CrewProviderKind) => {
    const agent = activeCrew?.agents.find((candidate) => candidate.id === agentId)
    if (!agent) return
    if (providerKind !== 'ollama' && !isProviderEnabledForCrew(providerKind)) return

    const currentModelOverride = agent.modelOverride?.trim()
    const nextCatalog = getProviderModelCatalog(providerKind)

    updateActiveCrewAgent(agentId, {
      providerKind,
      modelOverride: currentModelOverride && nextCatalog.models.includes(currentModelOverride) ? currentModelOverride : null,
    })

    if (providerKind === 'ollama') {
      return
    }

    const providerKey = providerKind === 'openai-compatible' ? 'openAICompatible' : 'openRouter'
    const providerState = providerModelOptions[providerKey]

    if (providerState?.loading || providerState?.models.length) {
      return
    }

    void handleLoadProviderModels(providerKey)
  }

  const inputStyle: React.CSSProperties = {
    padding: '6px 10px',
    borderRadius: 'var(--radius-sm)',
    border: '1px solid var(--border-color)',
    background: 'var(--bg-secondary)',
    fontSize: 13,
    width: '100%',
  }
  const textAreaStyle: React.CSSProperties = {
    ...inputStyle,
    resize: 'vertical',
    minHeight: 72,
  }

  useEffect(() => {
    setProviderModelOptions({})
  }, [activeCrew?.id])

  useEffect(() => {
    if (!pendingScrollCrewId || activeCrew?.id !== pendingScrollCrewId) return
    activeCrewDetailsRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' })
    setPendingScrollCrewId(null)
  }, [activeCrew?.id, pendingScrollCrewId])

  useEffect(() => {
    if (!activeCrew) return

    const invalidAgentIds = activeCrew.agents
      .filter((agent) => {
        const modelOverride = agent.modelOverride?.trim()
        if (!modelOverride) return false

        const catalog = getProviderModelCatalog(agent.providerKind)
        return catalog.authoritative && !catalog.models.includes(modelOverride)
      })
      .map((agent) => agent.id)

    if (invalidAgentIds.length === 0) return

    invalidAgentIds.forEach((agentId) => {
      updateCrewAgent(activeCrew.id, agentId, { modelOverride: null })
    })
  }, [activeCrew, availableModels, providerModelOptions, updateCrewAgent])

  const handleCreateCrew = () => {
    const nextName = crewName.trim() || buildDefaultCrewName(crews.map((crew) => crew.name))
    const id = crypto.randomUUID()
    createCrew(id, nextName, [])
    setActiveCrew(id)
    setPendingScrollCrewId(id)
    setCrewName('')
  }

  const updateActiveCrew = (patch: Parameters<typeof updateCrew>[1]) => {
    if (!activeCrew) return
    updateCrew(activeCrew.id, patch)
  }

  const updateActiveCrewAgent = (agentId: string, patch: Parameters<typeof updateCrewAgent>[2]) => {
    if (!activeCrew) return
    updateCrewAgent(activeCrew.id, agentId, patch)
  }

  const handleDuplicateCrew = () => {
    if (!activeCrew) return
    const newId = crypto.randomUUID()
    createCrew(newId, `${activeCrew.name} Kopie`, [])
    updateCrew(newId, {
      description: activeCrew.description,
      executionGuidelines: activeCrew.executionGuidelines,
      outputMode: activeCrew.outputMode,
      stopOnFailure: activeCrew.stopOnFailure,
      retryCount: activeCrew.retryCount,
      managerReviewEnabled: activeCrew.managerReviewEnabled,
      managerReviewGuidelines: activeCrew.managerReviewGuidelines,
      shareAllTaskOutputs: activeCrew.shareAllTaskOutputs,
      sharedOutputCharLimit: activeCrew.sharedOutputCharLimit,
      defaultProvider: activeCrew.defaultProvider,
      defaultModel: activeCrew.defaultModel,
      providerProfiles: activeCrew.providerProfiles,
      process: activeCrew.process,
      managerAgentId: activeCrew.managerAgentId,
      verbose: activeCrew.verbose,
      maxRpm: activeCrew.maxRpm,
      maxParallelTasks: activeCrew.maxParallelTasks,
      runtimeConfig: activeCrew.runtimeConfig,
      agents: activeCrew.agents.map((agent) => ({
        ...agent,
        tools: [...agent.tools],
        mcpServerNames: [...(agent.mcpServerNames ?? [])],
      })),
      tasks: [],
      status: 'idle',
    })
    setActiveCrew(newId)
    setPendingScrollCrewId(newId)
  }

  const handleExportCrew = () => {
    if (!activeCrew) return
    const fileName = `${activeCrew.name.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-') || 'crew'}.json`
    downloadCrewJson(fileName, {
      name: activeCrew.name,
      description: activeCrew.description,
      executionGuidelines: activeCrew.executionGuidelines,
      outputMode: activeCrew.outputMode,
      stopOnFailure: activeCrew.stopOnFailure,
      retryCount: activeCrew.retryCount,
      managerReviewEnabled: activeCrew.managerReviewEnabled,
      managerReviewGuidelines: activeCrew.managerReviewGuidelines,
      shareAllTaskOutputs: activeCrew.shareAllTaskOutputs,
      sharedOutputCharLimit: activeCrew.sharedOutputCharLimit,
      defaultProvider: activeCrew.defaultProvider,
      defaultModel: activeCrew.defaultModel,
      providerProfiles: activeCrew.providerProfiles,
      process: activeCrew.process,
      managerAgentId: activeCrew.managerAgentId,
      verbose: activeCrew.verbose,
      maxRpm: activeCrew.maxRpm,
      maxParallelTasks: activeCrew.maxParallelTasks,
      runtimeConfig: activeCrew.runtimeConfig,
      agents: activeCrew.agents,
    })
  }

  const handleImportCrew = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]
    if (!file) return

    try {
      const raw = await file.text()
      const imported = JSON.parse(raw) as {
        name?: unknown
        description?: unknown
        executionGuidelines?: unknown
        outputMode?: unknown
        stopOnFailure?: unknown
        retryCount?: unknown
        managerReviewEnabled?: unknown
        managerReviewGuidelines?: unknown
        shareAllTaskOutputs?: unknown
        sharedOutputCharLimit?: unknown
        defaultProvider?: unknown
        defaultModel?: unknown
        providerProfiles?: unknown
        process?: unknown
        managerAgentId?: unknown
        verbose?: unknown
        maxRpm?: unknown
        maxParallelTasks?: unknown
        runtimeConfig?: unknown
        agents?: unknown
      }
      const newId = crypto.randomUUID()
      const importedName = typeof imported.name === 'string' && imported.name.trim() ? imported.name : 'Importierte Crew'
      createCrew(newId, importedName, [])
      const patch: Parameters<typeof updateCrew>[1] = { tasks: [], status: 'idle' }
      if (typeof imported.description === 'string') patch.description = imported.description
      if (typeof imported.executionGuidelines === 'string') patch.executionGuidelines = imported.executionGuidelines
      if (imported.outputMode === 'standard' || imported.outputMode === 'bullet-report' || imported.outputMode === 'json') {
        patch.outputMode = imported.outputMode
      }
      if (typeof imported.stopOnFailure === 'boolean') patch.stopOnFailure = imported.stopOnFailure
      if (typeof imported.retryCount === 'number') patch.retryCount = imported.retryCount
      if (typeof imported.managerReviewEnabled === 'boolean') patch.managerReviewEnabled = imported.managerReviewEnabled
      if (typeof imported.managerReviewGuidelines === 'string') patch.managerReviewGuidelines = imported.managerReviewGuidelines
      if (typeof imported.shareAllTaskOutputs === 'boolean') patch.shareAllTaskOutputs = imported.shareAllTaskOutputs
      if (typeof imported.sharedOutputCharLimit === 'number') patch.sharedOutputCharLimit = imported.sharedOutputCharLimit
      if (
        imported.defaultProvider === 'ollama'
        || imported.defaultProvider === 'openai-compatible'
        || imported.defaultProvider === 'openrouter'
      ) {
        patch.defaultProvider = imported.defaultProvider
      }
      if (typeof imported.defaultModel === 'string') patch.defaultModel = imported.defaultModel
      if (
        imported.providerProfiles
        && typeof imported.providerProfiles === 'object'
        && imported.providerProfiles !== null
        && 'openAICompatible' in imported.providerProfiles
        && 'openRouter' in imported.providerProfiles
      ) {
        patch.providerProfiles = imported.providerProfiles as any
      }
      if (imported.process === 'sequential' || imported.process === 'parallel' || imported.process === 'hierarchical') {
        patch.process = imported.process
      }
      if (typeof imported.managerAgentId === 'string' || imported.managerAgentId === null) {
        patch.managerAgentId = imported.managerAgentId as any
      }
      if (typeof imported.verbose === 'boolean') patch.verbose = imported.verbose
      if (typeof imported.maxRpm === 'number') patch.maxRpm = imported.maxRpm
      if (typeof imported.maxParallelTasks === 'number') patch.maxParallelTasks = imported.maxParallelTasks
      if (
        imported.runtimeConfig
        && typeof imported.runtimeConfig === 'object'
        && imported.runtimeConfig !== null
        && 'enabled' in imported.runtimeConfig
        && 'baseUrl' in imported.runtimeConfig
        && 'model' in imported.runtimeConfig
        && 'timeoutMs' in imported.runtimeConfig
      ) {
        patch.runtimeConfig = imported.runtimeConfig as any
      }
      if (Array.isArray(imported.agents)) {
        patch.agents = imported.agents.map((agent) => ({
          id: typeof (agent as any)?.id === 'string' ? (agent as any).id : crypto.randomUUID(),
          name: typeof (agent as any)?.name === 'string' ? (agent as any).name : 'Agent',
          role: ROLE_OPTIONS.includes((agent as any)?.role) ? (agent as any).role : 'custom',
          goal: typeof (agent as any)?.goal === 'string' ? (agent as any).goal : '',
          backstory: typeof (agent as any)?.backstory === 'string' ? (agent as any).backstory : '',
          skillsMarkdown: typeof (agent as any)?.skillsMarkdown === 'string' ? (agent as any).skillsMarkdown : '',
          personalityId: typeof (agent as any)?.personalityId === 'string' ? (agent as any).personalityId : null,
          modelOverride: typeof (agent as any)?.modelOverride === 'string' ? (agent as any).modelOverride : null,
          providerKind:
            (agent as any)?.providerKind === 'ollama'
            || (agent as any)?.providerKind === 'openai-compatible'
            || (agent as any)?.providerKind === 'openrouter'
              ? (agent as any).providerKind
              : 'ollama',
          tools: Array.isArray((agent as any)?.tools) ? (agent as any).tools.filter((tool: unknown) => typeof tool === 'string') : [],
          mcpServerNames: Array.isArray((agent as any)?.mcpServerNames) ? (agent as any).mcpServerNames.filter((name: unknown) => typeof name === 'string') : [],
          enabled: typeof (agent as any)?.enabled === 'boolean' ? (agent as any).enabled : true,
          allowDelegation: typeof (agent as any)?.allowDelegation === 'boolean' ? (agent as any).allowDelegation : true,
          verbose: typeof (agent as any)?.verbose === 'boolean' ? (agent as any).verbose : false,
          maxIterations: typeof (agent as any)?.maxIterations === 'number' ? (agent as any).maxIterations : 10,
        }))
      }

      updateCrew(newId, patch)
      setActiveCrew(newId)
      setPendingScrollCrewId(newId)
    } finally {
      event.target.value = ''
    }
  }

  const handleLoadProviderModels = async (providerKey: 'openAICompatible' | 'openRouter') => {
    const config = resolvedActiveProviderConfigs[providerKey]
    if (!config) return

    setProviderModelOptions((current) => ({
      ...current,
      [providerKey]: {
        loading: true,
        endpoint: current[providerKey]?.endpoint,
        models: current[providerKey]?.models ?? [],
      },
    }))

    try {
      const result = await safeInvoke<CrewProviderModelsResult>('crew_provider_models_list', {
        request: {
          providerKind: providerKey === 'openAICompatible' ? 'openai-compatible' : 'openrouter',
          baseUrl: config.baseUrl,
          apiKey: config.apiKey,
        },
      })

      setProviderModelOptions((current) => ({
        ...current,
        [providerKey]: {
          loading: false,
          endpoint: result.endpoint,
          models: result.models,
        },
      }))
    } catch (error) {
      setProviderModelOptions((current) => ({
        ...current,
        [providerKey]: {
          loading: false,
          endpoint: config.baseUrl,
          models: current[providerKey]?.models ?? [],
          error: error instanceof Error ? error.message : String(error),
        },
      }))
    }
  }

  const toggleSection = (key: string) => setOpenSections((s) => ({ ...s, [key]: !s[key] }))
  const toggleAgent = (id: string) => setExpandedAgents((s) => ({ ...s, [id]: !s[id] }))
  const roleEmoji = (role: AgentRole) => {
    const map: Record<AgentRole, string> = { researcher: 'ðŸ”', writer: 'âœï¸', reviewer: 'ðŸ”Ž', planner: 'ðŸ“‹', executor: 'âš™ï¸', analyst: 'ðŸ“Š', custom: 'ðŸ¤–' }
    return map[role] ?? 'ðŸ¤–'
  }
  const processLabel = (p: CrewProcess) => PROCESS_OPTIONS.find((o) => o.value === p)?.label ?? p

  return (
    <div className="panel">
      {/* Header */}
      <div className="crew-header">
        <div className="crew-header-title">
          <span className="crew-header-icon">ðŸš€</span> Crew AI
        </div>
      </div>
      <input ref={importCrewInputRef} type="file" accept="application/json" style={{ display: 'none' }} onChange={(event) => void handleImportCrew(event)} />

      {/* Toolbar */}
      <div className="crew-toolbar">
        <input
          className="crew-toolbar-input"
          placeholder="Neue Crewâ€¦"
          value={crewName}
          onChange={(event) => setCrewName(event.target.value)}
          onKeyDown={(event) => { if (event.key === 'Enter') handleCreateCrew() }}
        />
        <button type="button" className="crew-toolbar-btn" onClick={handleCreateCrew}>âž• Anlegen</button>
        <button type="button" className="crew-toolbar-btn secondary" onClick={handleDuplicateCrew} disabled={!activeCrew}>ðŸ“‹ Duplizieren</button>
        <button type="button" className="crew-toolbar-btn secondary" onClick={handleExportCrew} disabled={!activeCrew}>ðŸ“¤ Export</button>
        <button type="button" className="crew-toolbar-btn secondary" onClick={() => importCrewInputRef.current?.click()}>ðŸ“¥ Import</button>
      </div>

      {crews.length === 0 ? (
        <div className="crew-empty">
          <div className="crew-empty-icon">ðŸš€</div>
          <div className="crew-empty-text">Noch keine Crew vorhanden. Erstelle deine erste Crew oben.</div>
        </div>
      ) : (
        <div className="crew-grid">
          {/* Left: Crew List */}
          <div className="crew-list">
            {crews.map((crew) => {
              const diag = getCrewDiagnostics(crew, defaultOpenAICompatibleProfile, defaultOpenRouterProfile)
              const enabledCount = crew.agents.filter((a) => a.enabled).length
              return (
                <div key={crew.id} className={`crew-card${activeCrew?.id === crew.id ? ' active' : ''}`} onClick={() => setActiveCrew(crew.id)}>
                  <span className={`crew-card-dot${diag.errors.length > 0 ? ' has-errors' : ''}`} />
                  <div className="crew-card-body">
                    <div className="crew-card-name">{crew.name}</div>
                    <div className="crew-card-meta">
                      <span>{processLabel(crew.process)}</span>
                      <span>Â·</span>
                      <span>{enabledCount}/{crew.agents.length} aktiv</span>
                      {diag.errors.length > 0 && <><span>Â·</span><span style={{ color: 'var(--danger)' }}>{diag.errors.length} Blocker</span></>}
                    </div>
                  </div>
                  <button type="button" className="crew-card-delete" onClick={(e) => { e.stopPropagation(); deleteCrew(crew.id) }}>âœ•</button>
                </div>
              )
            })}
          </div>

          {/* Right: Detail */}
          <div className="crew-detail" ref={activeCrewDetailsRef}>
            {activeCrew ? (
              <>
                {/* Section: Allgemein */}
                <div className={`crew-section${openSections.general ? ' open' : ''}`}>
                  <button type="button" className="crew-section-header" onClick={() => toggleSection('general')}>
                    <span className="crew-section-icon">ðŸ“</span> Allgemein
                    <span className="crew-section-chevron">â–¼</span>
                  </button>
                  {openSections.general && (
                    <div className="crew-section-body">
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">Crew-Name</span>
                          <input className="crew-input" value={activeCrew.name} onChange={(e) => updateActiveCrew({ name: e.target.value })} />
                        </div>
                        <div className="crew-form-group">
                          <span className="crew-label">AusfÃ¼hrungsmodus</span>
                          <select className="crew-select" value={activeCrew.process} onChange={(e) => updateActiveCrew({ process: e.target.value as CrewProcess })}>
                            {PROCESS_OPTIONS.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
                          </select>
                        </div>
                      </div>
                      <div className="crew-form-group full-width">
                        <span className="crew-label">Beschreibung</span>
                        <textarea className="crew-textarea" value={activeCrew.description} onChange={(e) => updateActiveCrew({ description: e.target.value })} />
                      </div>
                      <div className="crew-form-group full-width">
                        <span className="crew-label">Crew-Zusatzanweisungen</span>
                        <textarea className="crew-textarea" value={activeCrew.executionGuidelines} onChange={(e) => updateActiveCrew({ executionGuidelines: e.target.value })} placeholder="z. B. Antworte mit Risiken, Annahmen und nÃ¤chsten Schritten" />
                      </div>
                    </div>
                  )}
                </div>

                {/* Section: AusfÃ¼hrung */}
                <div className={`crew-section${openSections.execution ? ' open' : ''}`}>
                  <button type="button" className="crew-section-header" onClick={() => toggleSection('execution')}>
                    <span className="crew-section-icon">âš¡</span> AusfÃ¼hrung
                    <span className="crew-section-chevron">â–¼</span>
                  </button>
                  {openSections.execution && (
                    <div className="crew-section-body">
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">Manager-Agent</span>
                          <select className="crew-select" value={activeCrew.managerAgentId ?? ''} onChange={(e) => updateActiveCrew({ managerAgentId: e.target.value || null })}>
                            <option value="">Keiner</option>
                            {activeCrew.agents.map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
                          </select>
                        </div>
                        <div className="crew-form-group">
                          <span className="crew-label">Ausgabeformat</span>
                          <select className="crew-select" value={activeCrew.outputMode} onChange={(e) => updateActiveCrew({ outputMode: e.target.value as CrewOutputMode })}>
                            <option value="standard">Standard</option>
                            <option value="bullet-report">Stichpunkt-Report</option>
                            <option value="json">JSON</option>
                          </select>
                        </div>
                      </div>
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">Max RPM</span>
                          <input className="crew-input" type="number" min={1} max={600} value={activeCrew.maxRpm} onChange={(e) => updateActiveCrew({ maxRpm: Number(e.target.value) || 1 })} />
                        </div>
                        <div className="crew-form-group">
                          <span className="crew-label">Max parallele Tasks</span>
                          <input className="crew-input" type="number" min={1} max={24} value={activeCrew.maxParallelTasks} onChange={(e) => updateActiveCrew({ maxParallelTasks: Number(e.target.value) || 1 })} />
                        </div>
                      </div>
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">Retry-Versuche pro Task</span>
                          <input className="crew-input" type="number" min={0} max={5} value={activeCrew.retryCount} onChange={(e) => updateActiveCrew({ retryCount: Math.max(0, Number(e.target.value) || 0) })} />
                        </div>
                        <div className="crew-form-group">
                          <span className="crew-label">Zeichenlimit geteilte Ergebnisse</span>
                          <input className="crew-input" type="number" min={0} max={50000} value={activeCrew.sharedOutputCharLimit} onChange={(e) => updateActiveCrew({ sharedOutputCharLimit: Math.max(0, Number(e.target.value) || 0) })} />
                        </div>
                      </div>
                      <label className="crew-checkbox-label"><input type="checkbox" checked={activeCrew.verbose} onChange={(e) => updateActiveCrew({ verbose: e.target.checked })} /> Verbose Crew-Logging aktivieren</label>
                      <label className="crew-checkbox-label"><input type="checkbox" checked={activeCrew.stopOnFailure} onChange={(e) => updateActiveCrew({ stopOnFailure: e.target.checked })} /> Crew bei Task-Fehler sofort stoppen</label>
                      <label className="crew-checkbox-label"><input type="checkbox" checked={activeCrew.managerReviewEnabled} onChange={(e) => updateActiveCrew({ managerReviewEnabled: e.target.checked })} /> Manager-Review nach Task-Batches aktivieren</label>
                      {activeCrew.managerReviewEnabled && (
                        <div className="crew-form-group">
                          <span className="crew-label">Manager-Review-Anweisungen</span>
                          <textarea className="crew-textarea" value={activeCrew.managerReviewGuidelines} onChange={(e) => updateActiveCrew({ managerReviewGuidelines: e.target.value })} placeholder="z. B. Beurteile Risiken strenger und eskaliere frÃ¼hzeitig" />
                        </div>
                      )}
                      <label className="crew-checkbox-label"><input type="checkbox" checked={activeCrew.shareAllTaskOutputs} onChange={(e) => updateActiveCrew({ shareAllTaskOutputs: e.target.checked })} /> Vorherige Task-Ergebnisse global als Kontext teilen</label>
                    </div>
                  )}
                </div>

                {/* Section: Provider & Modell */}
                <div className={`crew-section${openSections.provider ? ' open' : ''}`}>
                  <button type="button" className="crew-section-header" onClick={() => toggleSection('provider')}>
                    <span className="crew-section-icon">ðŸ”Œ</span> Provider & Modell
                    <span className="crew-section-chevron">â–¼</span>
                  </button>
                  {openSections.provider && (
                    <div className="crew-section-body">
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">Crew-Provider</span>
                          <select className="crew-select" value={activeCrew.defaultProvider || 'ollama'} onChange={(e) => handleCrewDefaultProviderChange(e.target.value as CrewProviderKind)}>
                            {PROVIDER_OPTIONS.map((o) => {
                              const ok = o.value === activeCrew.defaultProvider || isProviderEnabledForCrew(o.value)
                              return <option key={o.value} value={o.value} disabled={!ok}>{ok ? o.label : `${o.label} (Profil aktivieren)`}</option>
                            })}
                          </select>
                          <span className="crew-hint">Mitglieder mit eigenem Provider/Modell bleiben beim Umschalten unverÃ¤ndert.</span>
                        </div>
                        <div className="crew-form-group">
                          <span className="crew-label">Crew-Modell</span>
                          <select className="crew-select" value={activeCrew.defaultModel || ''} onChange={(e) => updateActiveCrew({ defaultModel: e.target.value })}>
                            <option value="">Globale Einstellungen verwenden</option>
                            {getCrewDefaultModelOptions().map((m) => <option key={m} value={m}>{m}</option>)}
                          </select>
                          {activeCrew.defaultProvider !== 'ollama' && getCrewDefaultModelOptions().length === 0 && (
                            <span className="crew-hint">FÃ¼r diesen Provider sind noch keine Modelle geladen.</span>
                          )}
                          <span className="crew-hint">Gilt automatisch fÃ¼r alle Mitglieder ohne eigenes Modell-Override.</span>
                        </div>
                      </div>
                    </div>
                  )}
                </div>

                {/* Diagnostics */}
                <div className={`crew-section${openSections.diagnostics ? ' open' : ''}`}>
                  <button type="button" className="crew-section-header" onClick={() => toggleSection('diagnostics')}>
                    <span className="crew-section-icon">ðŸ©º</span> Diagnostik
                    <span className="crew-section-chevron">â–¼</span>
                  </button>
                  {openSections.diagnostics && (
                    <div className="crew-section-body">
                      {activeCrewDiagnostics.errors.length === 0 && activeCrewDiagnostics.warnings.length === 0 ? (
                        <div className="crew-alert success"><span className="crew-alert-icon">âœ…</span> Keine Blocker gefunden. Crew ist startbereit.</div>
                      ) : (
                        <>
                          {activeCrewDiagnostics.errors.map((entry) => (
                            <div key={`e-${entry}`} className="crew-alert error"><span className="crew-alert-icon">ðŸš«</span> {entry}</div>
                          ))}
                          {activeCrewDiagnostics.warnings.map((entry) => (
                            <div key={`w-${entry}`} className="crew-alert warning"><span className="crew-alert-icon">âš ï¸</span> {entry}</div>
                          ))}
                        </>
                      )}
                    </div>
                  )}
                </div>

                {/* Agents */}
                <div className="crew-section open">
                  <div className="crew-section-header" style={{ cursor: 'default' }}>
                    <span className="crew-section-icon">ðŸ‘¥</span> Crew-Mitglieder ({activeCrew.agents.filter((a) => a.enabled).length}/{activeCrew.agents.length})
                  </div>
                  <div className="crew-section-body">
                    <div className="crew-agents-list">
                      {activeCrew.agents.map((agent) => {
                        const pmc = getProviderModelCatalog(agent.providerKind)
                        const amo = getAgentModelOptions(agent)
                        const selModel = agent.modelOverride?.trim() && amo.includes(agent.modelOverride.trim()) ? agent.modelOverride.trim() : ''
                        const isOpen = expandedAgents[agent.id] ?? false
                        return (
                          <div key={agent.id} className={`crew-agent-card${!agent.enabled ? ' disabled' : ''}${isOpen ? ' open' : ''}`}>
                            <div className="crew-agent-header" onClick={() => toggleAgent(agent.id)}>
                              <div className="crew-agent-avatar">{roleEmoji(agent.role)}</div>
                              <div className="crew-agent-info">
                                <div className="crew-agent-name">{agent.name}</div>
                                <div className="crew-agent-role">{agent.role} Â· {agent.providerKind}</div>
                              </div>
                              <span className={`crew-badge ${agent.enabled ? 'active' : 'inactive'}`}>{agent.enabled ? 'Aktiv' : 'Inaktiv'}</span>
                              <span className="crew-agent-chevron">â–¼</span>
                            </div>
                            {isOpen && (
                              <div className="crew-agent-body">
                                <div className="crew-agent-col">
                                  <div className="crew-form-group"><span className="crew-label">Name</span><input className="crew-input" value={agent.name} onChange={(e) => updateActiveCrewAgent(agent.id, { name: e.target.value })} /></div>
                                  <div className="crew-form-group"><span className="crew-label">Rolle</span><select className="crew-select" value={agent.role} onChange={(e) => updateActiveCrewAgent(agent.id, { role: e.target.value as AgentRole })}>{ROLE_OPTIONS.map((r) => <option key={r} value={r}>{r}</option>)}</select></div>
                                  <div className="crew-form-group"><span className="crew-label">Ziel / Prompt-Fokus</span><textarea className="crew-textarea" value={agent.goal} onChange={(e) => updateActiveCrewAgent(agent.id, { goal: e.target.value })} /></div>
                                  <div className="crew-form-group"><span className="crew-label">Hintergrund</span><textarea className="crew-textarea" value={agent.backstory} onChange={(e) => updateActiveCrewAgent(agent.id, { backstory: e.target.value })} /></div>
                                  <div className="crew-form-group"><span className="crew-label">skills.md</span><textarea className="crew-textarea" value={agent.skillsMarkdown} onChange={(e) => updateActiveCrewAgent(agent.id, { skillsMarkdown: e.target.value })} placeholder="# skills.md&#10;- Projektkontext&#10;- Arbeitsstil" /></div>
                                </div>
                                <div className="crew-agent-col">
                                  <label className="crew-checkbox-label"><input type="checkbox" checked={agent.enabled} onChange={(e) => updateActiveCrewAgent(agent.id, { enabled: e.target.checked })} /> Aktiviert</label>
                                  <div className="crew-form-group">
                                    <span className="crew-label">Provider</span>
                                    <select className="crew-select" value={agent.providerKind} onChange={(e) => handleAgentProviderKindChange(agent.id, e.target.value as CrewProviderKind)}>
                                      {PROVIDER_OPTIONS.map((o) => { const ok = o.value === agent.providerKind || isProviderEnabledForCrew(o.value); return <option key={o.value} value={o.value} disabled={!ok}>{ok ? o.label : `${o.label} (aktivieren)`}</option> })}
                                    </select>
                                    {agent.providerKind !== 'ollama' && !isProviderEnabledForCrew(agent.providerKind) && <span className="crew-hint" style={{ color: 'var(--danger)' }}>Provider nicht aktiviert.</span>}
                                  </div>
                                  <div className="crew-form-group">
                                    <span className="crew-label">Modell</span>
                                    <select className="crew-select" value={selModel} onChange={(e) => updateActiveCrewAgent(agent.id, { modelOverride: e.target.value || null })}>
                                      <option value="">Crew-Modell ({getCrewDefaultModelLabel(agent.providerKind)})</option>
                                      {amo.map((m) => <option key={m} value={m}>{m}</option>)}
                                    </select>
                                    {agent.providerKind !== 'ollama' && pmc.models.length === 0 && <span className="crew-hint">Keine Modelle geladen.</span>}
                                  </div>
                                  <div className="crew-form-group"><span className="crew-label">Max Iterationen</span><input className="crew-input" type="number" min={1} max={100} value={agent.maxIterations} onChange={(e) => updateActiveCrewAgent(agent.id, { maxIterations: Number(e.target.value) || 1 })} /></div>
                                  <label className="crew-checkbox-label"><input type="checkbox" checked={agent.allowDelegation} onChange={(e) => updateActiveCrewAgent(agent.id, { allowDelegation: e.target.checked })} /> Delegation erlaubt</label>
                                  <label className="crew-checkbox-label"><input type="checkbox" checked={agent.verbose} onChange={(e) => updateActiveCrewAgent(agent.id, { verbose: e.target.checked })} /> Verbose Logs</label>
                                </div>
                                <div className="crew-agent-col">
                                  <div className="crew-form-group">
                                    <span className="crew-label">Tools</span>
                                    <div className="crew-tool-list">
                                      {claudeTools.map((tool) => (<label key={tool.id} className="crew-tool-item"><input type="checkbox" checked={agent.tools.includes(tool.id)} onChange={() => updateActiveCrewAgent(agent.id, { tools: toggleStringValue(agent.tools, tool.id) })} />{tool.label}</label>))}
                                    </div>
                                  </div>
                                  <div className="crew-form-group">
                                    <span className="crew-label">MCP-Zugriffe</span>
                                    {configuredMcpServers.length === 0 ? <span className="crew-hint">Keine MCP-Server konfiguriert.</span> : (
                                      <div className="crew-tool-list">
                                        {configuredMcpServers.map((srv) => (<label key={srv.name} className="crew-tool-item"><input type="checkbox" checked={agent.mcpServerNames.includes(srv.name)} onChange={() => updateActiveCrewAgent(agent.id, { mcpServerNames: toggleStringValue(agent.mcpServerNames, srv.name) })} />{srv.name}</label>))}
                                      </div>
                                    )}
                                  </div>
                                </div>
                              </div>
                            )}
                          </div>
                        )
                      })}
                    </div>
                  </div>
                </div>

                {/* Tasks hint */}
                <div className="crew-tasks-hint">
                  <span className="crew-tasks-hint-icon">ðŸ“‹</span>
                  Tasks werden unter /tasks erstellt, ausgefÃ¼hrt und geplant.
                </div>
              </>
            ) : (
              <div className="crew-empty">
                <div className="crew-empty-icon">ðŸ‘ˆ</div>
                <div className="crew-empty-text">WÃ¤hle eine Crew aus der Liste aus.</div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  )
}

