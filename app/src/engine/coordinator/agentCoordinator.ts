// ── Multi-Agent Coordinator (ported from Claude Code) ───────────────────────
// Mirrors: claude-code-main/src/coordinator/ + agent system
// Orchestrates sub-agents — each agent is a QueryEngine instance with its own config

import type { Message, AgentDefinition } from '../types'
import { extractTextContent, generateUUID } from '../types'
import { QueryEngine, type EngineConfig, type EngineEvent } from '../core/queryEngine'
import { safeInvoke, safeInvokeVoid } from '../../utils/safeInvoke'

function stringifyRunPayload(value: unknown, maxLength = 4000): string {
  try {
    const text = typeof value === 'string' ? value : JSON.stringify(value)
    return text.length > maxLength ? `${text.slice(0, maxLength)}...` : text
  } catch {
    return String(value).slice(0, maxLength)
  }
}

function appendAgentRunEvent(
  runId: string,
  eventType: string,
  summary: string,
  payload: unknown,
  redactionLevel = 'metadata',
): void {
  void safeInvokeVoid('engine_run_event_append', {
    request: {
      runId,
      eventType,
      summary,
      payloadJson: stringifyRunPayload(payload),
      redactionLevel,
    },
  })
}

// ── Agent Instance ─────────────────────────────────────────────────────────

export type AgentInstance = {
  id: string
  definition: AgentDefinition
  engine: QueryEngine
  messages: Message[]
  status: 'idle' | 'running' | 'completed' | 'failed'
  startedAt: number
  completedAt?: number
  result?: string
  error?: string
  turns: number
  costUsd: number
  childRunId?: string
  sandboxId?: string
}

// ── Coordinator ────────────────────────────────────────────────────────────

export class AgentCoordinator {
  private agents: Map<string, AgentInstance> = new Map()
  private baseConfig: EngineConfig

  constructor(baseConfig: EngineConfig) {
    this.baseConfig = baseConfig
  }

