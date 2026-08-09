import { detectModelCapabilities } from '../engine/api/ollamaClient'
import type { ChatProviderState } from './chatProvider'

export type ModelCapabilities = {
  supportsTools: boolean
  supportsThinking: boolean
  supportsVision: boolean
  contextLength: number
  family: string
  source: 'ollama' | 'provider-heuristic' | 'persisted-error'
}

const VISION_OVERRIDE_STORAGE_KEY = 'open-cowork:model-vision-overrides'

const VISION_MODEL_PATTERNS = [
  /(?:^|[/:_-])gpt-4(?:\.1|o)(?:$|[/:_.-])/i,
  /(?:^|[/:_-])gpt-5(?:$|[/:_.-])/i,
  /(?:^|[/:_-])o[34](?:$|[/:_.-])/i,
  /(?:^|[/:_-])claude-3(?:$|[/:_.-])/i,
  /(?:^|[/:_-])claude-(?:sonnet|opus|haiku)-4(?:$|[/:_.-])/i,
  /gemini/i,
  /(?:^|[/:_-])pixtral(?:$|[/:_.-])/i,
  /llava|bakllava|moondream|mllama/i,
  /(?:qwen|qwq).*(?:vl|vision)/i,
  /(?:minicpm|minicpm-v|internvl|deepseek-vl)/i,
  /(?:^|[/:_-])gemma-3(?:$|[/:_.-])/i,
  /vision|multimodal|image/i,
]

const REASONING_MODEL_PATTERNS = [
  /(?:^|[/:_-])o[34](?:$|[/:_.-])/i,
  /(?:^|[/:_-])gpt-5(?:$|[/:_.-])/i,
  /deepseek-r1|reasoning|thinking/i,
  /(?:^|[/:_-])qwen3(?:$|[/:_.-])/i,
]

function normalizeModelKey(provider: string, model: string): string {
  return `${provider.trim().toLowerCase()}:${model.trim().toLowerCase()}`
}

function readVisionOverrides(): Record<string, boolean> {
  if (typeof window === 'undefined') {
    return {}
  }

  try {
    const storage = window.localStorage
    const raw = storage.getItem(VISION_OVERRIDE_STORAGE_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw)
    return parsed && typeof parsed === 'object' ? parsed as Record<string, boolean> : {}
  } catch {
    return {}
  }
}

function writeVisionOverrides(overrides: Record<string, boolean>): void {
  if (typeof window === 'undefined') {
    return
  }

  try {
    window.localStorage.setItem(VISION_OVERRIDE_STORAGE_KEY, JSON.stringify(overrides))
  } catch {
    // Local persistence is at optimization; failing closed would hide valid tools.
  }
}

function hasPersistedVisionBlock(provider: string, model: string): boolean {
  return readVisionOverrides()[normalizeModelKey(provider, model)] === false
}

function inferFamily(model: string): string {
  const normalized = model.trim().toLowerCase()
  const suffix = normalized.split('/').filter(Boolean).at(-1) ?? normalized
  return suffix.split(/[:@._-]/).filter(Boolean)[0] || 'unknown'
}

export function markModelVisionUnsupported(provider: string, model: string): void {
  const normalizedModel = model.trim()
  if (!normalizedModel) return
  const overrides = readVisionOverrides()
  overrides[normalizeModelKey(provider, normalizedModel)] = false
  writeVisionOverrides(overrides)
}

export function detectProviderModelCapabilities(providerState: ChatProviderState): ModelCapabilities {
  if (providerState.preset === 'ollama') {
    const capabilities = detectModelCapabilities(providerState.model)
    return {
      ...capabilities,
      supportsVision: 'supportsVision' in capabilities ? Boolean(capabilities.supportsVision) : false,
      source: 'ollama',
    }
  }

  const capabilityProvider = providerState.preset ?? providerState.provider
  if (hasPersistedVisionBlock(capabilityProvider, providerState.model)) {
    return {
      supportsTools: true,
      supportsThinking: providerState.provider === 'codex' || providerState.preset === 'openrouter',
      supportsVision: false,
      contextLength: 0,
      family: inferFamily(providerState.model),
      source: 'persisted-error',
    }
  }

  const supportsVision = VISION_MODEL_PATTERNS.some((pattern) => pattern.test(providerState.model))
  const supportsThinking = providerState.provider === 'codex' || providerState.preset === 'openrouter'
    || REASONING_MODEL_PATTERNS.some((pattern) => pattern.test(providerState.model))

  return {
    supportsTools: true,
    supportsThinking,
    supportsVision,
    contextLength: 0,
    family: inferFamily(providerState.model),
    source: 'provider-heuristic',
  }
}
