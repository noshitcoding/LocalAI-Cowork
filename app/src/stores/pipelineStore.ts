import { create } from 'zustand'
import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'
import {
  getVolatileToolGateways,
  migrateLegacyToolGatewayConfigs,
  setVolatileToolGateways,
} from '../security/legacyConfigMigration'
import { useConfigStore } from './configStore'
import type { OllamaConfig } from './configStore'

export type RpcPipeline = {
  id: string
  name: string
  description: string | null
  steps_json: string
  zero_context: boolean
  created_at: string
  updated_at: string
}

export type ToolGatewayEntry = {
  id: string
  tool_type: string
  name: string
  config_json: string
  enabled: boolean
  created_at: string
  updated_at: string
}

export type PipelineExecutionResult = {
  pipelineId: string
  status: 'completed' | 'failed' | 'running'
  stepResults: Array<{ step: number; tool: string; result: string; success: boolean }>
  error?: string
}

type BackendPipelineExecutionResult = {
  pipelineId: string
  status: string
  stepResults: Array<{ step: number; tool: string; result: string; success: boolean }>
  error?: string | null
}

type PipelineState = {
  pipelines: RpcPipeline[]
  toolGateway: ToolGatewayEntry[]
  loading: boolean
  error: string | null
  executing: string | null
  lastResult: PipelineExecutionResult | null

  loadPipelines: () => Promise<void>
  upsertPipeline: (p: {
    id: string; name: string; description?: string
    stepsJson: string; zeroContext?: boolean
  }) => Promise<void>
  deletePipeline: (id: string) => Promise<void>

  loadToolGateway: () => Promise<void>
  upsertToolGateway: (t: {
    id: string; toolType: string; name: string
    configJson: string; enabled?: boolean
  }) => Promise<boolean>
  deleteToolGateway: (id: string) => Promise<void>

  executePipeline: (id: string, ollamaUrl: string, model: string) => Promise<PipelineExecutionResult>
}

const LOCAL_PIPELINES_KEY = 'open-cowork-pipelines'

type ParsedPipelineStep = {
  tool: string
  prompt: string
}

type RawStoreValue = Record<string, unknown>

function hasString(value: unknown): value is string {
  return typeof value === 'string'
}