  /**
   * Spawn a sub-agent with a specific task
   */
  async *spawnAgent(
    definition: AgentDefinition,
    task: string,
    parentMessages?: Message[],
  ): AsyncGenerator<EngineEvent & { agentId: string }> {
    const agentId = generateUUID()
    const childRunId = generateUUID()
    const sandboxId = generateUUID()

    await safeInvokeVoid('engine_run_create', {
      request: {
        id: childRunId,
        parentRunId: this.baseConfig.runId,
        sessionId: this.baseConfig.sessionId,
        title: definition.name,
        inputSummary: task.slice(0, 1000),
        status: 'running',
        phase: 'subagent_start',
        cwd: this.baseConfig.cwd,
        model: this.baseConfig.ollama?.model ?? this.baseConfig.anthropic?.model,
        provider: this.baseConfig.backend,
        toolsetPolicyId: this.baseConfig.toolsetPolicyId,
        metadataJson: JSON.stringify({
          agentId,
          agentName: definition.name,
          agentType: definition.type,
        }),
      },
    })

    const contextMessages: Message[] = parentMessages
      ? parentMessages.slice(-5)
      : []

    let sandboxWorkspace = this.baseConfig.cwd
    try {
      const sandbox = await safeInvoke<{ id: string; workspaceRoot: string }>('worker_sandbox_create', {
        request: {
          id: sandboxId,
          runId: childRunId,
          parentRunId: this.baseConfig.runId,
          sourceCwd: this.baseConfig.cwd,
          mode: 'workspace_copy',
          allowFileRead: true,
          allowFileWrite: true,
          allowShellExecution: true,
          allowWebFetch: false,
          allowWebSearch: false,
          allowMcp: false,
          metadataJson: JSON.stringify({
            agentId,
            agentName: definition.name,
          }),
        },
      })
      sandboxWorkspace = sandbox.workspaceRoot
      void safeInvokeVoid('engine_run_update', {
        request: {
          id: childRunId,
          phase: 'sandbox_ready',
          metadataJson: JSON.stringify({
            agentId,
            sandboxId,
            sandboxWorkspace,
            isolated: true,
          }),
        },
      })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      void safeInvokeVoid('engine_run_update', {
        request: {
          id: childRunId,
          status: 'failed',
          phase: 'sandbox_error',
          error: message,
        },
      })
      yield {
        type: 'error',
        error: `Sandbox for agent "${definition.name}" could not be created: ${message}`,
        agentId,
      }
      return
    }

    const agentConfig: EngineConfig = {
      ...this.baseConfig,
      cwd: sandboxWorkspace,
      systemPrompt: definition.systemPrompt ?? this.baseConfig.systemPrompt,
      maxTurns: definition.maxTurns ?? 10,
      maxBudgetUsd: definition.budget?.totalUsd ?? this.baseConfig.maxBudgetUsd,
      runId: childRunId,
      sessionId: this.baseConfig.sessionId,
      sandboxId,
    }

    if (definition.tools && definition.tools.length > 0) {
      const allowedToolNames = new Set(definition.tools)
      agentConfig.customTools = (this.baseConfig.customTools ?? [])
        .filter(t => allowedToolNames.has(t.name))
    }

    const engine = new QueryEngine(agentConfig)
    const instance: AgentInstance = {
      id: agentId,
      definition,
      engine,
      messages: [],
      status: 'running',
      startedAt: Date.now(),
      turns: 0,
      costUsd: 0,
      childRunId,
      sandboxId,
    }

    this.agents.set(agentId, instance)

    try {
      const query = engine.query(contextMessages, task)

      for await (const event of query) {
        instance.turns = engine.getAppState().turnCount
        instance.costUsd = engine.getAppState().totalCostUsd

        // Forward events with agent ID
        yield { ...event, agentId }

        if (event.type === 'tool_use_start') {
          void safeInvokeVoid('engine_run_update', {
            request: {
              id: childRunId,
              phase: `tool:${event.toolName}`,
              metadataJson: JSON.stringify({ toolName: event.toolName, input: event.input }),
            },
          })
          appendAgentRunEvent(
            childRunId,
            'tool_start',
            `Tool started: ${event.toolName}`,
            {
              agentId,
              toolUseId: event.toolUseId,
              toolName: event.toolName,
              input: event.input,
            },
          )
        }

        if (event.type === 'tool_use_complete') {
          appendAgentRunEvent(
            childRunId,
            'tool_result',
            `Tool completed: ${event.toolName}`,
            {
              agentId,
              toolUseId: event.toolUseId,
              toolName: event.toolName,
              result: event.result,
            },
          )
        }

        if (event.type === 'approval_required') {
          appendAgentRunEvent(
            childRunId,
            'approval_requested',
            'Approval requested',
            {
              agentId,
              request: event.request,
            },
          )
        }

        if (event.type === 'done') {
          instance.messages = event.messages
          instance.status = 'completed'
          instance.completedAt = Date.now()
          // Extract final text as result
          const lastAssistant = event.messages
            .filter(m => m.type === 'assistant')
            .pop()
          if (lastAssistant && lastAssistant.type === 'assistant') {
            instance.result = lastAssistant.content
              .filter((b): b is { type: 'text'; text: string } => b.type === 'text')
              .map(b => b.text)
              .join('\n')
          }
          void safeInvokeVoid('engine_run_update', {
            request: {
              id: childRunId,
              status: 'completed',
              phase: 'completed',
              resultSummary: instance.result ?? '',
              checkpointJson: JSON.stringify({
                turnCount: instance.turns,
                totalCostUsd: instance.costUsd,
                messageCount: event.messages.length,
              }),
            },
            })
          appendAgentRunEvent(
            childRunId,
            'run_completed',
            `Agent completed: ${definition.name}`,
            {
              agentId,
              result: instance.result ?? '',
              turns: instance.turns,
              costUsd: instance.costUsd,
            },
          )
            void safeInvokeVoid('worker_sandbox_update', {
            request: {
              id: sandboxId,
              status: 'completed',
              metadataJson: JSON.stringify({
                agentId,
                completedAt: new Date().toISOString(),
              }),
            },
          })
        }

        if (event.type === 'error') {
          instance.status = 'failed'
          instance.error = event.error
          instance.completedAt = Date.now()
          void safeInvokeVoid('engine_run_update', {
            request: {
              id: childRunId,
              status: 'failed',
              phase: 'error',
              error: event.error,
            },
          })
          appendAgentRunEvent(
            childRunId,
            'error',
            event.error.slice(0, 240),
            {
              agentId,
              error: event.error,
            },
          )
          void safeInvokeVoid('worker_sandbox_update', {
            request: {
              id: sandboxId,
              status: 'failed',
              metadataJson: JSON.stringify({
                agentId,
                error: event.error,
              }),
            },
          })
        }

        if (event.type === 'assistant_message') {
          void safeInvokeVoid('engine_run_checkpoint_add', {
            request: {
              runId: childRunId,
              label: `subagent-turn-${Date.now()}`,
              snapshotJson: JSON.stringify({
                turnCount: instance.turns,
                text: extractTextContent(event.message).slice(0, 4000),
              }),
            },
          })
        }
      }
    } catch (err) {
      instance.status = 'failed'
      instance.error = err instanceof Error ? err.message : String(err)
      instance.completedAt = Date.now()
      void safeInvokeVoid('engine_run_update', {
        request: {
          id: childRunId,
          status: 'failed',
          phase: 'error',
          error: instance.error,
        },
      })
      void safeInvokeVoid('worker_sandbox_update', {
        request: {
          id: sandboxId,
          status: 'failed',
          metadataJson: JSON.stringify({
            agentId,
            error: instance.error,
          }),
        },
      })
      yield {
        type: 'error',
        error: `Agent "${definition.name}" failed: ${instance.error}`,
        agentId,
      }
    }
  }

