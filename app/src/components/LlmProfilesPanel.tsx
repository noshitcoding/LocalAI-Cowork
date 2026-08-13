import { useEffect, useMemo, useRef, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { ChevronDown } from 'lucide-react'
import { checkOllamaConnection, listOllamaModels } from '../engine/api/ollamaClient'
import {
  useConfigStore,
  type ApiProfilePreset,
  type DefaultLlmProfileIds,
  type LlmProfile,
} from '../stores/configStore'
import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'
import { resolveProviderModelFromCatalog } from '../utils/providerModels'
import { getModelGuidance } from '../utils/modelGuidance'
import { tr } from '../i18n'
import SecureCredentialInput from './SecureCredentialInput'
import CodexAccountsPanel from './CodexAccountsPanel'

type ExternalProviderHealthCheckResult = {
  reachable: boolean
  status: number | null
  endpoint: string
  message: string
  checkedAt: string
}

type ExternalProviderModelsResult = {
  endpoint: string
  models: string[]
}

type ProfileHealthState = {
  loading: boolean
  reachable?: boolean
  endpoint?: string
  message?: string
}

type ProfileModelsState = {
  loading: boolean
  endpoint?: string
  models: string[]
  error?: string
  message?: string
}

const PROVIDER_ORDER: ApiProfilePreset[] = ['ollama', 'openai', 'openrouter', 'custom']

function parseRequestedPreset(value: string | null): ApiProfilePreset | null {
  if (value === 'openai-compatible') return 'openai'
  return PROVIDER_ORDER.find((preset) => preset === value) ?? null
}

const PROVIDER_LABELS: Record<ApiProfilePreset, string> = {
  ollama: 'Ollama',
  openai: 'OpenAI',
  openrouter: 'OpenRouter',
  custom: 'Custom API',
}

const PROVIDER_PLACEHOLDERS: Record<ApiProfilePreset, { baseUrl: string; model: string }> = {
  ollama: {
    baseUrl: 'http://localhost:11434/v1',
    model: 'llama3.1:8b',
  },
  openai: {
    baseUrl: 'https://api.openai.com/v1',
    model: 'gpt-4.1-mini',
  },
  openrouter: {
    baseUrl: 'https://openrouter.ai/api/v1',
    model: 'openai/gpt-4o-mini',
  },
  custom: {
    baseUrl: 'http://localhost:8000/v1',
    model: 'model-name',
  },
}

function parseNumericInput(raw: string, fallback: number): number {
  const parsed = Number(raw.replace(',', '.').trim())
  return Number.isFinite(parsed) ? parsed : fallback
}

function supportsApiKey(preset: ApiProfilePreset | undefined): boolean {
  return preset !== 'ollama'
}

function getDefaultProfileId(defaultIds: DefaultLlmProfileIds, preset: ApiProfilePreset): string {
  if (preset === 'ollama') return defaultIds.ollama
  if (preset === 'openrouter') return defaultIds.openrouter
  if (preset === 'openai') return defaultIds['openai-compatible']
  return defaultIds.api ?? defaultIds.ollama
}

function getRoutingProvider(profile: LlmProfile): 'openai-compatible' | 'openrouter' {
  return profile.preset === 'openrouter' ? 'openrouter' : 'openai-compatible'
}

function getOllamaBaseUrl(profile: LlmProfile): string {
  return profile.baseUrl.replace(/\/v1\/?$/i, '')
}

function resolveLoadedModel(currentModel: string, models: string[]): string {
  return resolveProviderModelFromCatalog(currentModel, models)
}

export default function LlmProfilesPanel() {
  const {
    llmProfiles,
    defaultLlmProfileIds,
    llmProfileModels,
    addLlmProfile,
    updateLlmProfile,
    setLlmProfileApiKey,
    deleteLlmProfile,
    setDefaultLlmProfile,
    setLlmProfileModels,
  } = useConfigStore()

  const [searchParams] = useSearchParams()
  const requestedProviderParam = searchParams.get('provider')
  const requestedProvider = parseRequestedPreset(requestedProviderParam)
  const appliedRequestedProvider = useRef<ApiProfilePreset | null>(null)

  const [healthChecks, setHealthChecks] = useState<Record<string, ProfileHealthState>>({})
  const [modelStates, setModelStates] = useState<Record<string, ProfileModelsState>>({})

  const profilesByProvider = useMemo(
    () => PROVIDER_ORDER.map((provider) => ({
      provider,
      profiles: llmProfiles.filter((profile) => profile.preset === provider),
    })).filter(({ profiles }) => profiles.length > 0),
    [llmProfiles],
  )
  const [expandedProvider, setExpandedProvider] = useState<ApiProfilePreset | null>(() => {
    if (requestedProvider) return requestedProvider
    const openRouterProfile = llmProfiles.find((profile) => (
      profile.id === defaultLlmProfileIds.openrouter && profile.preset === 'openrouter'
    ))
    return openRouterProfile?.model.trim() ? 'openrouter' : 'ollama'
  })

  useEffect(() => {
    if (!requestedProvider || appliedRequestedProvider.current === requestedProvider) return
    appliedRequestedProvider.current = requestedProvider
    setExpandedProvider(requestedProvider)

    const frame = window.requestAnimationFrame(() => {
      const section = document.getElementById(`llm-provider-${requestedProvider}`)
      if (typeof section?.scrollIntoView === 'function') {
        section.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
      }
      section?.querySelector<HTMLInputElement>('.llm-profile-api-key-field input')?.focus({ preventScroll: true })
    })

    return () => window.cancelAnimationFrame(frame)
  }, [requestedProvider])

  const openProvider = (provider: ApiProfilePreset) => {
    setExpandedProvider(provider)
    window.requestAnimationFrame(() => {
      const section = document.getElementById(`llm-provider-${provider}`)
      if (typeof section?.scrollIntoView === 'function') {
        section.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
      }
    })
  }

  const getProfileStatus = (profile: LlmProfile | undefined) => {
    if (!profile) return { label: tr('Not configured'), tone: 'warning' }

    const health = healthChecks[profile.id]
    if (health?.loading) return { label: tr('Checking...'), tone: 'neutral' }
    if (health?.reachable === true) return { label: tr('Connected'), tone: 'success' }
    if (health?.reachable === false) return { label: tr('Action needed'), tone: 'warning' }
    if (!profile.baseUrl.trim() || !profile.model.trim()) return { label: tr('Setup needed'), tone: 'warning' }
    if (supportsApiKey(profile.preset) && !profile.hasApiKey) return { label: tr('Access key needed'), tone: 'warning' }
    return { label: tr('Configured'), tone: 'neutral' }
  }

  const handleAddProfile = (provider: ApiProfilePreset) => {
    addLlmProfile(provider)
  }

  const handleOllamaHealthCheck = async (profile: LlmProfile) => {
    setHealthChecks((current) => ({
      ...current,
      [profile.id]: {
        loading: true,
        endpoint: profile.baseUrl,
      },
    }))

    try {
      const [reachable, models] = await Promise.all([
        checkOllamaConnection(getOllamaBaseUrl(profile)),
        listOllamaModels(getOllamaBaseUrl(profile)).catch(() => []),
      ])
      const modelNames = models.map((model) => model.name)
      setLlmProfileModels(profile.id, modelNames)
      setModelStates((current) => ({
        ...current,
        [profile.id]: {
          loading: false,
          endpoint: profile.baseUrl,
          models: modelNames,
          message: undefined,
        },
      }))
      setHealthChecks((current) => ({
        ...current,
        [profile.id]: {
          loading: false,
          reachable,
          endpoint: profile.baseUrl,
          message: reachable ? 'Connected' : 'Ollama is not reachable.',
        },
      }))
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      setHealthChecks((current) => ({
        ...current,
        [profile.id]: {
          loading: false,
          reachable: false,
          endpoint: profile.baseUrl,
          message,
        },
      }))
    }
  }

  const handleExternalHealthCheck = async (profile: LlmProfile) => {
    setHealthChecks((current) => ({
      ...current,
      [profile.id]: {
        loading: true,
        endpoint: profile.baseUrl,
      },
    }))

    if (!hasTauriRuntime()) {
      setHealthChecks((current) => ({
        ...current,
        [profile.id]: {
          loading: false,
          reachable: false,
          endpoint: profile.baseUrl,
          message: 'Tauri runtime is not available - feature can only be used in the desktop app.',
        },
      }))
      return
    }

    try {
      const result = await safeInvoke<ExternalProviderHealthCheckResult>('crew_provider_health_check', {
        request: {
          providerKind: getRoutingProvider(profile),
          profileId: profile.id,
          baseUrl: profile.baseUrl,
          apiKey: '',
          model: profile.model,
          verifyTlsCertificates: profile.verifyTlsCertificates,
        },
      })

      setHealthChecks((current) => ({
        ...current,
        [profile.id]: {
          loading: false,
          reachable: result.reachable,
          endpoint: result.endpoint,
          message: result.message,
        },
      }))
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      setHealthChecks((current) => ({
        ...current,
        [profile.id]: {
          loading: false,
          reachable: false,
          message,
        },
      }))
    }
  }

  const handleHealthCheck = async (profile: LlmProfile) => {
    if (profile.preset === 'ollama') {
      await handleOllamaHealthCheck(profile)
      return
    }

    await handleExternalHealthCheck(profile)
  }

  const handleOllamaModelsLoad = async (profile: LlmProfile) => {
    setModelStates((current) => ({
      ...current,
      [profile.id]: {
        loading: true,
        endpoint: profile.baseUrl,
        models: current[profile.id]?.models ?? llmProfileModels[profile.id] ?? [],
      },
    }))

    try {
      const models = await listOllamaModels(getOllamaBaseUrl(profile))
      const modelNames = models.map((model) => model.name)
      setLlmProfileModels(profile.id, modelNames)
      setModelStates((current) => ({
        ...current,
        [profile.id]: {
          loading: false,
          endpoint: profile.baseUrl,
          models: modelNames,
          message: undefined,
        },
      }))
    } catch (error) {
      setModelStates((current) => ({
        ...current,
        [profile.id]: {
          loading: false,
          endpoint: profile.baseUrl,
          models: current[profile.id]?.models ?? llmProfileModels[profile.id] ?? [],
          error: error instanceof Error ? error.message : String(error),
        },
      }))
    }
  }

  const handleExternalModelsLoad = async (profile: LlmProfile) => {
    setModelStates((current) => ({
      ...current,
      [profile.id]: {
        loading: true,
        endpoint: profile.baseUrl,
        models: current[profile.id]?.models ?? llmProfileModels[profile.id] ?? [],
      },
    }))

    if (!hasTauriRuntime()) {
      setModelStates((current) => ({
        ...current,
        [profile.id]: {
          loading: false,
          endpoint: profile.baseUrl,
          models: current[profile.id]?.models ?? llmProfileModels[profile.id] ?? [],
          error: 'Tauri runtime is not available - feature can only be used in the desktop app.',
        },
      }))
      return
    }

    try {
      const result = await safeInvoke<ExternalProviderModelsResult>('crew_provider_models_list', {
        request: {
          providerKind: getRoutingProvider(profile),
          profileId: profile.id,
          baseUrl: profile.baseUrl,
          apiKey: '',
          model: profile.model,
          verifyTlsCertificates: profile.verifyTlsCertificates,
        },
      })

      const resolvedModel = resolveLoadedModel(profile.model, result.models)
      const modelChanged = resolvedModel.trim() !== profile.model.trim()
      if (resolvedModel && modelChanged) {
        updateLlmProfile(profile.id, { model: resolvedModel })
      }

      setLlmProfileModels(profile.id, result.models)
      setModelStates((current) => ({
        ...current,
        [profile.id]: {
          loading: false,
          endpoint: result.endpoint,
          models: result.models,
          message: modelChanged
            ? tr('Model automatically set to {{model}}.', { model: resolvedModel })
            : undefined,
          error: !modelChanged && profile.model.trim() && result.models.length > 0 && !result.models.includes(profile.model.trim())
            ? `Configured model ${profile.model.trim()} is not in the loaded model list.`
            : undefined,
        },
      }))
    } catch (error) {
      setModelStates((current) => ({
        ...current,
        [profile.id]: {
          loading: false,
          endpoint: profile.baseUrl,
          models: current[profile.id]?.models ?? llmProfileModels[profile.id] ?? [],
          error: error instanceof Error ? error.message : String(error),
        },
      }))
    }
  }

  const handleLoadModels = async (profile: LlmProfile) => {
    if (profile.preset === 'ollama') {
      await handleOllamaModelsLoad(profile)
      return
    }

    await handleExternalModelsLoad(profile)
  }

  return (
    <>
    <CodexAccountsPanel />
    <div className="panel llm-profiles-panel">
      <div className="panel-heading-row">
        <h2>{tr('OpenAI-compatible API')}</h2>
        <div className="actions llm-profile-add-actions">
          <button type="button" className="btn-sm" onClick={() => handleAddProfile('ollama')}>{tr("+ Ollama")}</button>
          <button type="button" className="btn-sm" onClick={() => handleAddProfile('openai')}>{tr("+ OpenAI")}</button>
          <button type="button" className="btn-sm" onClick={() => handleAddProfile('openrouter')}>{tr("+ OpenRouter")}</button>
          <button type="button" className="btn-sm" onClick={() => handleAddProfile('custom')}>{tr("+ Custom API")}</button>
        </div>
      </div>
      <p className="hint-text">{tr('All API connections use the same adapter. Ollama, OpenRouter, and OpenAI are editable presets with suitable defaults.')}</p>
      <div className="llm-provider-default-row">
        <label htmlFor="global-api-profile">
          <span>{tr('Global default API profile')}</span>
          <select
            id="global-api-profile"
            value={defaultLlmProfileIds.api ?? llmProfiles[0]?.id ?? ''}
            onChange={(event) => setDefaultLlmProfile('openai-compatible', event.target.value)}
          >
            {llmProfiles.map((profile) => (
              <option key={profile.id} value={profile.id}>{profile.name} · {PROVIDER_LABELS[profile.preset ?? 'custom']}</option>
            ))}
          </select>
        </label>
      </div>

      <div className="llm-provider-overview" role="group" aria-label={tr('Provider overview')}>
        {profilesByProvider.map(({ provider, profiles }) => {
          const defaultProfile = profiles.find((profile) => profile.id === getDefaultProfileId(defaultLlmProfileIds, provider)) ?? profiles[0]
          const status = getProfileStatus(defaultProfile)
          const isFreeModel = provider === 'openrouter' && defaultProfile?.model.trim().endsWith(':free')

          return (
            <button
              key={provider}
              type="button"
              className={`llm-provider-overview-card${expandedProvider === provider ? ' active' : ''}`}
              aria-label={tr('Open {{provider}} settings', { provider: PROVIDER_LABELS[provider] })}
              aria-expanded={expandedProvider === provider}
              aria-controls={`llm-provider-${provider}`}
              onClick={() => openProvider(provider)}
            >
              <span className="llm-provider-overview-head">
                <strong>{PROVIDER_LABELS[provider]}</strong>
                <span className={`llm-provider-state tone-${status.tone}`}>{status.label}</span>
              </span>
              <span className="llm-provider-model">{defaultProfile?.model.trim() || tr('no model set')}</span>
              <span className="llm-provider-overview-meta">
                <span>{profiles.length} {tr(profiles.length === 1 ? 'profile' : 'profiles')}</span>
                {isFreeModel ? <span className="llm-provider-free-badge">{tr('Free model')}</span> : null}
              </span>
            </button>
          )
        })}
      </div>

      <div className="llm-provider-accordion">
        {profilesByProvider.map(({ provider, profiles }) => (
          <section
            key={provider}
            id={`llm-provider-${provider}`}
            className={`llm-provider-section${expandedProvider === provider ? ' open' : ''}`}
          >
            <button
              type="button"
              className="llm-provider-section-toggle"
              aria-expanded={expandedProvider === provider}
              aria-controls={`llm-provider-content-${provider}`}
              onClick={() => setExpandedProvider((current) => current === provider ? null : provider)}
            >
              <div>
                <strong>{PROVIDER_LABELS[provider]}</strong>
                <span>{profiles.length} {tr(profiles.length === 1 ? 'profile' : 'profiles')}</span>
              </div>
              <ChevronDown size={17} aria-hidden="true" />
            </button>

            {expandedProvider === provider ? (
              <div id={`llm-provider-content-${provider}`} className="llm-provider-section-content">
                {profiles.length === 0 ? (
                  <p className="panel-empty">{tr("No profile for")}{PROVIDER_LABELS[provider]}{tr("angelegt.")}</p>
                ) : (
                  <div className="llm-profile-list">
                {profiles.map((profile) => {
                  const isDefault = defaultLlmProfileIds.api === profile.id
                  const health = healthChecks[profile.id]
                  const models = llmProfileModels[profile.id] ?? []
                  const modelState = modelStates[profile.id]
                  const canDelete = !isDefault && profiles.length > 1
                  const guidance = getModelGuidance(profile.model)

                  return (
                    <div
                      key={profile.id}
                      className={`card llm-profile-card${isDefault ? ' is-default' : ''}`}
                    >
                      <div className="panel-heading-row llm-profile-card-header">
                        <div>
                          <strong>{profile.name}</strong>
                          {isDefault && <span className="llm-profile-default-badge">{tr("Default profile")}</span>}
                        </div>
                        <button
                          type="button"
                          className="btn-sm"
                          onClick={() => deleteLlmProfile(profile.id)}
                          disabled={!canDelete}
                          title={canDelete ? tr('Delete profile') : tr('Default profile or last profile cannot be deleted')}
                        >{tr("Delete")}</button>
                      </div>

                      <div className="grid llm-profile-fields">
                        <label>{tr("Profile name")}<input
                            value={profile.name}
                            onChange={(event) => updateLlmProfile(profile.id, { name: event.target.value })}
                          />
                        </label>
                        <label>{tr("Endpoint")}<input
                            value={profile.baseUrl}
                            onChange={(event) => updateLlmProfile(profile.id, { baseUrl: event.target.value })}
                            placeholder={PROVIDER_PLACEHOLDERS[profile.preset ?? provider].baseUrl}
                            style={{ fontFamily: 'monospace' }}
                          />
                        </label>
                        <label>{tr("Model")}{models.length > 0 ? (
                            <select value={profile.model} onChange={(event) => updateLlmProfile(profile.id, { model: event.target.value })}>
                              {models.map((model) => (
                                <option key={model} value={model}>
                                  {model} — {tr(getModelGuidance(model).title)}
                                </option>
                              ))}
                              {!models.includes(profile.model) && profile.model && (
                                <option value={profile.model}>{profile.model} — {tr(guidance.title)}</option>
                              )}
                            </select>
                          ) : (
                            <input
                              value={profile.model}
                              onChange={(event) => updateLlmProfile(profile.id, { model: event.target.value })}
                              placeholder={PROVIDER_PLACEHOLDERS[profile.preset ?? provider].model}
                              style={{ fontFamily: 'monospace' }}
                            />
                          )}
                        </label>
                        {supportsApiKey(profile.preset) && (
                          <label className="llm-profile-api-key-field">{tr("Access key for the application programming interface")}<SecureCredentialInput
                              value={profile.hasApiKey ? '••••••••••••' : ''}
                              onCommit={(value) => setLlmProfileApiKey(profile.id, value)}
                              placeholder={tr("sk?...")}
                              style={{ fontFamily: 'monospace' }}
                              ariaLabel={`${PROVIDER_LABELS[profile.preset ?? provider]} ${tr('Access key for the application programming interface')}`}
                            />
                          </label>
                        )}
                        {supportsApiKey(profile.preset) && (
                          <label className="toggle-row" style={{ alignSelf: 'end' }}>
                            <span>{tr("Check Secure Sockets Layer and Transport Layer Security certificates")}<span className="hint-text">{tr("Turn off for secure web connections with self-signed certificates.")}</span>
                            </span>
                            <button
                              type="button"
                              role="switch"
                              aria-checked={profile.verifyTlsCertificates}
                              className={`toggle-switch${profile.verifyTlsCertificates ? ' on' : ''}`}
                              onClick={() => updateLlmProfile(profile.id, { verifyTlsCertificates: !profile.verifyTlsCertificates })}
                            >
                              <span className="toggle-knob" />
                            </button>
                          </label>
                        )}
                        <label>{tr("Timeout in milliseconds")}<input
                            type="number"
                            min={1000}
                            max={86400000}
                            step={1000}
                            value={profile.timeoutMs}
                            onChange={(event) => updateLlmProfile(profile.id, { timeoutMs: parseNumericInput(event.target.value, profile.timeoutMs) })}
                          />
                        </label>
                        <label>{tr("Context window in tokens")}<input
                              type="number"
                              min={512}
                              max={2000000}
                              step={512}
                              value={profile.contextWindow ?? 128000}
                              onChange={(event) => updateLlmProfile(profile.id, { contextWindow: parseNumericInput(event.target.value, profile.contextWindow ?? 128000) })}
                            />
                          </label>
                        {profile.preset === 'ollama' && (
                          <label>{tr("Temperature")}<input
                              type="number"
                              min={0}
                              max={2}
                              step={0.05}
                              value={profile.temperature ?? 0.1}
                              onChange={(event) => updateLlmProfile(profile.id, { temperature: parseNumericInput(event.target.value, profile.temperature ?? 0.1) })}
                            />
                          </label>
                        )}
                      </div>

                      <section className="model-guidance" aria-label={tr('Model task guidance')}>
                        <div className="model-guidance-heading">
                          <div>
                            <span>{tr('Best model type for this task')}</span>
                            <strong>{tr(guidance.title)}</strong>
                          </div>
                          <p>{tr(guidance.summary)}</p>
                        </div>
                        <dl>
                          <div>
                            <dt>{tr('Recommended for')}</dt>
                            <dd>{tr(guidance.recommendedFor)}</dd>
                          </div>
                          <div>
                            <dt>{tr('Important tradeoff')}</dt>
                            <dd>{tr(guidance.tradeoff)}</dd>
                          </div>
                        </dl>
                        <details>
                          <summary>{tr('Explain the technical model name')}</summary>
                          <ul>
                            {guidance.nameExplanations.map((explanation) => (
                              <li key={`${explanation.text}-${JSON.stringify(explanation.values)}`}>
                                {tr(explanation.text, explanation.values)}
                              </li>
                            ))}
                          </ul>
                        </details>
                      </section>

                      <div className="actions llm-profile-actions">
                        <button
                          type="button"
                          className="btn-sm"
                          onClick={() => void handleHealthCheck(profile)}
                          disabled={health?.loading}
                        >
                          {health?.loading ? tr('Testing...') : tr('Health check')}
                        </button>
                        <button
                          type="button"
                          className="btn-sm"
                          onClick={() => void handleLoadModels(profile)}
                          disabled={modelState?.loading}
                        >
                          {modelState?.loading ? tr('Loading models...') : tr('Load models')}
                        </button>
                        {!isDefault && (
                          <button type="button" className="btn-sm" onClick={() => setDefaultLlmProfile(provider, profile.id)}>{tr("Set as default")}</button>
                        )}
                      </div>

                      {health?.message && (
                        <p style={{ marginTop: 8, color: health.reachable ? 'var(--success)' : 'var(--danger)' }}>
                          {health.message}{health.endpoint ? ` (${health.endpoint})` : ''}
                        </p>
                      )}
                      {models.length > 0 && !modelState?.error && (
                        <p className="hint-text" style={{ marginTop: 8 }}>
                          {models.length}{tr("Model(s) loaded")}{modelState?.endpoint ? ` ${tr('from')} ${modelState.endpoint}` : ''}.
                        </p>
                      )}
                      {modelState?.message && (
                        <p style={{ marginTop: 8, color: 'var(--success)' }}>{modelState.message}</p>
                      )}
                      {modelState?.error && (
                        <p className="error" style={{ marginTop: 8 }}>{modelState.error}</p>
                      )}
                    </div>
                  )
                })}
                  </div>
                )}
              </div>
            ) : null}
          </section>
        ))}
      </div>
      </div>
    </>
  )
}
