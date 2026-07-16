import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useConfigStore, type OllamaConfig } from '../stores/configStore'
import { useCoworkStore } from '../stores/coworkStore'
import { resolveCrewAgentWithProfile, useCrewStore, type AgentRole, type CrewAgent, type CrewExternalProviderConfig, type CrewOutputMode, type CrewPersonalityProfile, type CrewProcess, type CrewProviderKind, type CrewProviderProfiles, type CrewRuntimeConfig } from '../stores/crewStore'
import { usePersonalityStore } from '../stores/personalityStore'
import CrewControlPlanePanel from './crew/CrewControlPlanePanel'
import CrewGovernancePanel from './crew/CrewGovernancePanel'
import CrewHistoryPanel from './crew/CrewHistoryPanel'
import CrewRuntimePanel from './crew/CrewRuntimePanel'
import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'
import { tr } from '../i18n'
import { ArrowRight, ChevronDown, ListCollapse, ListTree, MousePointerClick, Settings2, Trash2, UsersRound, Workflow } from 'lucide-react'

const ROLE_OPTIONS: AgentRole[] = ['researcher', 'writer', 'reviewer', 'planner', 'executor', 'analyst', 'custom']
const PROCESS_OPTIONS: Array<{ value: CrewProcess; label: string }> = [
  { value: 'sequential', label: 'Sequenziell' },
  { value: 'parallel', label: 'Parallel' },
  { value: 'hierarchical', label: 'Hierarchisch' },
]
const PROVIDER_OPTIONS: Array<{ value: CrewProviderKind; label: string }> = [
  { value: 'ollama', label: 'Ollama' },
  { value: 'openai-compatible', label: 'OpenAI-compatible' },
  { value: 'openrouter', label: 'OpenRouter' },
]

const CREW_STARTER_PRESETS = [
  {
    id: 'research',
    name: 'Research squad',
    description: 'Find reliable evidence and turn it into a clear recommendation.',
  },
  {
    id: 'build',
    name: 'Build crew',
    description: 'Plan, create, and verify a complete working deliverable.',
  },
  {
    id: 'review',
    name: 'Review council',
    description: 'Audit an existing result, close gaps, and sign off the final output.',
  },
] as const

type ProviderModelState = {
  loading: boolean
  endpoint?: string
  models: string[]
  error?: string
  cacheKey?: string
}

type ImportedCrewAgent = {
  id?: unknown
  name?: unknown
  role?: unknown
  goal?: unknown
  backstory?: unknown
  skillsMarkdown?: unknown
  personalityId?: unknown
  modelOverride?: unknown
  providerKind?: unknown
  tools?: unknown
  mcpServerNames?: unknown
  enabled?: unknown
  allowDelegation?: unknown
  verbose?: unknown
  maxIterations?: unknown
}

type ImportedCrewPayload = {
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

function toggleStringValue(values: string[], nextValue: string): string[] {
  return values.includes(nextValue)
    ? values.filter((value) => value !== nextValue)
    : [...values, nextValue]
}

function setStringValue(values: string[], nextValue: string, enabled: boolean): string[] {
  if (enabled) {
    return values.includes(nextValue) ? values : [...values, nextValue]
  }

  return values.filter((value) => value !== nextValue)
}

function getProviderLabel(providerKind: CrewProviderKind): string {
  return PROVIDER_OPTIONS.find((option) => option.value === providerKind)?.label ?? providerKind
}

function isObjectRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function isCrewProviderProfiles(value: unknown): value is CrewProviderProfiles {
  return isObjectRecord(value)
    && isObjectRecord(value.openAICompatible)
    && isObjectRecord(value.openRouter)
}

function isCrewRuntimeConfig(value: unknown): value is CrewRuntimeConfig {
  return isObjectRecord(value)
    && typeof value.enabled === 'boolean'
    && typeof value.baseUrl === 'string'
    && typeof value.model === 'string'
    && typeof value.timeoutMs === 'number'
}

function isAgentRole(value: unknown): value is AgentRole {
  return ROLE_OPTIONS.includes(value as AgentRole)
}

function isCrewProviderKind(value: unknown): value is CrewProviderKind {
  return value === 'ollama' || value === 'openai-compatible' || value === 'openrouter'
}

function toStringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : []
}

function normalizeImportedCrewAgent(value: unknown): CrewAgent {
  const agent = isObjectRecord(value) ? value as ImportedCrewAgent : {}
  return {
    id: typeof agent.id === 'string' ? agent.id : crypto.randomUUID(),
    name: typeof agent.name === 'string' ? agent.name : 'Agent',
    role: isAgentRole(agent.role) ? agent.role : 'custom',
    goal: typeof agent.goal === 'string' ? agent.goal : '',
    backstory: typeof agent.backstory === 'string' ? agent.backstory : '',
    skillsMarkdown: typeof agent.skillsMarkdown === 'string' ? agent.skillsMarkdown : '',
    personalityId: typeof agent.personalityId === 'string' ? agent.personalityId : null,
    modelOverride: typeof agent.modelOverride === 'string' ? agent.modelOverride : null,
    providerKind: isCrewProviderKind(agent.providerKind) ? agent.providerKind : 'ollama',
    tools: toStringArray(agent.tools),
    mcpServerNames: toStringArray(agent.mcpServerNames),
    enabled: typeof agent.enabled === 'boolean' ? agent.enabled : true,
    allowDelegation: typeof agent.allowDelegation === 'boolean' ? agent.allowDelegation : true,
    verbose: typeof agent.verbose === 'boolean' ? agent.verbose : false,
    maxIterations: typeof agent.maxIterations === 'number' ? agent.maxIterations : 10,
  }
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
  fallbackConfig: { baseUrl?: string; model?: string; apiKey?: string; verifyTlsCertificates?: boolean } | undefined,
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
    verifyTlsCertificates: (config.verifyTlsCertificates ?? true) && (fallbackConfig?.verifyTlsCertificates ?? true),
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


function AutoResizeTextarea(props: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  const textareaRef = useRef<HTMLTextAreaElement | null>(null)
  const value = typeof props.value === 'string' ? props.value : ''

  useEffect(() => {
    const node = textareaRef.current
    if (!node) return

    node.style.height = '0px'
    node.style.height = `${node.scrollHeight}px`
  }, [value])

  return <textarea {...props} ref={textareaRef} rows={1} />
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
  defaultProvider?: CrewProviderKind
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
    errors.push('No active Crew-members available.')
  }

  const crewDefaultProvider = crew.defaultProvider ?? 'ollama'
  const openAiAgents = crewDefaultProvider === 'openai-compatible' ? enabledAgents : []
  if (openAiAgents.length > 0) {
    const profile = crew.providerProfiles?.openAICompatible
    const effectiveApiKey = profile?.apiKey.trim() || openAIProfile?.apiKey?.trim() || ''
    const effectiveBaseUrl = profile?.baseUrl.trim() || openAIProfile?.baseUrl?.trim() || ''
    const effectiveModel = profile?.model.trim() || openAIProfile?.model?.trim() || ''
    const needsFallbackModel = openAiAgents.some((agent) => !agent.modelOverride?.trim())
    if (!profile?.enabled) {
      errors.push('Active crew members use OpenAI-compatible routing, but the crew profile is not enabled.')
    }
    if (!effectiveApiKey) {
      errors.push('OpenAI-compatible crew profile has no API key.')
    }
    if (!effectiveBaseUrl) {
      errors.push('OpenAI-compatible crew profile has no endpoint.')
    }
    if (needsFallbackModel && !effectiveModel) {
      errors.push('OpenAI-compatible crew profile has no model and at least one agent has no model override.')
    }
  }

  const openRouterAgents = crewDefaultProvider === 'openrouter' ? enabledAgents : []
  if (openRouterAgents.length > 0) {
    const profile = crew.providerProfiles?.openRouter
    const effectiveApiKey = profile?.apiKey.trim() || openRouterProfile?.apiKey?.trim() || ''
    const effectiveBaseUrl = profile?.baseUrl.trim() || openRouterProfile?.baseUrl?.trim() || 'https://openrouter.ai/api/v1'
    const effectiveModel = profile?.model.trim() || openRouterProfile?.model?.trim() || ''
    const needsFallbackModel = openRouterAgents.some((agent) => !agent.modelOverride?.trim())
    if (!profile?.enabled) {
      errors.push('Active crew members use OpenRouter, but the crew profile is not enabled.')
    }
    if (!effectiveApiKey) {
      errors.push('OpenRouter crew profile has no API key.')
    }
    if (!effectiveBaseUrl) {
      errors.push('OpenRouter crew profile has no endpoint.')
    }
    if (needsFallbackModel && !effectiveModel) {
      errors.push('OpenRouter crew profile has no model and at least one agent has no model override.')
    }
  }

  if (crew.process === 'hierarchical') {
    if (!crew.managerAgentId) {
      errors.push('Hierarchical crew requires a manager agent.')
    } else if (!enabledAgentIds.has(crew.managerAgentId)) {
      errors.push('The selected manager agent is disabled or missing.')
    }
  }

  return { errors, warnings }
}