  /**
   * Abort a running agent
   */
  abortAgent(agentId: string): void {
    const instance = this.agents.get(agentId)
    if (instance && instance.status === 'running') {
      instance.engine.abort()
      instance.status = 'failed'
      instance.error = 'Abgebrochen'
      instance.completedAt = Date.now()
      if (instance.childRunId) {
        void safeInvokeVoid('engine_run_update', {
          request: {
            id: instance.childRunId,
            status: 'canceled',
            phase: 'canceled',
            error: 'Abgebrochen',
          },
        })
      }
      if (instance.sandboxId) {
        void safeInvokeVoid('worker_sandbox_update', {
          request: {
            id: instance.sandboxId,
            status: 'canceled',
            metadataJson: JSON.stringify({ error: 'Abgebrochen' }),
          },
        })
      }
    }
  }

  /**
   * Get all agent instances
   */
  getAgents(): AgentInstance[] {
    return Array.from(this.agents.values())
  }

  /**
   * Get a specific agent instance
   */
  getAgent(agentId: string): AgentInstance | undefined {
    return this.agents.get(agentId)
  }

  /**
   * Get running agents count
   */
  getRunningCount(): number {
    return Array.from(this.agents.values()).filter(a => a.status === 'running').length
  }

  /**
   * Abort all running agents
   */
  abortAll(): void {
    for (const [id, instance] of this.agents) {
      if (instance.status === 'running') {
        this.abortAgent(id)
      }
    }
  }

  /**
   * Clear completed/failed agents
   */
  clearCompleted(): void {
    for (const [id, instance] of this.agents) {
      if (instance.status === 'completed' || instance.status === 'failed') {
        this.agents.delete(id)
      }
    }
  }
}

// ── Default Agent Definitions ──────────────────────────────────────────────

export const DEFAULT_AGENTS: AgentDefinition[] = [
  {
    id: 'coder',
    name: 'Coder',
    description: 'Specialized in code implementation and debugging.',
    type: 'coding',
    tools: ['Read', 'Write', 'Edit', 'Bash', 'Glob', 'Grep'],
    maxTurns: 15,
  },
  {
    id: 'researcher',
    name: 'Researcher',
    description: 'Specialized in research and analysis.',
    type: 'research',
    tools: ['Read', 'Glob', 'Grep', 'WebFetch', 'WebSearch', 'MemoryRead', 'SessionSearch'],
    maxTurns: 10,
  },
  {
    id: 'reviewer',
    name: 'Reviewer',
    description: 'Specialized in code review and quality assurance.',
    type: 'review',
    tools: ['Read', 'Glob', 'Grep', 'Bash'],
    maxTurns: 8,
  },
  {
    id: 'planner',
    name: 'Planner',
    description: 'Specialized in project planning and task decomposition.',
    type: 'planning',
    tools: ['Read', 'Glob', 'TaskCreate', 'TaskList', 'MemoryRead', 'MemoryWrite', 'SessionSearch'],
    maxTurns: 5,
  },
]