function isRecord(value: unknown): value is RawStoreValue {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function parseJsonArray(raw: string): unknown[] {
  try {
    const parsed = JSON.parse(raw)
    return Array.isArray(parsed) ? parsed : []
  } catch {
    return []
  }
}

function normalizeRpcPipeline(value: unknown): RpcPipeline | null {
  if (!isRecord(value)) return null

  const id = hasString(value.id) ? value.id : ''
  if (!id) return null

  return {
    id,
    name: hasString(value.name) ? value.name : '',
    description: hasString(value.description) ? value.description : null,
    steps_json: hasString(value.steps_json) ? value.steps_json : '',
    zero_context: typeof value.zero_context === 'boolean' ? value.zero_context : false,
    created_at: hasString(value.created_at) ? value.created_at : new Date().toISOString(),
    updated_at: hasString(value.updated_at) ? value.updated_at : new Date().toISOString(),
  }
}

function getLocalPipelines(): RpcPipeline[] {
  const raw = localStorage.getItem(LOCAL_PIPELINES_KEY) ?? '[]'
  return parseJsonArray(raw).map(normalizeRpcPipeline).filter((pipeline): pipeline is RpcPipeline => pipeline !== null)
}

function setLocalPipelines(pipelines: RpcPipeline[]): void {
  try {
    localStorage.setItem(LOCAL_PIPELINES_KEY, JSON.stringify(pipelines))
  } catch {
    /* noop */
  }
}

function readSafePipelineSteps(rawSteps: string): ParsedPipelineStep[] {
  const parsed = parseJsonArray(rawSteps)

  if (parsed.length === 0 && rawSteps.trim() !== '[]') {
    throw new Error('Steps muss ein gueltiges JSON-Array enthalten.')
  }

  return parsed.map((entry, index) => {
    if (!isRecord(entry)) {
      return {
        tool: 'ollama',
        prompt: `Schritt ${index + 1}: ${String(entry)}`,
      }
    }

    const tool = hasString(entry.tool) && entry.tool.trim().length > 0
      ? entry.tool.trim()
      : 'ollama'

    const prompt = hasString(entry.prompt)
      ? entry.prompt.trim()
      : hasString(entry.args)
        ? entry.args.trim()
        : JSON.stringify(entry) ?? `Schritt ${index + 1}: Unbekannte Step-Struktur`

    return { tool, prompt }
  })
}

export const usePipelineStore = create<PipelineState>()((set, get) => ({
  pipelines: [],
  toolGateway: [],
  loading: false,
  error: null,
  executing: null,
  lastResult: null,

  loadPipelines: async () => {
    set({ loading: true, error: null })
    try {
      const pipelines = await safeInvoke<RpcPipeline[]>('pipeline_list', undefined, getLocalPipelines())
      set({ pipelines, loading: false })
    } catch (e) {
      set({ error: String(e), loading: false })
    }
  },

  upsertPipeline: async (pipeline) => {
    const now = new Date().toISOString()
    try {
      await safeInvoke('pipeline_upsert', {
        id: pipeline.id,
        name: pipeline.name,
        description: pipeline.description ?? null,
        stepsJson: pipeline.stepsJson,
        zeroContext: pipeline.zeroContext ?? false,
      }, undefined)
      await get().loadPipelines()
    } catch {
      const local = getLocalPipelines()
      const full: RpcPipeline = {
        id: pipeline.id,
        name: pipeline.name,
        description: pipeline.description ?? null,
        steps_json: pipeline.stepsJson,
        zero_context: pipeline.zeroContext ?? false,
        created_at: now,
        updated_at: now,
      }

      const idx = local.findIndex((entry) => entry.id === pipeline.id)
      if (idx >= 0) {
        local[idx] = full
      } else {
        local.unshift(full)
      }
      setLocalPipelines(local)
      set({ pipelines: local })
    }
  },

  deletePipeline: async (id) => {
    try {
      await safeInvoke('pipeline_delete', { id }, undefined)
      set((state) => ({ pipelines: state.pipelines.filter((pipeline) => pipeline.id !== id) }))
    } catch {
      const local = getLocalPipelines().filter((pipeline) => pipeline.id !== id)
      setLocalPipelines(local)
      set((state) => ({ pipelines: state.pipelines.filter((pipeline) => pipeline.id !== id) }))
    }
  },

  loadToolGateway: async () => {
    set({ loading: true, error: null })
    try {
      await migrateLegacyToolGatewayConfigs()
      const toolGateway = hasTauriRuntime()
        ? await safeInvoke<ToolGatewayEntry[]>('tool_gateway_list')
        : getVolatileToolGateways()
      set({ toolGateway, loading: false })
    } catch (e) {
      set({ error: String(e), loading: false })
    }
  },

  upsertToolGateway: async (toolGateway) => {
    const now = new Date().toISOString()
    try {
      await safeInvoke('tool_gateway_upsert', {
        id: toolGateway.id,
        toolType: toolGateway.toolType,
        name: toolGateway.name,
        configJson: toolGateway.configJson,
        enabled: toolGateway.enabled ?? true,
      }, undefined)
      await get().loadToolGateway()
      return true
    } catch (error) {
      if (hasTauriRuntime()) {
        set({ error: error instanceof Error ? error.message : String(error) })
        return false
      }
      const local = [...getVolatileToolGateways()]
      const full: ToolGatewayEntry = {
        id: toolGateway.id,
        tool_type: toolGateway.toolType,
        name: toolGateway.name,
        config_json: toolGateway.configJson,
        enabled: toolGateway.enabled ?? true,
        created_at: now,
        updated_at: now,
      }

      const idx = local.findIndex((entry) => entry.id === toolGateway.id)
      if (idx >= 0) {
        local[idx] = full
      } else {
        local.unshift(full)
      }
      setVolatileToolGateways(local)
      set({ toolGateway: local })
      return true
    }
  },

  deleteToolGateway: async (id) => {
    try {
      await safeInvoke('tool_gateway_delete', { id }, undefined)
      set((state) => ({ toolGateway: state.toolGateway.filter((gateway) => gateway.id !== id) }))
    } catch (error) {
      if (hasTauriRuntime()) {
        set({ error: error instanceof Error ? error.message : String(error) })
        return
      }
      const local = getVolatileToolGateways().filter((gateway) => gateway.id !== id)
      setVolatileToolGateways(local)
      set((state) => ({ toolGateway: state.toolGateway.filter((gateway) => gateway.id !== id) }))
    }
  },

  executePipeline: async (id, ollamaUrl, model) => {
    const executeLocally = async (): Promise<PipelineExecutionResult> => {
      const pipeline = get().pipelines.find((entry) => entry.id === id)
      if (!pipeline) {
        const missingPipelineResult: PipelineExecutionResult = {
          pipelineId: id,
          status: 'failed',
          stepResults: [],
          error: 'Pipeline not found.',
        }
        set({ lastResult: missingPipelineResult })
        return missingPipelineResult
      }

      set({ executing: id, error: null })

      let steps: ParsedPipelineStep[]
      try {
        steps = readSafePipelineSteps(pipeline.steps_json)
      } catch (error) {
        const parseResult: PipelineExecutionResult = {
          pipelineId: id,
          status: 'failed',
          stepResults: [],
          error: `Ungueltiges Steps-JSON: ${(error as Error).message}`,
        }
        set({ executing: null, lastResult: parseResult })
        return parseResult
      }

      const stepResults: PipelineExecutionResult['stepResults'] = []

      for (let i = 0; i < steps.length; i++) {
        const step = steps[i]
        const toolName = step.tool

        try {
          const previousContext = stepResults
            .map((entry) => `[${entry.tool}]: ${entry.result}`)
            .join('\n')
          const fullPrompt = previousContext
            ? `Context bisheriger Schritte:\n${previousContext}\n\nAktuelle Task:\n${step.prompt}`
            : step.prompt

          const response = await fetch(`${ollamaUrl}/api/generate`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ model, prompt: fullPrompt, stream: false }),
          })

          if (!response.ok) {
            stepResults.push({ step: i + 1, tool: toolName, result: `HTTP ${response.status}`, success: false })
            continue
          }

          const data = await response.json() as { response?: string }
          stepResults.push({
            step: i + 1,
            tool: toolName,
            result: data.response ?? '',
            success: true,
          })
        } catch (error) {
          stepResults.push({ step: i + 1, tool: toolName, result: String(error), success: false })
        }
      }

      const hasFailure = stepResults.some((result) => !result.success)
      const executionResult: PipelineExecutionResult = {
        pipelineId: id,
        status: hasFailure ? 'failed' : 'completed',
        stepResults,
      }

      set({ executing: null, lastResult: executionResult })
      return executionResult
    }

    set({ executing: id, error: null })

    try {
      let config: OllamaConfig = {
        baseUrl: ollamaUrl,
        model,
        timeoutMs: 200_000,
        contextWindow: 128_000,
        temperature: 0.1,
      }
      if (typeof useConfigStore.getState === 'function') {
        config = useConfigStore.getState().ollama
      }

      const response = await safeInvoke<BackendPipelineExecutionResult>('pipeline_execute', {
        request: {
          id,
          config,
        },
      })

      const result: PipelineExecutionResult = {
        pipelineId: response.pipelineId,
        status: response.status === 'completed'
          ? 'completed'
          : response.status === 'running'
            ? 'running'
            : 'failed',
        stepResults: response.stepResults,
        error: response.error ?? undefined,
      }
      set({ executing: null, lastResult: result })
      return result
    } catch {
      return executeLocally()
    }
  },
}))
