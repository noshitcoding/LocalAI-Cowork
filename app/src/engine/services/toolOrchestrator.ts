// ── Tool Orchestrator Service (ported from Claude Code) ────────────────────
// Mirrors: claude-code-main/src/services/toolExecution/
// Enhanced tool execution with streaming progress, timeouts, and batching

import type {
  ContentBlockToolUse,
  ContentBlockToolResult,
  Tool,
  Tools,
  ToolUseContext,
  ToolResult,
  ToolProgressData,
} from '../types'
import { findToolByName } from '../types'

// ── Types ──────────────────────────────────────────────────────────────────

export type ToolExecutionResult = {
  toolUseId: string
  toolName: string
  result: ContentBlockToolResult
  summary: string
  durationMs: number
}

export type ToolExecutionEvent =
  | { type: 'tool_start'; toolUseId: string; toolName: string; input: Record<string, unknown> }
  | { type: 'tool_progress'; toolUseId: string; data: ToolProgressData }
  | { type: 'tool_complete'; toolUseId: string; toolName: string; result: string; durationMs: number }
  | { type: 'tool_error'; toolUseId: string; toolName: string; error: string }
  | { type: 'approval_needed'; toolUseId: string; toolName: string; input: Record<string, unknown> }

export type ToolOrchestratorConfig = {
  /** Maximum time for a single tool execution (ms) */
  maxToolTimeoutMs: number
  /** Maximum total time for all tools in a turn (ms) */
  maxTurnToolTimeoutMs: number
  /** Maximum number of concurrent tools */
  maxConcurrent: number
  /** Whether to generate tool use summaries */
  generateSummaries: boolean
}

export const DEFAULT_ORCHESTRATOR_CONFIG: ToolOrchestratorConfig = {
  maxToolTimeoutMs: 60000,
  maxTurnToolTimeoutMs: 300000,
  maxConcurrent: 5,
  generateSummaries: true,
}

// ── Tool Orchestrator ──────────────────────────────────────────────────────

export class ToolOrchestrator {
  private config: ToolOrchestratorConfig

  constructor(config: Partial<ToolOrchestratorConfig> = {}) {
    this.config = { ...DEFAULT_ORCHESTRATOR_CONFIG, ...config }
  }

  /**
   * Execute all tool use blocks from an assistant message.
   * Handles concurrency, permissions, timeouts, and progress.
   */
  async *executeToolBlocks(
    toolUseBlocks: ContentBlockToolUse[],
    tools: Tools,
    context: ToolUseContext,
    checkPermission: (tool: Tool, input: Record<string, unknown>, ctx: ToolUseContext) => Promise<{ allowed: boolean; reason?: string }>,
  ): AsyncGenerator<ToolExecutionEvent, ContentBlockToolResult[]> {
    const results: ContentBlockToolResult[] = []

    // Separate into concurrent-safe and sequential
    const concurrent: Array<{ block: ContentBlockToolUse; tool: Tool }> = []
    const sequential: Array<{ block: ContentBlockToolUse; tool: Tool }> = []

    for (const block of toolUseBlocks) {
      const tool = findToolByName(tools, block.name)
      if (!tool) {
        results.push({
          type: 'tool_result',
          tool_use_id: block.id,
          content: `Tool "${block.name}" not found.`,
          is_error: true,
        })
        yield { type: 'tool_error', toolUseId: block.id, toolName: block.name, error: 'Tool not found' }
        continue
      }

      // Permission check
      const permission = await checkPermission(tool, block.input, context)
      if (!permission.allowed) {
        results.push({
          type: 'tool_result',
          tool_use_id: block.id,
          content: `Tool "${tool.name}" denied: ${permission.reason}`,
          is_error: true,
        })
        yield { type: 'tool_error', toolUseId: block.id, toolName: tool.name, error: `Denied: ${permission.reason}` }
        continue
      }

      if (tool.isConcurrencySafe?.(block.input)) {
        concurrent.push({ block, tool })
      } else {
        sequential.push({ block, tool })
      }
    }

    // Execute concurrent tools in parallel (with limit)
    if (concurrent.length > 0) {
      const batches = chunk(concurrent, this.config.maxConcurrent)
      for (const batch of batches) {
        // Emit start events for all tools in batch
        for (const { block, tool } of batch) {
          yield { type: 'tool_start', toolUseId: block.id, toolName: tool.name, input: block.input }
        }
        // Run batch in parallel
        const batchResults = await Promise.allSettled(
          batch.map(({ block, tool }) => this.runWithTimeout(tool, block, context)),
        )
        // Emit results
        for (let i = 0; i < batch.length; i++) {
          const { block, tool } = batch[i]
          const settled = batchResults[i]
          if (settled.status === 'fulfilled') {
            const result = settled.value
            results.push(result.toolResult)
            if (result.error) {
              yield { type: 'tool_error', toolUseId: block.id, toolName: tool.name, error: result.error }
            } else {
              yield {
                type: 'tool_complete',
                toolUseId: block.id,
                toolName: tool.name,
                result: result.toolResult.content,
                durationMs: result.durationMs,
              }
            }
          } else {
            const error = String(settled.reason)
            results.push({ type: 'tool_result', tool_use_id: block.id, content: `Error: ${error}`, is_error: true })
            yield { type: 'tool_error', toolUseId: block.id, toolName: tool.name, error }
          }
        }
      }
    }

    // Execute sequential tools one at a time
    for (const { block, tool } of sequential) {
      yield { type: 'tool_start', toolUseId: block.id, toolName: tool.name, input: block.input }

      const result = await this.runWithTimeout(tool, block, context)
      results.push(result.toolResult)

      if (result.error) {
        yield { type: 'tool_error', toolUseId: block.id, toolName: tool.name, error: result.error }
      } else {
        yield {
          type: 'tool_complete',
          toolUseId: block.id,
          toolName: tool.name,
          result: result.toolResult.content,
          durationMs: result.durationMs,
        }
      }

      // Apply context modifier if present
      if (result.contextModifier) {
        Object.assign(context, result.contextModifier(context))
      }
    }

    return results
  }

  private async runWithTimeout(
    tool: Tool,
    block: ContentBlockToolUse,
    context: ToolUseContext,
  ): Promise<{
    toolResult: ContentBlockToolResult
    durationMs: number
    error?: string
    contextModifier?: ToolResult['contextModifier']
  }> {
    const startTime = Date.now()

    try {
      const result = await Promise.race([
        tool.call(block.input, context, (_progress) => {
          // progress callback — events are consumed by the generator
        }),
        new Promise<never>((_, reject) =>
          setTimeout(() => reject(new Error(`Tool "${tool.name}" Timeout (${this.config.maxToolTimeoutMs}ms)`)), this.config.maxToolTimeoutMs),
        ),
      ])

      const resultStr = typeof result.data === 'string' ? result.data : JSON.stringify(result.data)
      return {
        toolResult: {
          type: 'tool_result',
          tool_use_id: block.id,
          content: resultStr,
        },
        durationMs: Date.now() - startTime,
        contextModifier: result.contextModifier,
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      return {
        toolResult: {
          type: 'tool_result',
          tool_use_id: block.id,
          content: `Error: ${msg}`,
          is_error: true,
        },
        durationMs: Date.now() - startTime,
        error: msg,
      }
    }
  }
}

// ── Helpers ────────────────────────────────────────────────────────────────

function chunk<T>(arr: T[], size: number): T[][] {
  const chunks: T[][] = []
  for (let i = 0; i < arr.length; i += size) {
    chunks.push(arr.slice(i, i + size))
  }
  return chunks
}
