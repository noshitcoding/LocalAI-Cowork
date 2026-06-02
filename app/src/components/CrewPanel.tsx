import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useConfigStore, type OllamaConfig } from '../stores/configStore'
import { useCoworkStore } from '../stores/coworkStore'
import { resolveCrewAgentWithProfile, useCrewStore, type AgentRole, type CrewAgent, type CrewExternalProviderConfig, type CrewOutputMode, type CrewPersonalityProfile, type CrewProcess, type CrewProviderKind, type CrewProviderProfiles, type CrewRuntimeConfig } from '../stores/crewStore'
import { usePersonalityStore } from '../stores/personalityStore'
import CrewControlPlanePanel from './crew/CrewControlPlanePanel'
import CrewGovernancePanel from './crew/CrewGovernancePanel'
import CrewHistoryPanel from './crew/CrewHistoryPanel'
import CrewRuntimePanel from './crew/CrewRuntimePanel'
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
    errors.push('Keine aktiven Crew-Mitglieder vorhanden.')
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

  const openRouterAgents = crewDefaultProvider === 'openrouter' ? enabledAgents : []
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
    syncAgentsFromPersonalityProfiles,
    migrateAgentsToPersonalityProfiles,
    updateCrewAgent,
  } = useCrewStore()
  const { availableModels, defaultLlmProfileIds, llmProfiles, mcpServer, mcpServers, ollama } = useConfigStore()
  const claudeTools = useCoworkStore((state) => state.claudeTools)
  const personalities = usePersonalityStore((state) => state.personalities)
  const loadPersonalities = usePersonalityStore((state) => state.loadPersonalities)
  const upsertPersonality = usePersonalityStore((state) => state.upsertPersonality)

  const [crewName, setCrewName] = useState('')
  const [providerModelOptions, setProviderModelOptions] = useState<Record<string, ProviderModelState>>({})
  const [pendingScrollCrewId, setPendingScrollCrewId] = useState<string | null>(null)
  const [openSections, setOpenSections] = useState<Record<string, boolean>>({ general: true, execution: false, provider: false, diagnostics: true })
  const [expandedAgents, setExpandedAgents] = useState<Record<string, boolean>>({})
  const [isCrewListVisible, setIsCrewListVisible] = useState(() => typeof window === 'undefined' ? true : window.innerWidth >= 1320)
  const importCrewInputRef = useRef<HTMLInputElement | null>(null)
  const activeCrewDetailsRef = useRef<HTMLDivElement | null>(null)

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
    return `${config.baseUrl.trim()}::${config.apiKey.trim()}::${config.verifyTlsCertificates}`
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
  }, [activeCrew, providerModelOptions, resolvedActiveProviderConfigs])

  useEffect(() => {
    if (!pendingScrollCrewId || activeCrew?.id !== pendingScrollCrewId) return
    activeCrewDetailsRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' })
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
  }, [activeCrew, availableModels, personalityProfiles, providerModelOptions, updateCrewAgent, updateCrewPersonalityProfile])

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
      const imported = JSON.parse(raw) as ImportedCrewPayload
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
        isCrewProviderProfiles(imported.providerProfiles)
      ) {
        patch.providerProfiles = imported.providerProfiles
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
    const map: Record<AgentRole, string> = { researcher: '🔍', writer: '✍️', reviewer: '🔎', planner: '📋', executor: '⚙️', analyst: '📊', custom: '🤖' }
    return map[role] ?? '🤖'
  }
  const processLabel = (p: CrewProcess) => PROCESS_OPTIONS.find((o) => o.value === p)?.label ?? p
  const activeAgentCount = activeCrew?.agents.filter((agent) => agent.enabled).length ?? 0
  const profileBackedAgentCount = activeCrew?.agents.filter((agent) => Boolean(agent.personalityId)).length ?? 0
  const configuredToolCount = activeCrew ? new Set(activeCrew.agents.flatMap((agent) => agent.tools)).size : 0
  const configuredMcpCount = activeCrew ? new Set(activeCrew.agents.flatMap((agent) => agent.mcpServerNames)).size : 0
  const outputModeLabel = activeCrew?.outputMode === 'bullet-report'
    ? 'Stichpunkte'
    : activeCrew?.outputMode === 'json'
      ? 'JSON'
      : 'Standard'

  return (
    <div className="panel crew-shell">
      <div className="crew-shell-top">
        <CrewRuntimePanel />
        <div className="crew-header">
          <div className="crew-header-copy">
            <div className="crew-header-eyebrow">Crew Workspace</div>
            <div className="crew-header-title">
              <span className="crew-header-icon">🚀</span>
              <span>Crew AI</span>
            </div>
            <div className="crew-header-subtitle">
              Plane Rollen, Governance und reproduzierbare Runs in einer aufgeraeumten Arbeitsflaeche statt in gequetschten Einzelkarten.
            </div>
          </div>
          <div className="crew-header-badge" aria-label="Crew-Uebersicht">
            <strong>{crews.length}</strong>
            <span>{crews.length === 1 ? 'Crew konfiguriert' : 'Crews konfiguriert'}</span>
          </div>
        </div>
      </div>
      <input ref={importCrewInputRef} type="file" accept="application/json" style={{ display: 'none' }} onChange={(event) => void handleImportCrew(event)} />

      <div className="crew-toolbar">
        <div className="crew-toolbar-primary">
          <input
            className="crew-toolbar-input"
            placeholder="Neue Crew…"
            value={crewName}
            onChange={(event) => setCrewName(event.target.value)}
            onKeyDown={(event) => { if (event.key === 'Enter') handleCreateCrew() }}
          />
          <button type="button" className="crew-toolbar-btn" onClick={handleCreateCrew}>Anlegen</button>
        </div>
        <div className="crew-toolbar-actions">
          <button type="button" className="crew-toolbar-btn secondary" onClick={handleDuplicateCrew} disabled={!activeCrew}>Duplizieren</button>
          <button type="button" className="crew-toolbar-btn secondary" onClick={handleExportCrew} disabled={!activeCrew}>Export</button>
          <button type="button" className="crew-toolbar-btn secondary" onClick={() => importCrewInputRef.current?.click()}>Import</button>
          <button type="button" className="crew-toolbar-btn secondary crew-toolbar-toggle" aria-pressed={isCrewListVisible} onClick={toggleCrewListVisibility}>
            {isCrewListVisible ? 'Liste ausblenden' : 'Liste zeigen'}
          </button>
        </div>
      </div>

      {activeCrew && (
        <div className="crew-overview-grid">
          <CrewControlPlanePanel activeCrew={activeCrew} />
          <div className="crew-overview-meta-grid">
            <CrewGovernancePanel activeCrewId={activeCrew.id} />
            <CrewHistoryPanel activeCrewId={activeCrew.id} />
          </div>
        </div>
      )}

      {crews.length === 0 ? (
        <div className="crew-empty">
          <div className="crew-empty-icon">🚀</div>
          <div className="crew-empty-text">Noch keine Crew vorhanden. Erstelle deine erste Crew oben.</div>
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
                  <div key={crew.id} className={`crew-card${activeCrew?.id === crew.id ? ' active' : ''}`} onClick={() => setActiveCrew(crew.id)}>
                    <span className={`crew-card-dot${diag.errors.length > 0 ? ' has-errors' : ''}`} />
                    <div className="crew-card-body">
                      <div className="crew-card-name">{crew.name}</div>
                      <div className="crew-card-meta">
                        <span>{processLabel(crew.process)}</span>
                        <span>·</span>
                        <span>{enabledCount}/{crew.agents.length} aktiv</span>
                        {diag.errors.length > 0 && <><span>·</span><span style={{ color: 'var(--danger)' }}>{diag.errors.length} Blocker</span></>}
                      </div>
                    </div>
                    <button type="button" className="crew-card-delete" onClick={(e) => { e.stopPropagation(); deleteCrew(crew.id) }}>✕</button>
                  </div>
                )
              })}
            </div>
          </div>

          {/* Right: Detail */}
          <div className="crew-detail" ref={activeCrewDetailsRef}>
            {activeCrew ? (
              <>
                {!isCrewListVisible && (
                  <div className="crew-active-compact">
                    <div className="crew-active-compact-main">
                      <span className={`crew-card-dot${activeCrewDiagnostics.errors.length > 0 ? ' has-errors' : ''}`} />
                      <div className="crew-active-compact-body">
                        <div className="crew-active-compact-name">{activeCrew.name}</div>
                        <div className="crew-active-compact-meta">
                          <span>{processLabel(activeCrew.process)}</span>
                          <span>·</span>
                          <span>{activeCrew.agents.filter((agent) => agent.enabled).length}/{activeCrew.agents.length} aktiv</span>
                        </div>
                      </div>
                    </div>
                    <button type="button" className="crew-compact-toggle" onClick={toggleCrewListVisibility}>Liste zeigen</button>
                  </div>
                )}

                {/* Section: Allgemein */}
                <div className={`crew-section${openSections.general ? ' open' : ''}`}>
                  <button type="button" className="crew-section-header" onClick={() => toggleSection('general')}>
                    <span className="crew-section-icon">📝</span> Allgemein
                    <span className="crew-section-chevron">▾</span>
                  </button>
                  {openSections.general && (
                    <div className="crew-section-body">
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">Crew-Name</span>
                          <input className="crew-input" value={activeCrew.name} onChange={(e) => updateActiveCrew({ name: e.target.value })} />
                        </div>
                        <div className="crew-form-group">
                          <span className="crew-label">Ausführungsmodus</span>
                          <select className="crew-select" value={activeCrew.process} onChange={(e) => updateActiveCrew({ process: e.target.value as CrewProcess })}>
                            {PROCESS_OPTIONS.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
                          </select>
                        </div>
                      </div>
                      <div className="crew-form-group full-width">
                        <span className="crew-label">Beschreibung</span>
                        <AutoResizeTextarea className="crew-textarea" value={activeCrew.description} onChange={(e) => updateActiveCrew({ description: e.target.value })} />
                      </div>
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">Execution Subject</span>
                          <input className="crew-input" value={activeCrew.executionSubject} onChange={(e) => updateActiveCrew({ executionSubject: e.target.value })} placeholder="workspace-user" />
                          <span className="crew-hint">Muss zu einer hinterlegten Crew-Rolle passen, wenn Governance aktiv ist.</span>
                        </div>
                      </div>
                      <div className="crew-form-group full-width">
                        <span className="crew-label">Crew-Zusatzanweisungen</span>
                        <AutoResizeTextarea className="crew-textarea" value={activeCrew.executionGuidelines} onChange={(e) => updateActiveCrew({ executionGuidelines: e.target.value })} placeholder="z. B. Antworte mit Risiken, Annahmen und nächsten Schritten" />
                      </div>
                      <div className="crew-form-group full-width">
                        <span className="crew-label">Knowledge-Fokus</span>
                        <AutoResizeTextarea className="crew-textarea" value={activeCrew.knowledgeFocus} onChange={(e) => updateActiveCrew({ knowledgeFocus: e.target.value })} placeholder="z. B. priorisiere API-Vertraege, Scheduler-Verhalten und letzte Crew-Laeufe" />
                        <span className="crew-hint">Lenkt die Memory- und Knowledge-Suche fuer den Python-Runtime-Prompt.</span>
                      </div>
                    </div>
                  )}
                </div>

                {/* Section: Ausführung */}
                <div className={`crew-section${openSections.execution ? ' open' : ''}`}>
                  <button type="button" className="crew-section-header" onClick={() => toggleSection('execution')}>
                    <span className="crew-section-icon">⚡</span> Ausführung
                    <span className="crew-section-chevron">▾</span>
                  </button>
                  {openSections.execution && (
                    <div className="crew-section-body">
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">Manager-Agent</span>
                          <select className="crew-select" value={activeCrew.managerAgentId ?? ''} onChange={(e) => updateActiveCrew({ managerAgentId: e.target.value || null })}>
                            <option value="">Keiner</option>
                            {activeCrew.agents.map((a) => {
                              const profile = a.personalityId ? personalityProfiles.find((entry) => entry.id === a.personalityId) : null
                              const resolved = profile ? resolveCrewAgentWithProfile(a, [profile]) : a
                              return <option key={a.id} value={a.id}>{resolved.name}</option>
                            })}
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
                          <AutoResizeTextarea className="crew-textarea" value={activeCrew.managerReviewGuidelines} onChange={(e) => updateActiveCrew({ managerReviewGuidelines: e.target.value })} placeholder="z. B. Beurteile Risiken strenger und eskaliere frühzeitig" />
                        </div>
                      )}
                      <label className="crew-checkbox-label"><input type="checkbox" checked={activeCrew.shareAllTaskOutputs} onChange={(e) => updateActiveCrew({ shareAllTaskOutputs: e.target.checked })} /> Vorherige Task-Ergebnisse global als Kontext teilen</label>
                    </div>
                  )}
                </div>

                {/* Section: Provider & Modell */}
                <div className={`crew-section${openSections.provider ? ' open' : ''}`}>
                  <button type="button" className="crew-section-header" onClick={() => toggleSection('provider')}>
                    <span className="crew-section-icon">🔌</span> Provider & Modell
                    <span className="crew-section-chevron">▾</span>
                  </button>
                  {openSections.provider && (
                    <div className="crew-section-body">
                      <div className="crew-form-row">
                        <div className="crew-form-group">
                          <span className="crew-label">Crew-Provider</span>
                          <select aria-label="Crew-Provider" className="crew-select" value={activeCrew.defaultProvider || 'ollama'} onChange={(e) => handleCrewDefaultProviderChange(e.target.value as CrewProviderKind)}>
                            {PROVIDER_OPTIONS.map((o) => {
                              const ok = o.value === activeCrew.defaultProvider || isProviderEnabledForCrew(o.value)
                              return <option key={o.value} value={o.value} disabled={!ok}>{ok ? o.label : `${o.label} (Profil aktivieren)`}</option>
                            })}
                          </select>
                          <span className="crew-hint">Der Crew-Provider gilt fuer alle Mitglieder. Pro Mitglied ist nur noch das Modell ueberschreibbar.</span>
                        </div>
                        <div className="crew-form-group">
                          <span className="crew-label">Crew-Modell</span>
                          <select className="crew-select" value={activeCrew.defaultModel || ''} onChange={(e) => updateActiveCrew({ defaultModel: e.target.value })}>
                            <option value="">Globale Einstellungen verwenden</option>
                            {getCrewDefaultModelOptions().map((m) => <option key={m} value={m}>{m}</option>)}
                          </select>
                          {activeCrew.defaultProvider !== 'ollama' && getCrewDefaultModelOptions().length === 0 && (
                            <span className="crew-hint">Für diesen Provider sind noch keine Modelle geladen.</span>
                          )}
                          <span className="crew-hint">Aktuell wirksam: {activeCrew.defaultModel || 'Globale Einstellungen'}</span>
                          <span className="crew-hint">Gilt automatisch für alle Mitglieder ohne eigenes Modell-Override.</span>
                        </div>
                      </div>
                    </div>
                  )}
                </div>

                {/* Diagnostics */}
                <div className={`crew-section${openSections.diagnostics ? ' open' : ''}`}>
                  <button type="button" className="crew-section-header" onClick={() => toggleSection('diagnostics')}>
                    <span className="crew-section-icon">🩺</span> Diagnostik
                    <span className="crew-section-chevron">▾</span>
                  </button>
                  {openSections.diagnostics && (
                    <div className="crew-section-body">
                      {activeCrewDiagnostics.errors.length === 0 && activeCrewDiagnostics.warnings.length === 0 ? (
                        <div className="crew-alert success"><span className="crew-alert-icon">✅</span> Keine Blocker gefunden. Crew ist startbereit.</div>
                      ) : (
                        <>
                          {activeCrewDiagnostics.errors.map((entry) => (
                            <div key={`e-${entry}`} className="crew-alert error"><span className="crew-alert-icon">🚫</span> {entry}</div>
                          ))}
                          {activeCrewDiagnostics.warnings.map((entry) => (
                            <div key={`w-${entry}`} className="crew-alert warning"><span className="crew-alert-icon">⚠️</span> {entry}</div>
                          ))}
                        </>
                      )}
                    </div>
                  )}
                </div>

                {/* Agents */}
                <div className="crew-section open">
                  <div className="crew-section-header" style={{ cursor: 'default' }}>
                    <span className="crew-section-icon">👥</span> Crew-Mitglieder ({activeAgentCount}/{activeCrew.agents.length})
                  </div>
                  <div className="crew-section-body">
                    <div className="crew-members-hero">
                      <div className="crew-members-copy">
                        <div className="crew-overview-kicker">Mitglieder</div>
                        <strong className="crew-overview-title">Rollen, Modelle und Zugriffe pro Agent</strong>
                        <div className="crew-overview-description">
                          Profile, Laufzeitverhalten und Berechtigungen bleiben hier gesammelt sichtbar, damit die Crew-Konfiguration nicht in einzelne Formblöcke zerfällt.
                        </div>
                      </div>
                      <div className="crew-members-stats">
                        <div className="crew-members-stat">
                          <strong>{activeAgentCount}</strong>
                          <span>aktiv</span>
                        </div>
                        <div className="crew-members-stat">
                          <strong>{profileBackedAgentCount}</strong>
                          <span>mit Profil</span>
                        </div>
                        <div className="crew-members-stat">
                          <strong>{configuredToolCount}</strong>
                          <span>Tools genutzt</span>
                        </div>
                        <div className="crew-members-stat">
                          <strong>{configuredMcpCount}</strong>
                          <span>MCP-Zugriffe</span>
                        </div>
                      </div>
                    </div>
                    <div className="crew-agent-panel crew-agent-panel-wide crew-bulk-access-panel">
                      <div className="crew-agent-panel-header crew-agent-panel-header-split">
                        <div>
                          <div className="crew-agent-panel-title">Freigaben fuer alle Mitglieder</div>
                          <div className="crew-agent-panel-subtitle">Setzt Tool- und MCP-Zugriffe global fuer die aktive Crew.</div>
                        </div>
                        <div className="crew-members-badge-row">
                          <span className="crew-inline-badge subtle">{claudeTools.length} Tools verfuegbar</span>
                          <span className="crew-inline-badge subtle">{configuredMcpServers.length} MCP-Server</span>
                        </div>
                      </div>
                      <div className="crew-agent-access-grid">
                        <div className="crew-agent-subpanel">
                          <div className="crew-form-group">
                            <div className="crew-subpanel-head">
                              <span className="crew-label">Tools</span>
                              <span className="crew-subpanel-count">Alle Mitglieder synchronisieren</span>
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
                                    {tool.label}
                                  </label>
                                )
                              })}
                            </div>
                          </div>
                        </div>
                        <div className="crew-agent-subpanel">
                          <div className="crew-form-group">
                            <div className="crew-subpanel-head">
                              <span className="crew-label">MCP-Zugriffe</span>
                              <span className="crew-subpanel-count">Workspace-Verbindungen</span>
                            </div>
                            {configuredMcpServers.length === 0 ? <span className="crew-hint">Keine MCP-Server konfiguriert.</span> : (
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
                            <div className="crew-agent-header" onClick={() => toggleAgent(agent.id)}>
                              <div className="crew-agent-avatar">{roleEmoji(profileAgent.role)}</div>
                              <div className="crew-agent-info">
                                <div className="crew-agent-name">{profileAgent.name}</div>
                                <div className="crew-agent-role">{profileAgent.role}</div>
                                <div className="crew-agent-summary">
                                  <span className="crew-inline-badge">{getProviderLabel(effectiveProviderKind)}</span>
                                  <span className="crew-inline-badge subtle">{effectiveModelLabel}</span>
                                  {profile && <span className="crew-inline-badge subtle">globales Profil</span>}
                                </div>
                              </div>
                              <div className="crew-agent-header-actions">
                                <span className={`crew-badge ${agent.enabled ? 'active' : 'inactive'}`}>{agent.enabled ? 'Aktiv' : 'Inaktiv'}</span>
                                <span className="crew-agent-chevron">▾</span>
                              </div>
                            </div>
                            {isOpen && (
                              <div className="crew-agent-body">
                                <div className="crew-agent-panel">
                                  <div className="crew-agent-panel-header">
                                    <div className="crew-agent-panel-title">Profil</div>
                                    <div className="crew-agent-panel-subtitle">Name, Rolle und Arbeitskontext des Crew-Mitglieds.</div>
                                  </div>
                                  <div className="crew-agent-col">
                                    <div className="crew-form-row">
                                      {profile && (
                                        <div className="crew-form-group">
                                          <span className="crew-label">Icon</span>
                                          <input className="crew-input" value={profile.icon ?? ''} maxLength={4} onChange={(e) => updateCrewPersonalityProfile(profile, { icon: e.target.value || null })} />
                                        </div>
                                      )}
                                      <div className="crew-form-group"><span className="crew-label">Name</span><input className="crew-input" value={profileAgent.name} onChange={(e) => updateProfileOrSnapshot({ name: e.target.value })} /></div>
                                      <div className="crew-form-group"><span className="crew-label">Rolle</span><select className="crew-select" value={profileAgent.role} onChange={(e) => updateProfileOrSnapshot({ role: e.target.value as AgentRole })}>{ROLE_OPTIONS.map((r) => <option key={r} value={r}>{r}</option>)}</select></div>
                                      {profile && (
                                        <div className="crew-form-group">
                                          <span className="crew-label">Temperatur</span>
                                          <input className="crew-input" type="number" min={0} max={2} step={0.1} value={profile.temperature ?? ''} onChange={(e) => updateCrewPersonalityProfile(profile, { temperature: e.target.value === '' ? null : Number(e.target.value) })} />
                                        </div>
                                      )}
                                    </div>
                                    {profile && (
                                      <label className="crew-checkbox-label"><input type="checkbox" checked={profile.isDefault ?? false} onChange={(e) => updateCrewPersonalityProfile(profile, { isDefault: e.target.checked })} /> Als Standard verwenden</label>
                                    )}
                                    <div className="crew-form-group"><span className="crew-label">Ziel / Prompt-Fokus</span><AutoResizeTextarea className="crew-textarea" value={profileAgent.goal} onChange={(e) => updateProfileOrSnapshot({ goal: e.target.value })} /></div>
                                    <div className="crew-form-group"><span className="crew-label">Hintergrund / System-Prompt</span><AutoResizeTextarea className="crew-textarea" value={profileAgent.backstory} onChange={(e) => updateProfileOrSnapshot({ backstory: e.target.value })} /></div>
                                    <div className="crew-form-group"><span className="crew-label">skills.md</span><AutoResizeTextarea className="crew-textarea" value={profileAgent.skillsMarkdown} onChange={(e) => updateProfileOrSnapshot({ skillsMarkdown: e.target.value })} placeholder="# skills.md&#10;- Projektkontext&#10;- Arbeitsstil" /></div>
                                    <span className="crew-hint">{profile ? 'Profilfelder werden global fuer alle Crews synchronisiert.' : 'Lokaler Snapshot ohne aktive Profil-Synchronisation.'}</span>
                                  </div>
                                </div>
                                <div className="crew-agent-panel">
                                  <div className="crew-agent-panel-header">
                                    <div className="crew-agent-panel-title">Konfiguration</div>
                                    <div className="crew-agent-panel-subtitle">Status, Provider, Modell und Laufzeitverhalten.</div>
                                  </div>
                                  <div className="crew-agent-col">
                                    <div className="crew-checkbox-stack">
                                      <label className="crew-checkbox-label"><input type="checkbox" checked={agent.enabled} onChange={(e) => updateActiveCrewAgent(agent.id, { enabled: e.target.checked })} /> Aktiviert</label>
                                      <label className="crew-checkbox-label"><input type="checkbox" checked={agent.allowDelegation} onChange={(e) => updateActiveCrewAgent(agent.id, { allowDelegation: e.target.checked })} /> Delegation erlaubt</label>
                                      <label className="crew-checkbox-label"><input type="checkbox" checked={agent.verbose} onChange={(e) => updateActiveCrewAgent(agent.id, { verbose: e.target.checked })} /> Verbose Logs</label>
                                    </div>
                                    <div className="crew-form-row">
                                      <div className="crew-form-group">
                                        <span className="crew-label">Provider</span>
                                        <input className="crew-input" value={getProviderLabel(effectiveProviderKind)} readOnly />
                                        <span className="crew-hint">Wird vom Crew-Provider gesteuert.</span>
                                      </div>
                                      <div className="crew-form-group">
                                        <span className="crew-label">Max Iterationen</span>
                                        <input className="crew-input" type="number" min={1} max={100} value={agent.maxIterations} onChange={(e) => updateActiveCrewAgent(agent.id, { maxIterations: Number(e.target.value) || 1 })} />
                                      </div>
                                    </div>
                                    <div className="crew-form-group">
                                      <span className="crew-label">Modell</span>
                                      <select className="crew-select" value={selModel} onChange={(e) => updateProfileOrSnapshot({ modelOverride: e.target.value || null })}>
                                        <option value="">Crew-Modell ({getCrewDefaultModelLabel(effectiveProviderKind)})</option>
                                        {amo.map((m) => <option key={m} value={m}>{m}</option>)}
                                      </select>
                                      {effectiveProviderKind !== 'ollama' && pmc.models.length === 0 && <span className="crew-hint">Keine Modelle geladen.</span>}
                                      <span className="crew-hint">Aktuell wirksam: {effectiveModelLabel}</span>
                                    </div>
                                  </div>
                                </div>
                                <div className="crew-agent-panel crew-agent-panel-wide">
                                  <div className="crew-agent-panel-header">
                                    <div className="crew-agent-panel-title">Werkzeuge & Zugriffe</div>
                                    <div className="crew-agent-panel-subtitle">Alle verfügbaren Funktionen bleiben erhalten, sind hier aber übersichtlicher gruppiert.</div>
                                  </div>
                                  <div className="crew-agent-access-grid">
                                    <div className="crew-agent-subpanel">
                                      <div className="crew-form-group">
                                        <span className="crew-label">Tools</span>
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
                                              {tool.label}
                                            </label>
                                          ))}
                                        </div>
                                      </div>
                                    </div>
                                    <div className="crew-agent-subpanel">
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
                <div className="crew-task-rail">
                  <div className="crew-task-rail-copy">
                    <div className="crew-overview-kicker">Task-Flow</div>
                    <strong className="crew-overview-title">Tasks werden unter /tasks erstellt, ausgefuehrt und geplant</strong>
                    <div className="crew-overview-description">
                      Diese Crew-Ansicht steuert dafuer Parallelitaet, Retry-Verhalten, Ergebnis-Sharing und das Ausgabeformat der Laufzeit.
                    </div>
                  </div>
                  <div className="crew-task-rail-metrics">
                    <div className="crew-task-metric">
                      <span>Parallel</span>
                      <strong>{activeCrew.maxParallelTasks}</strong>
                    </div>
                    <div className="crew-task-metric">
                      <span>Retries</span>
                      <strong>{activeCrew.retryCount}</strong>
                    </div>
                    <div className="crew-task-metric">
                      <span>Ausgabe</span>
                      <strong>{outputModeLabel}</strong>
                    </div>
                    <div className="crew-task-metric">
                      <span>Kontext teilen</span>
                      <strong>{activeCrew.shareAllTaskOutputs ? 'Ja' : 'Nein'}</strong>
                    </div>
                  </div>
                </div>
              </>
            ) : (
              <div className="crew-empty">
                <div className="crew-empty-icon">👈</div>
                <div className="crew-empty-text">Wähle eine Crew aus der Liste aus.</div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  )
}

