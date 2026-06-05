import { useMemo, useState } from 'react'
import { checkOllamaConnection, listOllamaModels } from '../engine/api/ollamaClient'
import { useConfigStore, type LlmProfile, type LlmProviderKind } from '../stores/configStore'
import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'
import { tr } from '../i18n'

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

const PROVIDER_ORDER: LlmProviderKind[] = ['ollama', 'openai-compatible', 'openrouter']

const PROVIDER_LABELS: Record<LlmProviderKind, string> = {
  ollama: 'Ollama',
  'openai-compatible': 'OpenAI-compatible',
  openrouter: 'OpenRouter',
}

const PROVIDER_PLACEHOLDERS: Record<LlmProviderKind, { baseUrl: string; model: string }> = {
  ollama: {
    baseUrl: 'http://localhost:11434',
    model: 'llama3.1:8b',
  },
  'openai-compatible': {
    baseUrl: 'https://api.openai.com/v1',
    model: 'gpt-4.1-mini',
  },
  openrouter: {
    baseUrl: 'https://openrouter.ai/api/v1',
    model: 'openai/gpt-4o-mini',
  },
}

function parseNumericInput(raw: string, fallback: number): number {
  const parsed = Number(raw.replace(',', '.').trim())
  return Number.isFinite(parsed) ? parsed : fallback
}

function supportsApiKey(provider: LlmProviderKind): boolean {
  return provider !== 'ollama'
}

function modelSuffix(model: string): string {
  return model.trim().split('/').filter(Boolean).at(-1) ?? model.trim()
}

function resolveLoadedModel(currentModel: string, models: string[]): string {
  const current = currentModel.trim()
  const normalizedModels = models.map((model) => model.trim()).filter(Boolean)
  if (normalizedModels.length === 0) return current
  if (!current) return normalizedModels.length === 1 ? normalizedModels[0] : ''

  const exact = normalizedModels.find((model) => model === current)
  if (exact) return exact

  const lowerCurrent = current.toLowerCase()
  const caseInsensitive = normalizedModels.find((model) => model.toLowerCase() === lowerCurrent)
  if (caseInsensitive) return caseInsensitive

  const suffixMatch = normalizedModels.find((model) => modelSuffix(model).toLowerCase() === lowerCurrent)
  if (suffixMatch) return suffixMatch

  return normalizedModels.length === 1 ? normalizedModels[0] : current
}

