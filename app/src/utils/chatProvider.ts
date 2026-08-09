import type {
  ApiProfilePreset,
  BackendKind,
  DefaultLlmProfileIds,
  LlmProfile,
  OllamaConfig,
} from '../stores/configStore'

export type ChatProviderKind = BackendKind

export const CHAT_PROVIDER_OPTIONS: ChatProviderKind[] = ['codex', 'openai-compatible']

export const CHAT_PROVIDER_LABELS: Record<ChatProviderKind, string> = {
  codex: 'Codex verwenden',
  'openai-compatible': 'OpenAI-kompatible API',
}

export type ChatProviderContext = {
  ollama: OllamaConfig
  availableModels: string[]
  llmProfiles: LlmProfile[]
  defaultLlmProfileIds: DefaultLlmProfileIds
  llmProfileModels: Record<string, string[]>
}

export type ChatProviderState = {
  provider: ChatProviderKind
  backend: ChatProviderKind
  label: string
  endpoint: string
  model: string
  apiKey: string
  timeoutMs: number
  verifyTlsCertificates: boolean
  contextWindow: number
  selectableModels: string[]
  profileId?: string
  authProfileId?: string
  reasoningEffort?: string
  preset?: ApiProfilePreset
  compatibilityProvider?: 'openai-compatible' | 'openrouter'
}

export type ChatProviderSelection =
  | {
      backend: 'codex'
      authProfileId?: string
      model?: string
      reasoningEffort?: string
    }
  | {
      backend: 'openai-compatible'
      profileId: string
      model?: string
    }

function resolveDefaultProfile(
  profiles: LlmProfile[],
  defaultIds: DefaultLlmProfileIds,
): LlmProfile | undefined {
  return profiles.find((profile) => profile.id === defaultIds.api)
    ?? profiles.find((profile) => profile.id === defaultIds.ollama)
    ?? profiles[0]
}

function modelSuffix(model: string): string {
  const trimmed = model.trim()
  return trimmed.split('/').filter(Boolean).at(-1) ?? trimmed
}

function resolveExternalModel(
  selectedModel: string,
  profileModel: string,
  selectableModels: string[],
): string {
  if (!selectedModel) return profileModel

  const normalizedModels = selectableModels.map((model) => model.trim()).filter(Boolean)
  if (normalizedModels.length > 0) {
    const lowerSelected = selectedModel.toLowerCase()
    const exactSelected = normalizedModels.find((model) => model.toLowerCase() === lowerSelected)
    if (exactSelected) return exactSelected

    const suffixSelected = normalizedModels.find((model) => modelSuffix(model).toLowerCase() === lowerSelected)
    if (suffixSelected) return suffixSelected

    if (profileModel) {
      const lowerProfile = profileModel.toLowerCase()
      const exactProfile = normalizedModels.find((model) => model.toLowerCase() === lowerProfile)
      if (exactProfile) return exactProfile

      const suffixProfile = normalizedModels.find((model) => modelSuffix(model).toLowerCase() === lowerProfile)
      if (suffixProfile) return suffixProfile
    }

    return profileModel || selectedModel
  }

  if (profileModel && profileModel.toLowerCase() !== selectedModel.toLowerCase()) {
    const lowerSelected = selectedModel.toLowerCase()
    if (modelSuffix(profileModel).toLowerCase() === lowerSelected) return profileModel
  }

  return selectedModel || profileModel
}

function uniqueModels(models: string[]): string[] {
  const seen = new Set<string>()
  return models
    .map((model) => model.trim())
    .filter((model) => Boolean(model) && !seen.has(model) && Boolean(seen.add(model)))
}

export function normalizeChatProvider(value: unknown): ChatProviderKind {
  return value === 'codex' ? 'codex' : 'openai-compatible'
}

