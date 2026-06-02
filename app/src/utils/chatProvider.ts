import type {
  DefaultLlmProfileIds,
  LlmProfile,
  OllamaConfig,
} from '../stores/configStore'

export type ChatProviderKind = 'ollama' | 'openai-compatible' | 'openrouter'

export const CHAT_PROVIDER_OPTIONS: ChatProviderKind[] = ['ollama', 'openai-compatible', 'openrouter']

export const CHAT_PROVIDER_LABELS: Record<ChatProviderKind, string> = {
  ollama: 'Ollama',
  'openai-compatible': 'OpenAI-kompatibel',
  openrouter: 'OpenRouter',
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
  label: string
  endpoint: string
  model: string
  apiKey: string
  timeoutMs: number
  verifyTlsCertificates: boolean
  selectableModels: string[]
  profileId?: string
}

export type ChatProviderSelection = {
  provider: ChatProviderKind
  model?: string
  profileId?: string
}

function resolveDefaultProfile(
  profiles: LlmProfile[],
  defaultIds: DefaultLlmProfileIds,
  provider: Exclude<ChatProviderKind, 'ollama'>,
): LlmProfile | undefined {
  return profiles.find((profile) => profile.id === defaultIds[provider] && profile.provider === provider)
    ?? profiles.find((profile) => profile.provider === provider)
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
    if (modelSuffix(profileModel).toLowerCase() === lowerSelected) {
      return profileModel
    }
  }

  return selectedModel || profileModel
}

export function normalizeChatProvider(value: unknown): ChatProviderKind {
  return value === 'openai-compatible' || value === 'openrouter' || value === 'ollama'
    ? value
    : 'ollama'
}

export function normalizeChatProviderSelection(value: unknown): ChatProviderSelection | undefined {
  if (!value || typeof value !== 'object') {
    return undefined
  }

  const raw = value as Record<string, unknown>
  const provider = normalizeChatProvider(raw.provider)
  const model = typeof raw.model === 'string' ? raw.model.trim() : ''
  const profileId = typeof raw.profileId === 'string' ? raw.profileId.trim() : ''

  return {
    provider,
    ...(model ? { model } : {}),
    ...(profileId ? { profileId } : {}),
  }
}

export function createChatProviderSelection(state: Pick<ChatProviderState, 'provider' | 'model' | 'profileId'>): ChatProviderSelection {
  return {
    provider: state.provider,
    ...(state.model.trim() ? { model: state.model.trim() } : {}),
    ...(state.profileId?.trim() ? { profileId: state.profileId.trim() } : {}),
  }
}

export function getChatProviderState(
  context: ChatProviderContext,
  rawProvider: unknown,
  selection?: ChatProviderSelection,
): ChatProviderState {
  const provider = normalizeChatProvider(selection?.provider ?? rawProvider)
  const selectedModel = selection?.model?.trim() ?? ''

  if (provider === 'ollama') {
    return {
      provider,
      label: CHAT_PROVIDER_LABELS[provider],
      endpoint: context.ollama.baseUrl,
      model: selectedModel || context.ollama.model,
      apiKey: '',
      timeoutMs: context.ollama.timeoutMs,
      verifyTlsCertificates: true,
      selectableModels: Array.isArray(context.availableModels) ? context.availableModels : [],
    }
  }

  const selectedProfile = selection?.profileId
    ? context.llmProfiles.find((item) => item.id === selection.profileId && item.provider === provider)
    : undefined
  const profile = selectedProfile ?? resolveDefaultProfile(context.llmProfiles, context.defaultLlmProfileIds, provider)
  const selectableModels = profile ? (context.llmProfileModels[profile.id] ?? []) : []
  const profileModel = profile?.model?.trim() || ''
  const model = resolveExternalModel(selectedModel, profileModel, selectableModels)

  return {
    provider,
    label: CHAT_PROVIDER_LABELS[provider],
    endpoint: profile?.baseUrl?.trim() ?? '',
    model,
    apiKey: profile?.apiKey?.trim() ?? '',
    timeoutMs: Math.max(1000, Number(profile?.timeoutMs ?? 600000)),
    verifyTlsCertificates: profile?.verifyTlsCertificates ?? true,
    selectableModels,
    profileId: profile?.id,
  }
}

export function getChatProviderFailureHint(provider: ChatProviderKind): string {
  if (provider === 'openai-compatible') {
    return 'Pruefe unter Einstellungen das OpenAI-kompatible Profil, den Endpoint, den API-Key und das Modell.'
  }

  if (provider === 'openrouter') {
    return 'Pruefe unter Einstellungen das OpenRouter-Profil, den Endpoint, den API-Key und das Modell.'
  }

  return 'Pruefe unter Einstellungen den Ollama-Endpoint, das Modell und den Timeout.'
}