export default function CrewPanel() {
  const navigate = useNavigate()
  const {
    crews,
    activeCrewId,
    createCrew,
    createStarterCrew,
    updateCrew,
    setCrewProviderProfiles,
    deleteCrew,
    setActiveCrew,
    loadAgents,
    installDefaultAgents,
    syncAgentsFromPersonalityProfiles,
    migrateAgentsToPersonalityProfiles,
    updateCrewAgent,
    runCrew,
    stopCrew,
  } = useCrewStore()
  const { availableModels, defaultLlmProfileIds, llmProfiles, mcpServer, mcpServers, ollama } = useConfigStore()
  const claudeTools = useCoworkStore((state) => state.claudeTools)
  const personalities = usePersonalityStore((state) => state.personalities)
  const loadPersonalities = usePersonalityStore((state) => state.loadPersonalities)
  const upsertPersonality = usePersonalityStore((state) => state.upsertPersonality)

  const [crewName, setCrewName] = useState('')
  const [providerModelOptions, setProviderModelOptions] = useState<Record<string, ProviderModelState>>({})
  const [pendingScrollCrewId, setPendingScrollCrewId] = useState<string | null>(null)
  const [openSections, setOpenSections] = useState<Record<string, boolean>>({ general: true, execution: false, provider: false, diagnostics: false, members: false })
  const [expandedAgents, setExpandedAgents] = useState<Record<string, boolean>>({})
  const [isCrewListVisible, setIsCrewListVisible] = useState(() => typeof window === 'undefined' ? true : window.innerWidth >= 1320)
  const importCrewInputRef = useRef<HTMLInputElement | null>(null)
  const crewNameInputRef = useRef<HTMLInputElement | null>(null)
  const activeCrewDetailsRef = useRef<HTMLDivElement | null>(null)
  const diagnosticsHeaderRef = useRef<HTMLButtonElement | null>(null)
  const providerCredentialTokensRef = useRef(new Map<string, string>())

  useEffect(() => {
    loadAgents()
    installDefaultAgents()
  }, [installDefaultAgents, loadAgents])

  useEffect(() => {
    void loadPersonalities()
  }, [loadPersonalities])

  const personalityProfiles = useMemo<CrewPersonalityProfile[]>(() => (
    personalities.map((personality) => ({
      id: personality.id,
      name: personality.name,
      description: personality.description,
      role: personality.role,
      goal: personality.goal || personality.description,
      systemPrompt: personality.system_prompt,
      skillsMarkdown: personality.skills_markdown,
      modelOverride: personality.model_override,
      temperature: personality.temperature,
      icon: personality.icon,
      isDefault: personality.is_default,
    }))
  ), [personalities])

  useEffect(() => {
    if (personalityProfiles.length === 0) return

    syncAgentsFromPersonalityProfiles(personalityProfiles)
  }, [personalityProfiles, syncAgentsFromPersonalityProfiles])

  useEffect(() => {
    if (!hasTauriRuntime() || personalityProfiles.length === 0) return

    void migrateAgentsToPersonalityProfiles(personalityProfiles).then((changed) => {
      if (changed) {
        void loadPersonalities()
      }
    })
  }, [loadPersonalities, migrateAgentsToPersonalityProfiles, personalityProfiles])

  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return

    const mediaQuery = window.matchMedia('(max-width: 1320px)')
    const syncCrewListVisibility = (event?: MediaQueryListEvent) => {
      const matches = event?.matches ?? mediaQuery.matches
      if (matches) {
        setIsCrewListVisible(false)
      }
    }

    syncCrewListVisibility()
    mediaQuery.addEventListener('change', syncCrewListVisibility)

    return () => mediaQuery.removeEventListener('change', syncCrewListVisibility)
  }, [])

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
  const configuredMcpServers = (mcpServers.length > 0 ? mcpServers : [mcpServer]).filter((server) => server.name.trim())

  const getProviderModelCacheKey = (providerKey: 'openAICompatible' | 'openRouter') => {
    const config = resolvedActiveProviderConfigs[providerKey]
    if (!config) return ''
    const secret = config.apiKey.trim()
    let credentialToken = providerCredentialTokensRef.current.get(secret)
    if (!credentialToken) {
      credentialToken = crypto.randomUUID()
      providerCredentialTokensRef.current.set(secret, credentialToken)
    }
    return `${config.baseUrl.trim()}::${credentialToken}::${config.verifyTlsCertificates}`
  }

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
      return resolvedActiveCrewConfig.model || ollama.model || 'not set'
    }

    if (providerKind === 'openai-compatible') {
      return resolvedActiveProviderConfigs.openAICompatible?.model || defaultOpenAICompatibleProfile?.model || 'not set'
    }

    return resolvedActiveProviderConfigs.openRouter?.model || 'not set'
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

  const getAgentModelOptions = (agent: { modelOverride?: string | null }) => {
    const catalog = getProviderModelCatalog(activeCrew?.defaultProvider || 'ollama')
    const modelOverride = agent.modelOverride?.trim()

    if (!modelOverride || catalog.authoritative || catalog.models.includes(modelOverride)) {
      return catalog.models
    }

    return [modelOverride, ...catalog.models]
  }

  const handleCrewDefaultProviderChange = (providerKind: CrewProviderKind) => {
    if (providerKind !== 'ollama' && !isProviderEnabledForCrew(providerKind)) return

    updateActiveCrew({
      defaultProvider: providerKind,
      defaultModel: '',
      agents: activeCrew?.agents.map((agent) => ({
        ...agent,
        providerKind,
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

  useEffect(() => {
    setProviderModelOptions({})
  }, [activeCrew?.id])

  useEffect(() => {
    if (!activeCrew) return

    const providerKind = activeCrew.defaultProvider || 'ollama'
    const providerKey = getProviderKey(providerKind)
    if (!providerKey || !isProviderEnabledForCrew(providerKind)) return

    const current = providerModelOptions[providerKey]
    const cacheKey = getProviderModelCacheKey(providerKey)
    if (current?.loading) return
    if (current?.cacheKey === cacheKey && (current.endpoint || current.error)) return

    void handleLoadProviderModels(providerKey)
    // Provider loaders are render-local; this effect tracks their store inputs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeCrew, providerModelOptions, resolvedActiveProviderConfigs])

  useEffect(() => {
    if (!pendingScrollCrewId || activeCrew?.id !== pendingScrollCrewId) return
    const activeCrewDetails = activeCrewDetailsRef.current
    if (typeof activeCrewDetails?.scrollIntoView === 'function') {
      activeCrewDetails.scrollIntoView({ behavior: 'smooth', block: 'start' })
    }
    setPendingScrollCrewId(null)
  }, [activeCrew?.id, pendingScrollCrewId])

  const updateCrewPersonalityProfile = useCallback((profile: CrewPersonalityProfile, patch: Partial<CrewPersonalityProfile>) => {
    const next = {
      ...profile,
      ...patch,
    }
    void upsertPersonality({
      id: next.id,
      name: next.name,
      description: next.goal || next.description,
      role: next.role,
      goal: next.goal,
      systemPrompt: next.systemPrompt,
      skillsMarkdown: next.skillsMarkdown,
      modelOverride: next.modelOverride,
      temperature: next.temperature ?? null,
      icon: next.icon ?? null,
      isDefault: next.isDefault ?? false,
    })
  }, [upsertPersonality])

  useEffect(() => {
    if (!activeCrew) return

    const invalidAgentIds = activeCrew.agents
      .filter((agent) => {
        const profile = agent.personalityId ? personalityProfiles.find((entry) => entry.id === agent.personalityId) : null
        const effectiveAgent = profile ? resolveCrewAgentWithProfile(agent, [profile]) : agent
        const modelOverride = effectiveAgent.modelOverride?.trim()
        if (!modelOverride) return false

        const catalog = getProviderModelCatalog(activeCrew.defaultProvider || 'ollama')
        return catalog.authoritative && !catalog.models.includes(modelOverride)
      })
      .map((agent) => agent.id)

    if (invalidAgentIds.length === 0) return

    invalidAgentIds.forEach((agentId) => {
      const agent = activeCrew.agents.find((entry) => entry.id === agentId)
      const profile = agent?.personalityId ? personalityProfiles.find((entry) => entry.id === agent.personalityId) : null
      if (profile) {
        updateCrewPersonalityProfile(profile, { modelOverride: null })
        return
      }

      updateCrewAgent(activeCrew.id, agentId, { modelOverride: null })
    })
    // The catalog helper is render-local; this effect tracks its store inputs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeCrew, availableModels, personalityProfiles, providerModelOptions, updateCrewAgent, updateCrewPersonalityProfile])

  const handleCreateCrew = () => {
    const nextName = crewName.trim() || buildDefaultCrewName(crews.map((crew) => crew.name))
    const id = createStarterCrew(nextName, nextName)
    setPendingScrollCrewId(id)
    setCrewName('')
  }

  const handleCreateStarterPreset = (preset: (typeof CREW_STARTER_PRESETS)[number]) => {
    const presetName = tr(preset.name)
    const id = createStarterCrew(presetName, tr(preset.description))
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

  const updateAllActiveCrewAgents = (mapper: (agent: CrewAgent) => CrewAgent) => {
    if (!activeCrew) return
    updateActiveCrew({
      agents: activeCrew.agents.map(mapper),
    })
  }

  const setCrewToolForAllAgents = (toolId: string, enabled: boolean) => {
    updateAllActiveCrewAgents((agent) => ({
      ...agent,
      tools: setStringValue(agent.tools, toolId, enabled),
      allowDelegation: toolId === 'delegate_task' ? enabled : agent.allowDelegation,
    }))
  }

  const setCrewMcpServerForAllAgents = (serverName: string, enabled: boolean) => {
    updateAllActiveCrewAgents((agent) => ({
      ...agent,
      mcpServerNames: setStringValue(agent.mcpServerNames, serverName, enabled),
    }))
  }

  const handleDuplicateCrew = async () => {
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
    await setCrewProviderProfiles(newId, {
      openAICompatible: { ...activeCrew.providerProfiles.openAICompatible },
      openRouter: { ...activeCrew.providerProfiles.openRouter },
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
      providerProfiles: {
        openAICompatible: { ...activeCrew.providerProfiles.openAICompatible, apiKey: '' },
        openRouter: { ...activeCrew.providerProfiles.openRouter, apiKey: '' },
      },
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
      const imported = JSON.parse(raw) as ImportedCrewPayload
      const newId = crypto.randomUUID()
      const importedName = typeof imported.name === 'string' && imported.name.trim() ? imported.name : 'Importierte Crew'
      createCrew(newId, importedName, [])
      const patch: Parameters<typeof updateCrew>[1] = { tasks: [], status: 'idle' }
      let importedProviderProfiles: CrewProviderProfiles | null = null
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
        isCrewProviderProfiles(imported.providerProfiles)
      ) {
        importedProviderProfiles = imported.providerProfiles
      }
      if (imported.process === 'sequential' || imported.process === 'parallel' || imported.process === 'hierarchical') {
        patch.process = imported.process
      }
      if (typeof imported.managerAgentId === 'string' || imported.managerAgentId === null) {
        patch.managerAgentId = imported.managerAgentId
      }
      if (typeof imported.verbose === 'boolean') patch.verbose = imported.verbose
      if (typeof imported.maxRpm === 'number') patch.maxRpm = imported.maxRpm
      if (typeof imported.maxParallelTasks === 'number') patch.maxParallelTasks = imported.maxParallelTasks
      if (
        isCrewRuntimeConfig(imported.runtimeConfig)
      ) {
        patch.runtimeConfig = imported.runtimeConfig
      }
      if (Array.isArray(imported.agents)) {
        patch.agents = imported.agents.map(normalizeImportedCrewAgent)
      }

      updateCrew(newId, patch)
      if (importedProviderProfiles) {
        await setCrewProviderProfiles(newId, importedProviderProfiles)
      }
      setActiveCrew(newId)
      setPendingScrollCrewId(newId)
    } finally {
      event.target.value = ''
    }
  }

  const handleLoadProviderModels = async (providerKey: 'openAICompatible' | 'openRouter') => {
    const config = resolvedActiveProviderConfigs[providerKey]
    if (!config) return
    const cacheKey = getProviderModelCacheKey(providerKey)

    setProviderModelOptions((current) => ({
      ...current,
      [providerKey]: {
        loading: true,
        endpoint: current[providerKey]?.endpoint,
        models: current[providerKey]?.models ?? [],
        cacheKey,
      },
    }))

    try {
      const result = await safeInvoke<CrewProviderModelsResult>('crew_provider_models_list', {
        request: {
          providerKind: providerKey === 'openAICompatible' ? 'openai-compatible' : 'openrouter',
          baseUrl: config.baseUrl,
          apiKey: config.apiKey,
          model: config.model,
          verifyTlsCertificates: config.verifyTlsCertificates,
        },
      })

      setProviderModelOptions((current) => ({
        ...current,
        [providerKey]: {
          loading: false,
          endpoint: result.endpoint,
          models: result.models,
          cacheKey,
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
          cacheKey,
        },
      }))
    }
  }

  const toggleSection = (key: string) => setOpenSections((s) => ({ ...s, [key]: !s[key] }))
  const toggleAgent = (id: string) => setExpandedAgents((s) => ({ ...s, [id]: !s[id] }))
  const toggleCrewListVisibility = () => setIsCrewListVisible((current) => !current)
  const roleEmoji = (role: AgentRole) => {
    const map: Record<AgentRole, string> = { researcher: 'RE', writer: 'WR', reviewer: 'RV', planner: 'PL', executor: 'EX', analyst: 'AN', custom: 'AG' }
    return map[role] ?? 'AG'
  }
  const processLabel = (p: CrewProcess) => PROCESS_OPTIONS.find((o) => o.value === p)?.label ?? p
  const activeAgentCount = activeCrew?.agents.filter((agent) => agent.enabled).length ?? 0
  const profileBackedAgentCount = activeCrew?.agents.filter((agent) => Boolean(agent.personalityId)).length ?? 0
  const configuredToolCount = activeCrew ? new Set(activeCrew.agents.flatMap((agent) => agent.tools)).size : 0
  const configuredMcpCount = activeCrew ? new Set(activeCrew.agents.flatMap((agent) => agent.mcpServerNames)).size : 0
  const activeCrewNeedsMission = Boolean(activeCrew && activeCrew.tasks.length === 0)
  const activeCrewBlockerCount = activeCrewDiagnostics.errors.length + (activeCrewNeedsMission ? 1 : 0)
  const activeCrewHasProviderBlocker = activeCrewDiagnostics.errors.some((entry) => (
    entry.includes('OpenRouter') || entry.includes('OpenAI-compatible')
  ))
  const activeCrewProviderSettingsPath = activeCrewDiagnostics.errors.some((entry) => entry.includes('OpenRouter'))
    ? '/settings?provider=openrouter'
    : activeCrewDiagnostics.errors.some((entry) => entry.includes('OpenAI-compatible'))
      ? '/settings?provider=openai-compatible'
      : '/settings'
  const outputModeLabel = activeCrew?.outputMode === 'bullet-report'
    ? 'Stichpunkte'
    : activeCrew?.outputMode === 'json'
      ? 'JSON'
      : 'Standard'
  const reviewActiveCrewBlockers = () => {
    setOpenSections((sections) => ({ ...sections, diagnostics: true }))
    diagnosticsHeaderRef.current?.scrollIntoView?.({ behavior: 'smooth', block: 'start' })
    diagnosticsHeaderRef.current?.focus({ preventScroll: true })
  }

  return (
    <div className="panel crew-shell">
      <div className="crew-shell-top">
        <div className="crew-header">
          <div className="crew-header-copy">
            <div className="crew-header-eyebrow">{tr("Crew Workspace")}</div>
            <div className="crew-header-title">
              <span className="crew-header-icon" aria-hidden="true"><UsersRound size={24} strokeWidth={1.9} /></span>
              <span>{tr("Crew AI")}</span>
            </div>
            <div className="crew-header-subtitle">{tr("Plan roles, governance, and reproducible runs in a clean workspace instead of cramped individual cards.")}</div>
          </div>
          <div className="crew-header-badge" aria-label={tr("Crew-Uebersicht")}>
            <strong>{crews.length}</strong>
            <span>{tr(crews.length === 1 ? 'Crew configured' : 'Crews configured')}</span>
          </div>
        </div>
      </div>
      <input ref={importCrewInputRef} type="file" accept="application/json" style={{ display: 'none' }} onChange={(event) => void handleImportCrew(event)} />

      <div className="crew-toolbar">
        <div className="crew-toolbar-primary">
          <input
            ref={crewNameInputRef}
            className="crew-toolbar-input"
            placeholder={tr("New crew...")}
            value={crewName}
            onChange={(event) => setCrewName(event.target.value)}
            onKeyDown={(event) => { if (event.key === 'Enter') handleCreateCrew() }}
          />
          <button type="button" className="crew-toolbar-btn" onClick={handleCreateCrew}>{tr("Create")}</button>
        </div>
        <div className="crew-toolbar-actions">
          {activeCrew?.status === 'running' || activeCrew?.status === 'awaiting-approval' ? (
            <button type="button" className="crew-toolbar-btn danger" onClick={() => void stopCrew(activeCrew.id)}>{tr('Stop run')}</button>
          ) : (
            <button
              type="button"
              className="crew-toolbar-btn run"
              disabled={!activeCrew || activeCrew.tasks.length === 0 || activeCrewDiagnostics.errors.length > 0}
              onClick={() => activeCrew && void runCrew(activeCrew.id)}
            >
              {activeCrew?.status === 'completed' || activeCrew?.status === 'failed' || activeCrew?.status === 'canceled' ? tr('Run again') : tr('Run crew')}
            </button>
          )}
          <button type="button" className="crew-toolbar-btn secondary" onClick={handleDuplicateCrew} disabled={!activeCrew}>{tr("Duplicate")}</button>
          <button type="button" className="crew-toolbar-btn secondary" onClick={handleExportCrew} disabled={!activeCrew}>{tr("Export")}</button>
          <button type="button" className="crew-toolbar-btn secondary" onClick={() => importCrewInputRef.current?.click()}>{tr("Import")}</button>
          <button type="button" className="crew-toolbar-btn secondary crew-toolbar-toggle" aria-pressed={isCrewListVisible} onClick={toggleCrewListVisibility}>
            {isCrewListVisible ? <ListCollapse size={15} aria-hidden="true" /> : <ListTree size={15} aria-hidden="true" />}
            {isCrewListVisible ? tr('Hide list') : tr('Show list')}
          </button>
        </div>
      </div>

      {activeCrew && (
        <div className="crew-active-compact" aria-label={tr('Crew launch checklist')}>
          <div className="crew-active-compact-main">
            <span className={`crew-card-dot${activeCrewBlockerCount > 0 ? ' has-errors' : ''}`} aria-hidden="true" />
            <div className="crew-active-compact-body">
              <div className="crew-overview-title-row">
                <div className="crew-active-compact-name">{activeCrew.name}</div>
                <span className={`crew-status-pill ${activeCrewBlockerCount > 0 ? 'warning' : 'ready'}`}>
                  {tr(activeCrewBlockerCount > 0 ? 'Action needed' : 'Ready')}
                </span>
              </div>
              <div className="crew-active-compact-meta">
                <span>{activeAgentCount} {tr('Active members')}</span>
                <span> / </span>
                <span>{activeCrew.tasks.length} {tr('Tasks')}</span>
                <span> / </span>
                <span>{activeCrewBlockerCount} {tr(activeCrewBlockerCount === 1 ? 'Blocker' : 'Blockers')}</span>
              </div>
              <div className="crew-active-compact-meta">
                {activeCrewDiagnostics.errors.length > 0
                  ? tr(activeCrewDiagnostics.errors[0])
                  : activeCrewNeedsMission
                    ? tr('Create a mission in Tasks before running this crew.')
                    : tr('This crew is configured and ready for its next run.')}
              </div>
            </div>
          </div>
          <div className="crew-overview-actions">
            {activeCrewDiagnostics.errors.length > 0 && (
              <button type="button" className="crew-compact-toggle" onClick={reviewActiveCrewBlockers}>{tr('Review blockers')}</button>
            )}
            {activeCrewHasProviderBlocker && (
              <button type="button" className="crew-compact-toggle" onClick={() => navigate(activeCrewProviderSettingsPath)}>{tr('Open settings')}</button>
            )}
            <button type="button" className="crew-compact-toggle" onClick={() => navigate(`/tasks?crew=${encodeURIComponent(activeCrew.id)}`)}>
              {tr('Prepare mission in Tasks')}
            </button>
          </div>
        </div>
      )}

      {crews.length === 0 ? (
        <div className="crew-empty">
          <div className="crew-empty-icon" aria-hidden="true"><Workflow size={26} strokeWidth={1.7} /></div>
          <div className="crew-empty-copy">
            <strong>{tr('Build your first operating team')}</strong>
            <div className="crew-empty-text">{tr('Create a crew, give each agent a clear role, then reuse the setup for repeatable work.')}</div>
          </div>
          <div className="crew-starter-grid" aria-label={tr('Starter crews')}>
            {CREW_STARTER_PRESETS.map((preset, index) => (
              <button
                key={preset.id}
                type="button"
                className="crew-starter-card"
                onClick={() => handleCreateStarterPreset(preset)}
              >
                <span className="crew-starter-number" aria-hidden="true">0{index + 1}</span>
                <strong>{tr(preset.name)}</strong>
                <small>{tr(preset.description)}</small>
              </button>
            ))}
          </div>
          <div className="crew-empty-steps" aria-label={tr('Crew setup steps')}>
            <span><strong>01</strong>{tr('Name the crew')}</span>
            <span><strong>02</strong>{tr('Add roles and tools')}</span>
            <span><strong>03</strong>{tr('Run and review')}</span>
          </div>
          <button type="button" className="crew-toolbar-btn secondary" onClick={() => crewNameInputRef.current?.focus()}>{tr('Name a custom crew')}</button>
        </div>
      ) : (
        <div className={`crew-grid${isCrewListVisible ? '' : ' crew-grid-list-hidden'}`}>
          {/* Left: Crew List */}
          <div className={`crew-list-column${isCrewListVisible ? '' : ' hidden'}`}>
            <div className="crew-list">
              {crews.map((crew) => {
                const diag = getCrewDiagnostics(crew, defaultOpenAICompatibleProfile, defaultOpenRouterProfile)
                const enabledCount = crew.agents.filter((a) => a.enabled).length
                return (
                  <div
                    key={crew.id}
                    className={`crew-card${activeCrew?.id === crew.id ? ' active' : ''}`}
                  >
                    <button
                      type="button"
                      className="crew-card-main"
                      aria-current={activeCrew?.id === crew.id ? 'page' : undefined}
                      onClick={() => setActiveCrew(crew.id)}
                    >
                      <span className={`crew-card-dot${diag.errors.length > 0 ? ' has-errors' : ''}`} />
                      <span className="crew-card-body">
                        <span className="crew-card-name">{crew.name}</span>
                        <span className="crew-card-meta">
                          <span>{processLabel(crew.process)}</span>
                          <span> / </span>
                          <span>{enabledCount}/{crew.agents.length} {tr("active")}</span>
                          {diag.errors.length > 0 && <><span> / </span><span style={{ color: 'var(--danger)' }}>{diag.errors.length} {tr("Blocker")}</span></>}
                        </span>
                      </span>
                    </button>
                    <button type="button" className="crew-card-delete" onClick={(e) => { e.stopPropagation(); deleteCrew(crew.id) }} aria-label={tr("Delete crew")}>
                      <Trash2 size={14} aria-hidden="true" />
                    </button>
                  </div>
                )
              })}
            </div>
          </div>

          {/* Right: Detail */}
          <div className="crew-detail" ref={activeCrewDetailsRef}>
            {activeCrew ? (
              <>
                {/* Section: General */}
                <div className={`crew-section${openSections.general ? ' open' : ''}`}>
                  <button type="button" className="crew-section-header" aria-expanded={openSections.general} aria-controls="crew-section-general" onClick={() => toggleSection('general')}>
                    <span className="crew-section-icon" aria-hidden="true">01</span>{tr("General")}<ChevronDown className="crew-section-chevron" size={16} aria-hidden="true" />
                  </button>
                  {openSections.general && (
                    <div id="crew-section-general" className="crew-section-body">
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">{tr("Crew-Name")}</span>
                          <input aria-label={tr("Crew-Name")} className="crew-input" value={activeCrew.name} onChange={(e) => updateActiveCrew({ name: e.target.value })} />
                        </div>
                        <div className="crew-form-group">
                          <span className="crew-label">{tr("Execution mode")}</span>
                          <select aria-label={tr("Execution mode")} className="crew-select" value={activeCrew.process} onChange={(e) => updateActiveCrew({ process: e.target.value as CrewProcess })}>
                            {PROCESS_OPTIONS.map((o) => <option key={o.value} value={o.value}>{tr(o.label)}</option>)}
                          </select>
                        </div>
                      </div>
                      <div className="crew-form-group full-width">
                        <span className="crew-label">{tr("Description")}</span>
                        <AutoResizeTextarea aria-label={tr("Description")} className="crew-textarea" value={activeCrew.description} onChange={(e) => updateActiveCrew({ description: e.target.value })} />
                      </div>
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">{tr("Execution Subject")}</span>
                          <input aria-label={tr("Execution Subject")} className="crew-input" value={activeCrew.executionSubject} onChange={(e) => updateActiveCrew({ executionSubject: e.target.value })} placeholder={tr("workspace-user")} />
                          <span className="crew-hint">{tr("Must match a stored crew role when governance is active.")}</span>
                        </div>
                      </div>
                      <div className="crew-form-group full-width">
                        <span className="crew-label">{tr("Additional crew instructions")}</span>
                        <AutoResizeTextarea aria-label={tr("Additional crew instructions")} className="crew-textarea" value={activeCrew.executionGuidelines} onChange={(e) => updateActiveCrew({ executionGuidelines: e.target.value })} placeholder={tr("e.g. respond with risks, assumptions, and next steps")} />
                      </div>
                      <div className="crew-form-group full-width">
                        <span className="crew-label">{tr("Knowledge focus")}</span>
                        <AutoResizeTextarea aria-label={tr("Knowledge focus")} className="crew-textarea" value={activeCrew.knowledgeFocus} onChange={(e) => updateActiveCrew({ knowledgeFocus: e.target.value })} placeholder={tr("z. B. priorisiere API-Vertraege, Scheduler-Verhalten und letzte Crew-Laeufe")} />
                        <span className="crew-hint">{tr("Guides memory and knowledge search for the Python runtime prompt.")}</span>
                      </div>
                    </div>
                  )}
                </div>

                {/* Section: Execution */}
                <div className={`crew-section${openSections.execution ? ' open' : ''}`}>
                  <button type="button" className="crew-section-header" aria-expanded={openSections.execution} aria-controls="crew-section-execution" onClick={() => toggleSection('execution')}>
                    <span className="crew-section-icon" aria-hidden="true">02</span>{tr("Execution")}<ChevronDown className="crew-section-chevron" size={16} aria-hidden="true" />
                  </button>
                  {openSections.execution && (
                    <div id="crew-section-execution" className="crew-section-body">
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">{tr("Manager-Agent")}</span>
                          <select aria-label={tr("Manager-Agent")} className="crew-select" value={activeCrew.managerAgentId ?? ''} onChange={(e) => updateActiveCrew({ managerAgentId: e.target.value || null })}>
                            <option value="">{tr("Nor")}</option>
                            {activeCrew.agents.map((a) => {
                              const profile = a.personalityId ? personalityProfiles.find((entry) => entry.id === a.personalityId) : null
                              const resolved = profile ? resolveCrewAgentWithProfile(a, [profile]) : a
                              return <option key={a.id} value={a.id}>{resolved.name}</option>
                            })}
                          </select>
                        </div>
                        <div className="crew-form-group">
                          <span className="crew-label">{tr("Outputformat")}</span>
                          <select aria-label={tr("Outputformat")} className="crew-select" value={activeCrew.outputMode} onChange={(e) => updateActiveCrew({ outputMode: e.target.value as CrewOutputMode })}>
                            <option value="standard">{tr("Standard")}</option>
                            <option value="bullet-report">{tr("Stichpunkt-Report")}</option>
                            <option value="json">{tr("JSON")}</option>
                          </select>
                        </div>
                      </div>
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">{tr("Max RPM")}</span>
                          <input aria-label={tr("Max RPM")} className="crew-input" type="number" min={1} max={600} value={activeCrew.maxRpm} onChange={(e) => updateActiveCrew({ maxRpm: Number(e.target.value) || 1 })} />
                        </div>
                        <div className="crew-form-group">
                          <span className="crew-label">{tr("Max parallele Tasks")}</span>
                          <input aria-label={tr("Max parallele Tasks")} className="crew-input" type="number" min={1} max={24} value={activeCrew.maxParallelTasks} onChange={(e) => updateActiveCrew({ maxParallelTasks: Number(e.target.value) || 1 })} />
                        </div>
                      </div>
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">{tr("Retry-Versuche pro Task")}</span>
                          <input aria-label={tr("Retry-Versuche pro Task")} className="crew-input" type="number" min={0} max={5} value={activeCrew.retryCount} onChange={(e) => updateActiveCrew({ retryCount: Math.max(0, Number(e.target.value) || 0) })} />
                        </div>
                        <div className="crew-form-group">
                          <span className="crew-label">{tr("Zeichenlimit geteilte Resultse")}</span>
                          <input aria-label={tr("Zeichenlimit geteilte Resultse")} className="crew-input" type="number" min={0} max={50000} value={activeCrew.sharedOutputCharLimit} onChange={(e) => updateActiveCrew({ sharedOutputCharLimit: Math.max(0, Number(e.target.value) || 0) })} />
                        </div>
                      </div>
                      <label className="crew-checkbox-label"><input type="checkbox" checked={activeCrew.verbose} onChange={(e) => updateActiveCrew({ verbose: e.target.checked })} />{tr("Enable verbose crew logging")}</label>
                      <label className="crew-checkbox-label"><input type="checkbox" checked={activeCrew.stopOnFailure} onChange={(e) => updateActiveCrew({ stopOnFailure: e.target.checked })} />{tr("Crew bei Task-Error sofort stoppen")}</label>
                      <label className="crew-checkbox-label"><input type="checkbox" checked={activeCrew.managerReviewEnabled} onChange={(e) => updateActiveCrew({ managerReviewEnabled: e.target.checked })} />{tr("Enable manager review after task batches")}</label>
                      {activeCrew.managerReviewEnabled && (
                        <div className="crew-form-group">
                          <span className="crew-label">{tr("Manager-Review-instructions")}</span>
                          <AutoResizeTextarea aria-label={tr("Manager-Review-instructions")} className="crew-textarea" value={activeCrew.managerReviewGuidelines} onChange={(e) => updateActiveCrew({ managerReviewGuidelines: e.target.value })} placeholder={tr("e.g. assess risks more strictly and escalate early")} />
                        </div>
                      )}
                      <label className="crew-checkbox-label"><input type="checkbox" checked={activeCrew.shareAllTaskOutputs} onChange={(e) => updateActiveCrew({ shareAllTaskOutputs: e.target.checked })} />{tr("Vorherige Task-Resultse global als Context teilen")}</label>
                    </div>
                  )}
                </div>

                {/* Section: Provider & Model */}
                <div className={`crew-section${openSections.provider ? ' open' : ''}`}>
                  <button type="button" className="crew-section-header" aria-expanded={openSections.provider} aria-controls="crew-section-provider" onClick={() => toggleSection('provider')}>
                    <span className="crew-section-icon" aria-hidden="true">03</span>{tr("Provider & Model")}<ChevronDown className="crew-section-chevron" size={16} aria-hidden="true" />
                  </button>
                  {openSections.provider && (
                    <div id="crew-section-provider" className="crew-section-body">
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">{tr("Crew-Provider")}</span>
                          <select aria-label={tr("Crew-Provider")} className="crew-select" value={activeCrew.defaultProvider || 'ollama'} onChange={(e) => handleCrewDefaultProviderChange(e.target.value as CrewProviderKind)}>
                            {PROVIDER_OPTIONS.map((o) => {
                              const ok = o.value === activeCrew.defaultProvider || isProviderEnabledForCrew(o.value)
                              return <option key={o.value} value={o.value} disabled={!ok}>{ok ? tr(o.label) : `${tr(o.label)} (${tr("Enable profile")})`}</option>
                            })}
                          </select>
                          <span className="crew-hint">{tr("The crew provider applies to all members. Only the model can still be overridden per member.")}</span>
                        </div>
                        <div className="crew-form-group">
                          <span className="crew-label">{tr("Crew-Model")}</span>
                          <select aria-label={tr("Crew-Model")} className="crew-select" value={activeCrew.defaultModel || ''} onChange={(e) => updateActiveCrew({ defaultModel: e.target.value })}>
                            <option value="">{tr("Globale Settings verwenden")}</option>
                            {getCrewDefaultModelOptions().map((m) => <option key={m} value={m}>{m}</option>)}
                          </select>
                          {activeCrew.defaultProvider !== 'ollama' && getCrewDefaultModelOptions().length === 0 && (
                            <span className="crew-hint">{tr("No models have been loaded for this provider yet.")}</span>
                          )}
                          <span className="crew-hint">{tr("Aktuell wirksam:")}{activeCrew.defaultModel || tr('Global settings')}</span>
                          <span className="crew-hint">{tr("Gilt automatisch for alle members ohne eigenes Model-Override.")}</span>
                        </div>
                      </div>
                    </div>
                  )}
                </div>

                {/* Diagnostics */}
                <div className={`crew-section${openSections.diagnostics ? ' open' : ''}`}>
                  <button ref={diagnosticsHeaderRef} type="button" className="crew-section-header" aria-expanded={openSections.diagnostics} aria-controls="crew-section-diagnostics" onClick={() => toggleSection('diagnostics')}>
                    <span className="crew-section-icon" aria-hidden="true">04</span>{tr("Diagnostics")}<ChevronDown className="crew-section-chevron" size={16} aria-hidden="true" />
                  </button>
                  {openSections.diagnostics && (
                    <div id="crew-section-diagnostics" className="crew-section-body">
                      {activeCrewDiagnostics.errors.length === 0 && activeCrewDiagnostics.warnings.length === 0 ? (
                        <div className="crew-alert success"><span className="crew-alert-icon" aria-hidden="true">OK</span>{tr("No blockers found. Crew is ready to start.")}</div>
                      ) : (
                        <>
                          {activeCrewDiagnostics.errors.map((entry) => (
                            <div key={`e-${entry}`} className="crew-alert error"><span className="crew-alert-icon" aria-hidden="true">!</span> {tr(entry)}</div>
                          ))}
                          {activeCrewDiagnostics.warnings.map((entry) => (
                            <div key={`w-${entry}`} className="crew-alert warning"><span className="crew-alert-icon" aria-hidden="true">!</span> {tr(entry)}</div>
                          ))}
                          {activeCrewHasProviderBlocker && (
                            <div className="crew-overview-actions">
                              <button type="button" className="ui-button ui-button--secondary" onClick={() => navigate(activeCrewProviderSettingsPath)}>
                                <Settings2 size={15} aria-hidden="true" />
                                {tr('Fix provider settings')}
                              </button>
                            </div>
                          )}
                        </>
                      )}
                    </div>
                  )}
                </div>

                {/* Agents */}
                <div className={`crew-section${openSections.members ? ' open' : ''}`}>
                  <button type="button" className="crew-section-header" aria-expanded={openSections.members} aria-controls="crew-section-members" onClick={() => toggleSection('members')}>
                    <span className="crew-section-icon" aria-hidden="true">05</span>{tr("Crew-members (")}{activeAgentCount}/{activeCrew.agents.length})<ChevronDown className="crew-section-chevron" size={16} aria-hidden="true" />
                  </button>
                  {openSections.members && (
                  <div id="crew-section-members" className="crew-section-body">
                    <div className="crew-members-hero">
                      <div className="crew-members-copy">
                        <div className="crew-overview-kicker">{tr("members")}</div>
                        <strong className="crew-overview-title">{tr("Roles, models, and access per agent")}</strong>
                        <div className="crew-overview-description">{tr("Profiles, runtime behavior, and permissions stay visible together here so the crew configuration does not fragment into separate form blocks.")}</div>
                      </div>
                      <div className="crew-members-stats">
                        <div className="crew-members-stat">
                          <strong>{activeAgentCount}</strong>
                          <span>{tr("active")}</span>
                        </div>
                        <div className="crew-members-stat">
                          <strong>{profileBackedAgentCount}</strong>
                          <span>{tr("with profile")}</span>
                        </div>
                        <div className="crew-members-stat">
                          <strong>{configuredToolCount}</strong>
                          <span>{tr("Tools genutzt")}</span>
                        </div>
                        <div className="crew-members-stat">
                          <strong>{configuredMcpCount}</strong>
                          <span>{tr("MCP-Zugriffe")}</span>
                        </div>
                      </div>
                    </div>
                    <div className="crew-agent-panel crew-agent-panel-wide crew-bulk-access-panel">
                      <div className="crew-agent-panel-header crew-agent-panel-header-split">
                        <div>
                          <div className="crew-agent-panel-title">{tr("Approvals for all members")}</div>
                          <div className="crew-agent-panel-subtitle">{tr("Sets tool and MCP access globally for the active crew.")}</div>
                        </div>
                        <div className="crew-members-badge-row">
                          <span className="crew-inline-badge subtle">{claudeTools.length}{tr("Tools available")}</span>
                          <span className="crew-inline-badge subtle">{configuredMcpServers.length}{tr("MCP-Server")}</span>
                        </div>
                      </div>
                      <div className="crew-agent-access-grid">
                        <div className="crew-agent-subpanel">
                          <div className="crew-form-group">
                            <div className="crew-subpanel-head">
                              <span className="crew-label">{tr("Tools")}</span>
                              <span className="crew-subpanel-count">{tr("All members synchronisieren")}</span>
                            </div>
                            <div className="crew-tool-list">
                              {claudeTools.map((tool) => {
                                const allAgentsHaveTool = activeCrew.agents.length > 0 && activeCrew.agents.every((agent) => agent.tools.includes(tool.id))
                                return (
                                  <label key={tool.id} className="crew-tool-item">
                                    <input
                                      type="checkbox"
                                      checked={allAgentsHaveTool}
                                      onChange={(event) => setCrewToolForAllAgents(tool.id, event.target.checked)}
                                    />
                                    {tr(tool.label)}
                                  </label>
                                )
                              })}
                            </div>
                          </div>
                        </div>
                        <div className="crew-agent-subpanel">
                          <div className="crew-form-group">
                            <div className="crew-subpanel-head">
                              <span className="crew-label">{tr("MCP-Zugriffe")}</span>
                              <span className="crew-subpanel-count">{tr("Workspace-Verbindungen")}</span>
                            </div>
                            {configuredMcpServers.length === 0 ? <span className="crew-hint">{tr("No MCP-Server configured.")}</span> : (
                              <div className="crew-tool-list">
                                {configuredMcpServers.map((srv) => {
                                  const allAgentsHaveServer = activeCrew.agents.length > 0 && activeCrew.agents.every((agent) => agent.mcpServerNames.includes(srv.name))
                                  return (
                                    <label key={srv.name} className="crew-tool-item">
                                      <input
                                        type="checkbox"
                                        checked={allAgentsHaveServer}
                                        onChange={(event) => setCrewMcpServerForAllAgents(srv.name, event.target.checked)}
                                      />
                                      {srv.name}
                                    </label>
                                  )
                                })}
                              </div>
                            )}
                          </div>
                        </div>
                      </div>
                    </div>
                    <div className="crew-agents-list">
                      {activeCrew.agents.map((agent) => {
                        const profile = agent.personalityId ? personalityProfiles.find((entry) => entry.id === agent.personalityId) ?? null : null
                        const profileAgent = profile ? resolveCrewAgentWithProfile(agent, [profile]) : agent
                        const effectiveProviderKind = activeCrew.defaultProvider || 'ollama'
                        const pmc = getProviderModelCatalog(effectiveProviderKind)
                        const amo = getAgentModelOptions(profileAgent)
                        const selModel = profileAgent.modelOverride?.trim() && amo.includes(profileAgent.modelOverride.trim()) ? profileAgent.modelOverride.trim() : ''
                        const effectiveModelLabel = selModel || getCrewDefaultModelLabel(effectiveProviderKind)
                        const isOpen = expandedAgents[agent.id] ?? false
                        const updateProfileOrSnapshot = (patch: Partial<CrewAgent>) => {
                          if (profile) {
                            updateCrewPersonalityProfile(profile, {
                              name: patch.name ?? profile.name,
                              role: patch.role ?? profile.role,
                              goal: patch.goal ?? profile.goal,
                              systemPrompt: patch.backstory ?? profile.systemPrompt,
                              skillsMarkdown: patch.skillsMarkdown ?? profile.skillsMarkdown,
                              modelOverride: patch.modelOverride !== undefined ? patch.modelOverride : profile.modelOverride,
                            })
                            return
                          }

                          updateActiveCrewAgent(agent.id, patch)
                        }
                        return (
                          <div key={agent.id} className={`crew-agent-card${!agent.enabled ? ' disabled' : ''}${isOpen ? ' open' : ''}`}>
                            <button
                              type="button"
                              className="crew-agent-header"
                              aria-expanded={isOpen}
                              onClick={() => toggleAgent(agent.id)}
                            >
                              <div className="crew-agent-avatar">{roleEmoji(profileAgent.role)}</div>
                              <div className="crew-agent-info">
                                <div className="crew-agent-name">{profileAgent.name}</div>
                                <div className="crew-agent-role">{profileAgent.role}</div>
                                <div className="crew-agent-summary">
                                  <span className="crew-inline-badge">{getProviderLabel(effectiveProviderKind)}</span>
                                  <span className="crew-inline-badge subtle">{effectiveModelLabel}</span>
                                  {profile && <span className="crew-inline-badge subtle">{tr("global profile")}</span>}
                                </div>
                              </div>
                              <div className="crew-agent-header-actions">
                                <span className={`crew-badge ${agent.enabled ? 'active' : 'inactive'}`}>{agent.enabled ? tr("Active") : tr("Inactive")}</span>
                                <ChevronDown className="crew-agent-chevron" size={16} aria-hidden="true" />
                              </div>
                            </button>
                            {isOpen && (
                              <div className="crew-agent-body">
                                <div className="crew-agent-panel">
                                  <div className="crew-agent-panel-header">
                                    <div className="crew-agent-panel-title">{tr("Profile")}</div>
                                    <div className="crew-agent-panel-subtitle">{tr("Name, role, and work context of the crew member.")}</div>
                                  </div>
                                  <div className="crew-agent-col">
                                    <div className="crew-form-row">
                                      {profile && (
                                        <div className="crew-form-group">
                                          <span className="crew-label">{tr("Icon")}</span>
                                          <input aria-label={tr("Icon")} className="crew-input" value={profile.icon ?? ''} maxLength={4} onChange={(e) => updateCrewPersonalityProfile(profile, { icon: e.target.value || null })} />
                                        </div>
                                      )}
                                      <div className="crew-form-group"><span className="crew-label">{tr("Name")}</span><input aria-label={tr("Name")} className="crew-input" value={profileAgent.name} onChange={(e) => updateProfileOrSnapshot({ name: e.target.value })} /></div>
                                      <div className="crew-form-group"><span className="crew-label">{tr("Rolle")}</span><select aria-label={tr("Rolle")} className="crew-select" value={profileAgent.role} onChange={(e) => updateProfileOrSnapshot({ role: e.target.value as AgentRole })}>{ROLE_OPTIONS.map((r) => <option key={r} value={r}>{r}</option>)}</select></div>
                                      {profile && (
                                        <div className="crew-form-group">
                                          <span className="crew-label">{tr("Temperatur")}</span>
                                          <input aria-label={tr("Temperatur")} className="crew-input" type="number" min={0} max={2} step={0.1} value={profile.temperature ?? ''} onChange={(e) => updateCrewPersonalityProfile(profile, { temperature: e.target.value === '' ? null : Number(e.target.value) })} />
                                        </div>
                                      )}
                                    </div>
                                    {profile && (
                                      <label className="crew-checkbox-label"><input type="checkbox" checked={profile.isDefault ?? false} onChange={(e) => updateCrewPersonalityProfile(profile, { isDefault: e.target.checked })} />{tr("Als Standard verwenden")}</label>
                                    )}
                                    <div className="crew-form-group"><span className="crew-label">{tr("Target / Prompt-Fokus")}</span><AutoResizeTextarea aria-label={tr("Target / Prompt-Fokus")} className="crew-textarea" value={profileAgent.goal} onChange={(e) => updateProfileOrSnapshot({ goal: e.target.value })} /></div>
                                    <div className="crew-form-group"><span className="crew-label">{tr("Background / system prompt")}</span><AutoResizeTextarea aria-label={tr("Background / system prompt")} className="crew-textarea" value={profileAgent.backstory} onChange={(e) => updateProfileOrSnapshot({ backstory: e.target.value })} /></div>
                                    <div className="crew-form-group"><span className="crew-label">{tr("skills.md")}</span><AutoResizeTextarea aria-label={tr("skills.md")} className="crew-textarea" value={profileAgent.skillsMarkdown} onChange={(e) => updateProfileOrSnapshot({ skillsMarkdown: e.target.value })} placeholder={tr("# skills.md&#10;- Project context&#10;- Work style")} /></div>
                                    <span className="crew-hint">{profile ? tr("Profile fields are synchronized globally for all crews.") : tr("Local snapshot without active profile synchronization.")}</span>
                                  </div>
                                </div>
                                <div className="crew-agent-panel">
                                  <div className="crew-agent-panel-header">
                                    <div className="crew-agent-panel-title">{tr("Configuration")}</div>
                                    <div className="crew-agent-panel-subtitle">{tr("Status, provider, model, and runtime behavior.")}</div>
                                  </div>
                                  <div className="crew-agent-col">
                                    <div className="crew-checkbox-stack">
                                      <label className="crew-checkbox-label"><input type="checkbox" checked={agent.enabled} onChange={(e) => updateActiveCrewAgent(agent.id, { enabled: e.target.checked })} />{tr("Enabled")}</label>
                                      <label className="crew-checkbox-label"><input type="checkbox" checked={agent.allowDelegation} onChange={(e) => updateActiveCrewAgent(agent.id, { allowDelegation: e.target.checked })} />{tr("Delegation allowed")}</label>
                                      <label className="crew-checkbox-label"><input type="checkbox" checked={agent.verbose} onChange={(e) => updateActiveCrewAgent(agent.id, { verbose: e.target.checked })} />{tr("Verbose Logs")}</label>
                                    </div>
                                    <div className="crew-form-row">
                                      <div className="crew-form-group">
                                        <span className="crew-label">{tr("Provider")}</span>
                                        <input aria-label={tr("Provider")} className="crew-input" value={getProviderLabel(effectiveProviderKind)} readOnly />
                                        <span className="crew-hint">{tr("Controlled by the crew provider.")}</span>
                                      </div>
                                      <div className="crew-form-group">
                                        <span className="crew-label">{tr("Max Iterationen")}</span>
                                        <input aria-label={tr("Max Iterationen")} className="crew-input" type="number" min={1} max={100} value={agent.maxIterations} onChange={(e) => updateActiveCrewAgent(agent.id, { maxIterations: Number(e.target.value) || 1 })} />
                                      </div>
                                    </div>
                                    <div className="crew-form-group">
                                      <span className="crew-label">{tr("Model")}</span>
                                      <select aria-label={tr("Model")} className="crew-select" value={selModel} onChange={(e) => updateProfileOrSnapshot({ modelOverride: e.target.value || null })}>
                                        <option value="">{tr("Crew-Model (")}{getCrewDefaultModelLabel(effectiveProviderKind)})</option>
                                        {amo.map((m) => <option key={m} value={m}>{m}</option>)}
                                      </select>
                                      {effectiveProviderKind !== 'ollama' && pmc.models.length === 0 && <span className="crew-hint">{tr("No models loaded.")}</span>}
                                      <span className="crew-hint">{tr("Aktuell wirksam:")}{effectiveModelLabel}</span>
                                    </div>
                                  </div>
                                </div>
                                <div className="crew-agent-panel crew-agent-panel-wide">
                                  <div className="crew-agent-panel-header">
                                    <div className="crew-agent-panel-title">{tr("Werkzeuge & Zugriffe")}</div>
                                    <div className="crew-agent-panel-subtitle">{tr("All available features remain available, but are grouped more clearly here.")}</div>
                                  </div>
                                  <div className="crew-agent-access-grid">
                                    <div className="crew-agent-subpanel">
                                      <div className="crew-form-group">
                                        <span className="crew-label">{tr("Tools")}</span>
                                        <div className="crew-tool-list">
                                          {claudeTools.map((tool) => (
                                            <label key={tool.id} className="crew-tool-item">
                                              <input
                                                type="checkbox"
                                                checked={agent.tools.includes(tool.id)}
                                                onChange={(event) => updateActiveCrewAgent(agent.id, {
                                                  tools: toggleStringValue(agent.tools, tool.id),
                                                  allowDelegation: tool.id === 'delegate_task' ? event.target.checked : agent.allowDelegation,
                                                })}
                                              />
                                              {tr(tool.label)}
                                            </label>
                                          ))}
                                        </div>
                                      </div>
                                    </div>
                                    <div className="crew-agent-subpanel">
                                      <div className="crew-form-group">
                                        <span className="crew-label">{tr("MCP-Zugriffe")}</span>
                                        {configuredMcpServers.length === 0 ? <span className="crew-hint">{tr("No MCP-Server configured.")}</span> : (
                                          <div className="crew-tool-list">
                                            {configuredMcpServers.map((srv) => (<label key={srv.name} className="crew-tool-item"><input type="checkbox" checked={agent.mcpServerNames.includes(srv.name)} onChange={() => updateActiveCrewAgent(agent.id, { mcpServerNames: toggleStringValue(agent.mcpServerNames, srv.name) })} />{srv.name}</label>))}
                                          </div>
                                        )}
                                      </div>
                                    </div>
                                  </div>
                                </div>
                              </div>
                            )}
                          </div>
                        )
                      })}
                    </div>
                  </div>
                  )}
                </div>

                {/* Tasks hint */}
                <div className="crew-task-rail">
                  <div className="crew-task-rail-copy">
                    <div className="crew-overview-kicker">{tr("Task-Flow")}</div>
                    <strong className="crew-overview-title">{tr("Turn this crew into one complete mission")}</strong>
                    <div className="crew-overview-description">{tr("Prepare the objective here, then create, run, and schedule the complete crew workflow in Tasks.")}</div>
                    <button
                      type="button"
                      className="ui-button ui-button--primary"
                      style={{ justifySelf: 'start' }}
                      onClick={() => navigate(`/tasks?crew=${encodeURIComponent(activeCrew.id)}`)}
                    >
                      {tr("Prepare mission in Tasks")}
                      <ArrowRight size={15} aria-hidden="true" />
                    </button>
                  </div>
                  <div className="crew-task-rail-metrics">
                    <div className="crew-task-metric">
                      <span>{tr("Parallel")}</span>
                      <strong>{activeCrew.maxParallelTasks}</strong>
                    </div>
                    <div className="crew-task-metric">
                      <span>{tr("Retries")}</span>
                      <strong>{activeCrew.retryCount}</strong>
                    </div>
                    <div className="crew-task-metric">
                      <span>{tr("Output")}</span>
                      <strong>{outputModeLabel}</strong>
                    </div>
                    <div className="crew-task-metric">
                      <span>{tr("Context teilen")}</span>
                      <strong>{activeCrew.shareAllTaskOutputs ? 'Ja' : 'Nein'}</strong>
                    </div>
                  </div>
                </div>
              </>
            ) : (
              <div className="crew-empty">
                <div className="crew-empty-icon" aria-hidden="true"><MousePointerClick size={24} /></div>
                <div className="crew-empty-text">{tr("Select a crew from the list.")}</div>
              </div>
            )}
          </div>
        </div>
      )}

      <CrewRuntimePanel />

      {activeCrew && (
        <div className="crew-overview-grid">
          <CrewControlPlanePanel activeCrew={activeCrew} />
          <div className="crew-overview-meta-grid">
            <CrewGovernancePanel activeCrewId={activeCrew.id} />
            <CrewHistoryPanel activeCrewId={activeCrew.id} />
          </div>
        </div>
      )}
    </div>
  )
}
