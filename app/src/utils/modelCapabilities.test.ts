import { beforeEach, describe, expect, it } from 'vitest'
import { detectProviderModelCapabilities, markModelVisionUnsupported } from './modelCapabilities'
import type { ChatProviderState } from './chatProvider'

function createProviderState(patch: Partial<ChatProviderState>): ChatProviderState {
  return {
    provider: 'openrouter',
    label: 'OpenRouter',
    endpoint: 'https://openrouter.ai/api/v1',
    model: 'openai/gpt-4o-mini',
    apiKey: '',
    timeoutMs: 600000,
    verifyTlsCertificates: true,
    contextWindow: 128000,
    selectableModels: [],
    ...patch,
  }
}

describe('detectProviderModelCapabilities', () => {
  beforeEach(() => {
    window.localStorage.clear()
  })

  it('detects OpenAI-compatible vision-capable model names', () => {
    const capabilities = detectProviderModelCapabilities(createProviderState({
      provider: 'openai-compatible',
      label: 'OpenAI-compatible',
      model: 'gpt-4o-mini',
    }))

    expect(capabilities).toMatchObject({
      supportsTools: true,
      supportsVision: true,
    })
  })

  it('keeps text-only Ollama models without vision support', () => {
    const capabilities = detectProviderModelCapabilities(createProviderState({
      provider: 'ollama',
      label: 'Ollama',
      endpoint: 'http://localhost:11434',
      model: 'llama3.1:8b',
    }))

    expect(capabilities).toMatchObject({
      supportsTools: true,
      supportsVision: false,
      source: 'ollama',
    })
  })

  it('persists known provider image-input failures', () => {
    markModelVisionUnsupported('openrouter', 'openai/gpt-4o-mini')

    const capabilities = detectProviderModelCapabilities(createProviderState({
      provider: 'openrouter',
      model: 'openai/gpt-4o-mini',
    }))

    expect(capabilities).toMatchObject({
      supportsVision: false,
      source: 'persisted-error',
    })
  })
})
