import { describe, expect, it } from 'vitest'
import {
  getChatProviderState,
  type ChatProviderContext,
} from './chatProvider'

const openAiProfile = {
  id: 'default-openai-compatible',
  name: 'OpenAI-compatible',
  provider: 'openai-compatible' as const,
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

  it('lists configured models from every profile for the selected external provider', () => {
    const context = createContext()
    context.llmProfiles = [openAiProfile, secondOpenAiProfile]
    context.llmProfileModels[secondOpenAiProfile.id] = ['custom/loaded-model']

    const state = getChatProviderState(context, 'openai-compatible')

    expect(state.selectableModels).toEqual([
      '0xSero/Hy3-preview-nvfp4',
      'custom/loaded-model',
      'custom/manual-model',
    ])
  })

  it('keeps a selected model from another profile when it is present in the dropdown list', () => {
    const context = createContext()
    context.llmProfiles = [openAiProfile, secondOpenAiProfile]
    context.llmProfileModels[secondOpenAiProfile.id] = ['google/gemma-4-31B-it']

    const state = getChatProviderState(context, 'openai-compatible', {
      provider: 'openai-compatible',
      profileId: openAiProfile.id,
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
        baseUrl: 'http://localhost:11434',
        model: 'qwen2.5:14b',
        apiKey: '',
        timeoutMs: 600000,
        verifyTlsCertificates: true,
        contextWindow: 128000,
        temperature: 0.1,
      },
    ]

    const state = getChatProviderState(context, 'ollama')

    expect(state.selectableModels).toEqual(['llama3.1:8b', 'qwen2.5:14b'])
  })
})
