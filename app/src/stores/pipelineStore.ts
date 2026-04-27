import { create } from 'zustand'
import { safeInvoke } from '../utils/safeInvoke'
import type { OllamaConfig } from '../stores/configStore'

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
  }) => Promise<void>
  deleteToolGateway: (id: string) => Promise<void>

  executePipeline: (id: string, ollamaUrl: string, model: string) => Promise<PipelineExecutionResult>
}

/* ── Local fallback storage ─────────────────────────────────────────── */

const LOCAL_PIPELINES_KEY = 'open-cowork-pipelines'
const LOCAL_GATEWAY_KEY = 'open-cowork-gateway'

function getLocalPipelines(): RpcPipeline[] {
  try { return JSON.parse(localStorage.getItem(LOCAL_PIPELINES_KEY) ?? '[]') } catch { return [] }
}
function setLocalPipelines(p: RpcPipeline[]): void {
  try { localStorage.setItem(LOCAL_PIPELINES_KEY, JSON.stringify(p)) } catch { /* noop */ }
}
function getLocalGateway(): ToolGatewayEntry[] {
  try { return JSON.parse(localStorage.getItem(LOCAL_GATEWAY_KEY) ?? '[]') } catch { return [] }
}
function setLocalGateway(g: ToolGatewayEntry[]): void {
  try { localStorage.setItem(LOCAL_GATEWAY_KEY, JSON.stringify(g)) } catch { /* noop */ }
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

  upsertPipeline: async (p) => {
    const now = new Date().toISOString()
    try {
      await safeInvoke('pipeline_upsert', {
        id: p.id,
        name: p.name,
        description: p.description ?? null,
        stepsJson: p.stepsJson,
        zeroContext: p.zeroContext ?? false,
      }, undefined)
      await get().loadPipelines()
    } catch {
      // Fallback: save locally
      const local = getLocalPipelines()
      const full: RpcPipeline = {
        id: p.id, name: p.name, description: p.description ?? null,
        steps_json: p.stepsJson, zero_context: p.zeroContext ?? false,
        created_at: now, updated_at: now,
      }
      const idx = local.findIndex(x => x.id === p.id)
      if (idx >= 0) local[idx] = full; else local.unshift(full)
      setLocalPipelines(local)
      set({ pipelines: local })
    }
  },

  deletePipeline: async (id) => {
    try {
      await safeInvoke('pipeline_delete', { id }, undefined)
      set((s) => ({ pipelines: s.pipelines.filter((p) => p.id !== id) }))
    } catch {
      const local = getLocalPipelines().filter(p => p.id !== id)
      setLocalPipelines(local)
      set((s) => ({ pipelines: s.pipelines.filter((p) => p.id !== id) }))
    }
  },

  loadToolGateway: async () => {
    set({ loading: true, error: null })
    try {
      const toolGateway = await safeInvoke<ToolGatewayEntry[]>('tool_gateway_list', undefined, getLocalGateway())
      set({ toolGateway, loading: false })
    } catch (e) {
      set({ error: String(e), loading: false })
    }
  },

  upsertToolGateway: async (t) => {
    const now = new Date().toISOString()
    try {
      await safeInvoke('tool_gateway_upsert', {
        id: t.id,
        toolType: t.toolType,
        name: t.name,
        configJson: t.configJson,
        enabled: t.enabled ?? true,
      }, undefined)
      await get().loadToolGateway()
    } catch {
      const local = getLocalGateway()
      const full: ToolGatewayEntry = {
        id: t.id, tool_type: t.toolType, name: t.name,
        config_json: t.configJson, enabled: t.enabled ?? true,
        created_at: now, updated_at: now,
      }
      const idx = local.findIndex(x => x.id === t.id)
      if (idx >= 0) local[idx] = full; else local.unshift(full)
      setLocalGateway(local)
      set({ toolGateway: local })
    }
  },

  deleteToolGateway: async (id) => {
    try {
      await safeInvoke('tool_gateway_delete', { id }, undefined)
      set((s) => ({ toolGateway: s.toolGateway.filter((t) => t.id !== id) }))
    } catch {
      const local = getLocalGateway().filter(t => t.id !== id)
      setLocalGateway(local)
      set((s) => ({ toolGateway: s.toolGateway.filter((t) => t.id !== id) }))
    }
  },

  executePipeline: async (id, ollamaUrl, model) => {
    const executeLocally = async (): Promise<PipelineExecutionResult> => {
    const pipeline = get().pipelines.find(p => p.id === id)
    if (!pipeline) {
      const result: PipelineExecutionResult = {
        pipelineId: id, status: 'failed', stepResults: [],
        error: 'Pipeline nicht gefunden.',
      }
      set({ lastResult: result })
      return result
    }

    set({ executing: id, error: null })

    let steps: Array<{ tool?: string; prompt?: string; args?: string }>
    try {
      steps = JSON.parse(pipeline.steps_json)
      if (!Array.isArray(steps)) throw new Error('Steps muss ein Array sein')
    } catch (e) {
      const result: PipelineExecutionResult = {
        pipelineId: id, status: 'failed', stepResults: [],
        error: `Ungueltige Steps-JSON: ${e}`,
      }
      set({ executing: null, lastResult: result })
      return result
    }

    const stepResults: PipelineExecutionResult['stepResults'] = []

    for (let i = 0; i < steps.length; i++) {
      const step = steps[i]
      const toolName = step.tool ?? 'ollama'
      const prompt = step.prompt ?? step.args ?? `Schritt ${i + 1}: ${JSON.stringify(step)}`

      try {
        // Execute via Ollama
        const previousContext = stepResults.map(r => `[${r.tool}]: ${r.result}`).join('\n')
        const fullPrompt = previousContext
          ? `Kontext bisheriger Schritte:\n${previousContext}\n\nAktuelle Aufgabe:\n${prompt}`
          : prompt

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
        stepResults.push({ step: i + 1, tool: toolName, result: data.response ?? '', success: true })
      } catch (e) {
        stepResults.push({ step: i + 1, tool: toolName, result: String(e), success: false })
      }
    }

    const hasFailure = stepResults.some(r => !r.success)
    const result: PipelineExecutionResult = {
      pipelineId: id,
      status: hasFailure ? 'failed' : 'completed',
      stepResults,
    }

    set({ executing: null, lastResult: result })
    return result
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
      try {
        const { useConfigStore } = await import('./configStore')
        config = useConfigStore.getState().ollama
      } catch {
        // keep passed values as fallback
      }

      const response = await safeInvoke<BackendPipelineExecutionResult>('pipeline_execute', {
        request: {
          id,
          config,
        },
      })

      const result: PipelineExecutionResult = {
        pipelineId: response.pipelineId,
        status: response.status === 'completed' ? 'completed' : response.status === 'running' ? 'running' : 'failed',
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