export function normalizeChatProviderSelection(value: unknown): ChatProviderSelection | undefined {
  if (!value || typeof value !== 'object') return undefined

  const raw = value as Record<string, unknown>
  const legacyProvider = typeof raw.provider === 'string' ? raw.provider : ''
  const backend = raw.backend === 'codex' || legacyProvider === 'codex'
    ? 'codex'
    : 'openai-compatible'
  const model = typeof raw.model === 'string' ? raw.model.trim() : ''

  if (backend === 'codex') {
    const authProfileId = typeof raw.authProfileId === 'string' ? raw.authProfileId.trim() : ''
    const reasoningEffort = typeof raw.reasoningEffort === 'string' ? raw.reasoningEffort.trim() : ''
    return {
      backend,
      ...(authProfileId ? { authProfileId } : {}),
      ...(model ? { model } : {}),
      ...(reasoningEffort ? { reasoningEffort } : {}),
    }
  }

  const profileId = typeof raw.profileId === 'string' ? raw.profileId.trim() : ''
  return {
    backend,
    profileId,
    ...(model ? { model } : {}),
  }
}

export function createChatProviderSelection(
  state: Pick<ChatProviderState, 'backend' | 'model' | 'profileId' | 'authProfileId' | 'reasoningEffort'>,
): ChatProviderSelection {
  if (state.backend === 'codex') {
    return {
      backend: 'codex',
      ...(state.authProfileId?.trim() ? { authProfileId: state.authProfileId.trim() } : {}),
      ...(state.model.trim() ? { model: state.model.trim() } : {}),
      ...(state.reasoningEffort?.trim() ? { reasoningEffort: state.reasoningEffort.trim() } : {}),
    }
  }

  return {
    backend: 'openai-compatible',
    profileId: state.profileId?.trim() ?? '',
    ...(state.model.trim() ? { model: state.model.trim() } : {}),
  }
}

export function getChatProviderState(
  context: ChatProviderContext,
  rawProvider: unknown,
  rawSelection?: ChatProviderSelection | Record<string, unknown>,
): ChatProviderState {
  const selection = normalizeChatProviderSelection(rawSelection)
  const provider = normalizeChatProvider(selection?.backend ?? rawProvider)
  const selectedModel = selection?.model?.trim() ?? ''

  if (provider === 'codex') {
    const codexSelection = selection?.backend === 'codex' ? selection : undefined
    return {
      provider,
      backend: provider,
      label: CHAT_PROVIDER_LABELS[provider],
      endpoint: '',
      model: selectedModel,
      apiKey: '',
      timeoutMs: 600000,
      verifyTlsCertificates: true,
      contextWindow: 128000,
      selectableModels: [],
      authProfileId: codexSelection?.authProfileId,
      reasoningEffort: codexSelection?.reasoningEffort,
    }
  }

  const profiles = Array.isArray(context.llmProfiles) ? context.llmProfiles : []
  const profileModelMap = context.llmProfileModels ?? {}
  const requestedProfileId = selection?.backend === 'openai-compatible' ? selection.profileId : ''
  const profile = (requestedProfileId ? profiles.find((item) => item.id === requestedProfileId) : undefined)
    ?? resolveDefaultProfile(profiles, context.defaultLlmProfileIds)
  const loadedModels = profile ? (profileModelMap[profile.id] ?? []) : []
  const selectableModels = uniqueModels([
    ...(profile?.preset === 'ollama' && Array.isArray(context.availableModels) ? context.availableModels : []),
    ...loadedModels,
    ...(profile?.model ? [profile.model] : []),
  ])
  const model = resolveExternalModel(
    selectedModel,
    profile?.model?.trim() ?? '',
    loadedModels.length > 0 ? selectableModels : [],
  )

  return {
    provider,
    backend: provider,
    label: CHAT_PROVIDER_LABELS[provider],
    endpoint: profile?.baseUrl?.trim() ?? '',
    model,
    apiKey: profile?.authMode === 'none' ? '' : (profile?.apiKey?.trim() ?? ''),
    timeoutMs: Math.max(1000, Number(profile?.timeoutMs ?? 600000)),
    verifyTlsCertificates: profile?.verifyTlsCertificates ?? true,
    contextWindow: Math.max(512, profile?.contextWindow ?? 128000),
    selectableModels,
    profileId: profile?.id,
    preset: profile?.preset,
    compatibilityProvider: profile?.preset === 'openrouter' ? 'openrouter' : 'openai-compatible',
  }
}

export function getChatProviderFailureHint(provider: ChatProviderKind): string {
  return provider === 'codex'
    ? 'Check the selected Codex account in Settings and sign in again if needed.'
    : 'Check the API profile, endpoint, access key, and model in Settings.'
}
