import { describe, expect, it } from 'vitest'
import {
  getChatProviderState,
  type ChatProviderContext,
} from './chatProvider'

const openAiProfile = {
  id: 'default-openai-compatible',
  name: 'OpenAI-kompatibel',
  provider: 'openai-compatible' as const,
  baseUrl: 'https://mlis.example.test/v1',
  model: '0xSero/Hy3-preview-nvfp4',
  apiKey: 'sk-test',
  timeoutMs: 600000,
  verifyTlsCertificates: true,
  contextWindow: null,
  temperature: null,
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
  it('falls back to the profile model when a stored external thread model is no longer listed', () => {
    const state = getChatProviderState(createContext(), 'openai-compatible', {
      provider: 'openai-compatible',
      profileId: openAiProfile.id,
      model: 'Hy3-preview-nvfp4',
    })

    expect(state.model).toBe('0xSero/Hy3-preview-nvfp4')
  })

  it('keeps a stored external thread model while no provider model list is loaded', () => {
    const context = createContext()
    context.llmProfileModels[openAiProfile.id] = []

    const state = getChatProviderState(context, 'openai-compatible', {
      provider: 'openai-compatible',
      profileId: openAiProfile.id,
      model: 'custom-model',
    })

    expect(state.model).toBe('custom-model')
  })

  it('uses the full profile model when a stored external thread model is only the suffix', () => {
    const context = createContext()
    context.llmProfileModels[openAiProfile.id] = []

    const state = getChatProviderState(context, 'openai-compatible', {
      provider: 'openai-compatible',
      profileId: openAiProfile.id,
      model: 'Hy3-preview-nvfp4',
    })

    expect(state.model).toBe('0xSero/Hy3-preview-nvfp4')
  })
})
