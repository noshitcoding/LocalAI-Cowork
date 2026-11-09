import { describe, expect, it } from 'vitest'
import { MODEL_CATALOG_MAX_AGE_MS, providerCatalogTarget } from './providerModelCatalog'
import {
  getChatProviderState,
  assertChatProviderModelAvailable,
  type ChatProviderContext,
} from './chatProvider'

const openAiProfile = {
  id: 'default-openai-compatible',
  name: 'OpenAI-compatible',
  provider: 'openai-compatible' as const,
  preset: 'custom' as const,
  authMode: 'bearer' as const,
  baseUrl: 'https://mlis.example.test/v1',
  model: '0xSero/Hy3-preview-nvfp4',
  apiKey: 'sk-test',
  timeoutMs: 600000,
  verifyTlsCertificates: true,
  contextWindow: null,
  temperature: null,
}

const secondOpenAiProfile = {
  ...openAiProfile,
  id: 'secondary-openai-compatible',
  name: 'OpenAI-compatible 2',
  model: 'custom/manual-model',
}

function createContext(): ChatProviderContext {
  return {
    ollama: {
      baseUrl: 'http://localhost:11434',
      model: 'llama3.1:8b',
      timeoutMs: 600000,
      contextWindow: 128000,
      temperature: 0.1,
    },
    availableModels: [],
    llmProfiles: [openAiProfile],
    defaultLlmProfileIds: {
      api: openAiProfile.id,
      ollama: 'default-ollama',
      'openai-compatible': openAiProfile.id,
      openrouter: 'default-openrouter',
    },
    llmProfileModels: {
      [openAiProfile.id]: ['0xSero/Hy3-preview-nvfp4'],
    },
  }
}

describe('getChatProviderState', () => {
  it('preserves a stale OpenRouter choice and blocks execution instead of switching models', () => {
    const context = createContext()
    context.llmProfiles = [{ ...openAiProfile, preset: 'openrouter', model: 'retired/model:free' }]
    context.llmProfileModels[openAiProfile.id] = ['openrouter/free']
    context.llmProfileModelCatalogs = { [openAiProfile.id]: {
      target: providerCatalogTarget(context.llmProfiles[0]), checkedAt: Date.now(),
    } }
    const state = getChatProviderState(context, 'openai-compatible')
    expect(state.model).toBe('retired/model:free')
    expect(state.modelAvailable).toBe(false)
    expect(() => assertChatProviderModelAvailable(state)).toThrow('PROVIDER_MODEL_UNAVAILABLE')
    context.llmProfiles[0].model = 'openrouter/free'
    expect(() => assertChatProviderModelAvailable(getChatProviderState(context, 'openai-compatible'))).not.toThrow()
  })
  it('does not use an unknown-age, expired, or different-endpoint catalog to reject a saved model', () => {
    const context = createContext()
    const profile = { ...openAiProfile, preset: 'openrouter' as const, model: 'saved/model' }
    context.llmProfiles = [profile]
    context.llmProfileModels[profile.id] = ['openrouter/free']
    expect(getChatProviderState(context, 'openai-compatible').modelAvailable).toBeUndefined()
    context.llmProfileModelCatalogs = { [profile.id]: {
      target: providerCatalogTarget(profile), checkedAt: Date.now() - MODEL_CATALOG_MAX_AGE_MS,
    } }
    expect(getChatProviderState(context, 'openai-compatible').modelAvailable).toBeUndefined()
    context.llmProfileModelCatalogs[profile.id] = {
      target: providerCatalogTarget({ ...profile, baseUrl: 'https://other.test' }), checkedAt: Date.now(),
    }
    const state = getChatProviderState(context, 'openai-compatible')
    expect(state.model).toBe('saved/model')
    expect(state.modelAvailable).toBeUndefined()
    expect(() => assertChatProviderModelAvailable(state)).not.toThrow()
  })
  it('falls back to the profile model when a stored external thread model is no longer listed', () => {
    const state = getChatProviderState(createContext(), 'openai-compatible', {
      backend: 'openai-compatible',
      profileId: openAiProfile.id,
      model: 'Hy3-preview-nvfp4',
    })

    expect(state.model).toBe('0xSero/Hy3-preview-nvfp4')
  })

  it('keeps a stored external thread model while no provider model list is loaded', () => {
    const context = createContext()
    context.llmProfileModels[openAiProfile.id] = []

    const state = getChatProviderState(context, 'openai-compatible', {
      backend: 'openai-compatible',
      profileId: openAiProfile.id,
      model: 'custom-model',
    })

    expect(state.model).toBe('custom-model')
  })

  it('clamps a legacy profile timeout to the local daemon limit', () => {
    const context = createContext()
    context.llmProfiles = [{ ...openAiProfile, timeoutMs: 200_000_000 }]

    const state = getChatProviderState(context, 'openai-compatible')

    expect(state.timeoutMs).toBe(86_400_000)
  })

  it('uses the full profile model when a stored external thread model is only the suffix', () => {
    const context = createContext()
    context.llmProfileModels[openAiProfile.id] = []

    const state = getChatProviderState(context, 'openai-compatible', {
      backend: 'openai-compatible',
      profileId: openAiProfile.id,
      model: 'Hy3-preview-nvfp4',
    })

    expect(state.model).toBe('0xSero/Hy3-preview-nvfp4')
  })

  it('lists only models belonging to the selected API profile', () => {
    const context = createContext()
    context.llmProfiles = [openAiProfile, secondOpenAiProfile]
    context.llmProfileModels[secondOpenAiProfile.id] = ['custom/loaded-model']

    const state = getChatProviderState(context, 'openai-compatible')

    expect(state.selectableModels).toEqual([
      '0xSero/Hy3-preview-nvfp4',
    ])
  })

  it('keeps a selected model from another profile when it is present in the dropdown list', () => {
    const context = createContext()
    context.llmProfiles = [openAiProfile, secondOpenAiProfile]
    context.llmProfileModels[secondOpenAiProfile.id] = ['google/gemma-4-31B-it']

    const state = getChatProviderState(context, 'openai-compatible', {
      backend: 'openai-compatible',
      profileId: secondOpenAiProfile.id,
      model: 'google/gemma-4-31B-it',
    })

    expect(state.model).toBe('google/gemma-4-31B-it')
    expect(state.selectableModels).toContain('google/gemma-4-31B-it')
  })

  it('lists configured Ollama profile models alongside loaded Ollama models', () => {
    const context = createContext()
    context.availableModels = ['llama3.1:8b']
    context.llmProfiles = [
      ...context.llmProfiles,
      {
        id: 'default-ollama',
        name: 'Lokales Ollama',
        provider: 'ollama',
        preset: 'ollama',
        authMode: 'none',
        baseUrl: 'http://localhost:11434',
        model: 'qwen2.5:14b',
        apiKey: '',
        timeoutMs: 600000,
        verifyTlsCertificates: true,
        contextWindow: 128000,
        temperature: 0.1,
      },
    ]

    const state = getChatProviderState(context, 'openai-compatible', {
      backend: 'openai-compatible',
      profileId: 'default-ollama',
    })

    expect(state.selectableModels).toEqual(['llama3.1:8b', 'qwen2.5:14b'])
  })
})