export default function LlmProfilesPanel() {
  const {
    llmProfiles,
    defaultLlmProfileIds,
    llmProfileModels,
    addLlmProfile,
    updateLlmProfile,
    deleteLlmProfile,
    setDefaultLlmProfile,
    setLlmProfileModels,
  } = useConfigStore()

  const [healthChecks, setHealthChecks] = useState<Record<string, ProfileHealthState>>({})
  const [modelStates, setModelStates] = useState<Record<string, ProfileModelsState>>({})

  const profilesByProvider = useMemo(
    () => PROVIDER_ORDER.map((provider) => ({
      provider,
      profiles: llmProfiles.filter((profile) => profile.provider === provider),
    })),
    [llmProfiles],
  )

  const handleAddProfile = (provider: LlmProviderKind) => {
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
        checkOllamaConnection(profile.baseUrl),
        listOllamaModels(profile.baseUrl).catch(() => []),
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
          providerKind: profile.provider,
          baseUrl: profile.baseUrl,
          apiKey: profile.apiKey,
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
          endpoint: profile.baseUrl,
          message,
        },
      }))
    }
  }

  const handleHealthCheck = async (profile: LlmProfile) => {
    if (profile.provider === 'ollama') {
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
      const models = await listOllamaModels(profile.baseUrl)
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
          providerKind: profile.provider,
          baseUrl: profile.baseUrl,
          apiKey: profile.apiKey,
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
    if (profile.provider === 'ollama') {
      await handleOllamaModelsLoad(profile)
      return
    }

    await handleExternalModelsLoad(profile)
  }

  return (
    <div className="panel">
      <div className="panel-heading-row">
        <h2>{tr("🧩 LLM-Profile")}</h2>
        <div className="actions" style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <button type="button" className="btn-sm" onClick={() => handleAddProfile('ollama')}>{tr("+ Ollama")}</button>
          <button type="button" className="btn-sm" onClick={() => handleAddProfile('openai-compatible')}>{tr("+ OpenAI-compatible")}</button>
          <button type="button" className="btn-sm" onClick={() => handleAddProfile('openrouter')}>{tr("+ OpenRouter")}</button>
        </div>
      </div>
      <p className="hint-text">{tr("Maintain multiple endpoints in parallel and set one global default profile per provider for dropdowns and fallbacks.")}</p>

      <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
        {profilesByProvider.map(({ provider, profiles }) => (
          <div key={provider} className="card">
            <div className="panel-heading-row" style={{ marginBottom: 12 }}>
              <div>
                <strong>{PROVIDER_LABELS[provider]}</strong>
                <div className="hint-text">{tr("Global default profile for this provider")}</div>
              </div>
              <select
                value={defaultLlmProfileIds[provider]}
                onChange={(event) => setDefaultLlmProfile(provider, event.target.value)}
                disabled={profiles.length === 0}
              >
                {profiles.map((profile) => (
                  <option key={profile.id} value={profile.id}>{profile.name}</option>
                ))}
              </select>
            </div>

            {profiles.length === 0 ? (
              <p className="panel-empty">{tr("No profile for")}{PROVIDER_LABELS[provider]}{tr("angelegt.")}</p>
            ) : (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
                {profiles.map((profile) => {
                  const isDefault = defaultLlmProfileIds[provider] === profile.id
                  const health = healthChecks[profile.id]
                  const models = llmProfileModels[profile.id] ?? []
                  const modelState = modelStates[profile.id]
                  const canDelete = !isDefault && profiles.length > 1

                  return (
                    <div
                      key={profile.id}
                      className="card"
                      style={{ border: isDefault ? '1px solid var(--accent)' : '1px solid var(--border-color)' }}
                    >
                      <div className="panel-heading-row" style={{ marginBottom: 10 }}>
                        <div>
                          <strong>{profile.name}</strong>
                          {isDefault && <span style={{ fontSize: 11, color: 'var(--accent)', marginLeft: 8 }}>{tr("Default profile")}</span>}
                        </div>
                        <button
                          type="button"
                          className="btn-sm"
                          onClick={() => deleteLlmProfile(profile.id)}
                          disabled={!canDelete}
                          title={canDelete ? tr('Delete profile') : tr('Default profile or last profile cannot be deleted')}
                        >{tr("Delete")}</button>
                      </div>

                      <div className="grid" style={{ gridTemplateColumns: 'repeat(3, minmax(0, 1fr))' }}>
                        <label>{tr("Profile name")}<input
                            value={profile.name}
                            onChange={(event) => updateLlmProfile(profile.id, { name: event.target.value })}
                          />
                        </label>
                        <label>{tr("Endpoint")}<input
                            value={profile.baseUrl}
                            onChange={(event) => updateLlmProfile(profile.id, { baseUrl: event.target.value })}
                            placeholder={PROVIDER_PLACEHOLDERS[profile.provider].baseUrl}
                            style={{ fontFamily: 'monospace' }}
                          />
                        </label>
                        <label>{tr("Model")}{models.length > 0 ? (
                            <select value={profile.model} onChange={(event) => updateLlmProfile(profile.id, { model: event.target.value })}>
                              {models.map((model) => <option key={model} value={model}>{model}</option>)}
                              {!models.includes(profile.model) && profile.model && <option value={profile.model}>{profile.model}</option>}
                            </select>
                          ) : (
                            <input
                              value={profile.model}
                              onChange={(event) => updateLlmProfile(profile.id, { model: event.target.value })}
                              placeholder={PROVIDER_PLACEHOLDERS[profile.provider].model}
                              style={{ fontFamily: 'monospace' }}
                            />
                          )}
                        </label>
                        {supportsApiKey(profile.provider) && (
                          <label>{tr("API Key")}<input
                              type="password"
                              value={profile.apiKey}
                              onChange={(event) => updateLlmProfile(profile.id, { apiKey: event.target.value })}
                              placeholder={tr("sk?...")}
                              style={{ fontFamily: 'monospace' }}
                            />
                          </label>
                        )}
                        {supportsApiKey(profile.provider) && (
                          <label className="toggle-row" style={{ alignSelf: 'end' }}>
                            <span>{tr("Check SSL verification / TLS certificates")}<span className="hint-text">{tr("Turn off for HTTPS with self-signed certificates.")}</span>
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
                        <label>{tr("Timeout (ms)")}<input
                            type="number"
                            min={1000}
                            max={86400000}
                            step={1000}
                            value={profile.timeoutMs}
                            onChange={(event) => updateLlmProfile(profile.id, { timeoutMs: parseNumericInput(event.target.value, profile.timeoutMs) })}
                          />
                        </label>
                        {profile.provider === 'ollama' && (
                          <label>{tr("Context Window")}<input
                              type="number"
                              min={512}
                              max={262144}
                              step={512}
                              value={profile.contextWindow ?? 128000}
                              onChange={(event) => updateLlmProfile(profile.id, { contextWindow: parseNumericInput(event.target.value, profile.contextWindow ?? 128000) })}
                            />
                          </label>
                        )}
                        {profile.provider === 'ollama' && (
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

                      <div className="actions" style={{ marginTop: 12 }}>
                        <button type="button" className="btn-sm" onClick={() => void handleHealthCheck(profile)}>
                          {health?.loading ? tr('Testing...') : tr('Health check')}
                        </button>
                        <button type="button" className="btn-sm" onClick={() => void handleLoadModels(profile)}>
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
                      {models.length > 0 && (
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
        ))}
      </div>
    </div>
  )
}
