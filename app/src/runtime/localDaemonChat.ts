import type { CrewLiveStatus, LiveToolCall } from '../stores/chatStore'
import { useChatStore } from '../stores/chatStore'
import { useWorkTasksStore } from '../stores/workTasksStore'
import {
  appendCrewLiveEntry,
  buildCrewLiveMessageContent,
  createCrewLiveEntry,
  type CrewExecutionLog,
} from '../engine/crew/workTaskCrewRuntime'
import type { RunEvent, RunRecord } from './contracts'
import { createLocalDaemonRuntimeClient, watchDurableLocalRun } from './localDaemonExecution'
import type { LocalDaemonRuntimeClient } from './localDaemonClient'
import { reconcileDurableLocalEntities } from './localDaemonEntities'

const activeWatchers = new Map<string, { unsubscribe: () => void; done: Promise<RunRecord> }>()

type ClientRunLink = {
  threadId: string
  assistantMessageId: string
  taskId: string | null
  crewLiveMessageId: string | null
  crewLiveTitle: string
  crewStreamId: string
}

function objectPayload(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' ? value as Record<string, unknown> : {}
}

function stringValue(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

function runLink(run: RunRecord): ClientRunLink | null {
  const input = objectPayload(run.spec.input)
  const threadId = stringValue(input.client_thread_id)
  const assistantMessageId = stringValue(input.client_assistant_message_id)
  if (!threadId || !assistantMessageId) return null
  return {
    threadId,
    assistantMessageId,
    taskId: stringValue(input.client_task_id) || null,
    crewLiveMessageId: stringValue(input.client_crew_live_message_id) || null,
    crewLiveTitle: stringValue(input.crew_live_title) || 'Crew execution',
    crewStreamId: stringValue(input.crew_stream_id) || `crew-${run.spec.id}`,
  }
}

function ensureRunMessages(run: RunRecord, link: ClientRunLink): void {
  const chat = useChatStore.getState()
  const thread = chat.threads.find((candidate) => candidate.id === link.threadId)
  if (!thread) return
  if (thread.messages.some((message) => message.id === link.assistantMessageId)) return
  const input = objectPayload(run.spec.input)
  if (link.crewLiveMessageId && !thread.messages.some((message) => message.id === link.crewLiveMessageId)) {
    const crewLive = {
      streamId: link.crewStreamId,
      title: link.crewLiveTitle,
      status: 'running' as const,
      entries: [],
      agentColors: {},
      updatedAt: Date.now(),
    }
    chat.addMessageWithId(link.threadId, {
      id: link.crewLiveMessageId,
      role: 'assistant',
      content: buildCrewLiveMessageContent(crewLive),
      timestamp: Date.parse(run.spec.created_at),
      streaming: true,
      crewLive,
    })
  }
  if (input.scheduled === true) {
    const userMessageId = stringValue(input.client_user_message_id)
    if (userMessageId && !thread.messages.some((message) => message.id === userMessageId)) {
      chat.addMessageWithId(link.threadId, {
        id: userMessageId,
        role: 'user',
        content: stringValue(input.prompt) || 'Scheduled task',
        timestamp: Date.parse(run.spec.created_at),
      })
    }
  }
  chat.addMessageWithId(link.threadId, {
    id: link.assistantMessageId,
    role: 'assistant',
    content: '',
    timestamp: Date.parse(run.spec.created_at),
    durableRunId: run.spec.id,
    durableRunState: run.state,
    streaming: run.state === 'queued' || run.state === 'running',
  })
}

function applyCrewLog(link: ClientRunLink, rawLog: unknown): void {
  if (!link.crewLiveMessageId) return
  const log = objectPayload(rawLog) as Partial<CrewExecutionLog>
  if (typeof log.id !== 'string' || typeof log.action !== 'string') return
  const state = useChatStore.getState()
  const message = state.threads
    .find((thread) => thread.id === link.threadId)
    ?.messages.find((candidate) => candidate.id === link.crewLiveMessageId)
  const current = message?.crewLive ?? {
    streamId: link.crewStreamId,
    title: link.crewLiveTitle,
    status: 'running' as const,
    entries: [],
    agentColors: {},
    updatedAt: Date.now(),
  }
  if (current.entries.some((entry) => entry.id === log.id)) return
  const entry = createCrewLiveEntry(log as CrewExecutionLog)
  if (!entry) return
  const next = appendCrewLiveEntry(current, entry)
  state.updateMessage(link.threadId, link.crewLiveMessageId, {
    content: buildCrewLiveMessageContent(next),
    streaming: true,
    crewLive: next,
  })
}

function finishCrewMonitor(link: ClientRunLink, run: RunRecord): void {
  if (!link.crewLiveMessageId) return
  const state = useChatStore.getState()
  const message = state.threads
    .find((thread) => thread.id === link.threadId)
    ?.messages.find((candidate) => candidate.id === link.crewLiveMessageId)
  if (!message?.crewLive) return
  const status: CrewLiveStatus = run.state === 'completed'
    ? 'completed'
    : run.state === 'canceled'
      ? 'canceled'
      : 'failed'
  const next = { ...message.crewLive, status, updatedAt: Date.now() }
  state.updateMessage(link.threadId, link.crewLiveMessageId, {
    content: buildCrewLiveMessageContent(next),
    streaming: false,
    crewLive: next,
  }, { persist: true })
}

function updateToolCall(
  link: ClientRunLink,
  patch: Partial<LiveToolCall> & Pick<LiveToolCall, 'id' | 'toolName'>,
): void {
  const state = useChatStore.getState()
  const message = state.threads
    .find((thread) => thread.id === link.threadId)
    ?.messages.find((candidate) => candidate.id === link.assistantMessageId)
  const existing = message?.liveToolCalls ?? []
  const current = existing.find((call) => call.id === patch.id)
  const next: LiveToolCall = {
    id: patch.id,
    toolName: patch.toolName,
    input: patch.input ?? current?.input ?? {},
    status: patch.status ?? current?.status ?? 'requested',
    startedAt: current?.startedAt ?? patch.startedAt ?? Date.now(),
    result: patch.result ?? current?.result,
    error: patch.error ?? current?.error,
    finishedAt: patch.finishedAt ?? current?.finishedAt,
  }
  state.updateMessage(link.threadId, link.assistantMessageId, {
    liveToolCalls: [...existing.filter((call) => call.id !== next.id), next],
  })
}

function applyEvent(link: ClientRunLink, runId: string, event: RunEvent): void {
  const chat = useChatStore.getState()
  const payload = objectPayload(event.payload)
  if (event.kind === 'model_started') {
    chat.updateMessage(link.threadId, link.assistantMessageId, {
      durableRunId: runId,
      durableRunState: 'running',
      streaming: true,
    })
    return
  }
  if (event.kind === 'model_completed') {
    const response = objectPayload(payload.response)
    const logs = Array.isArray(response.logs) ? response.logs : []
    for (const log of logs) applyCrewLog(link, log)
    const content = stringValue(payload.content)
    if (content) chat.updateMessage(link.threadId, link.assistantMessageId, { content })
    return
  }
  if (event.kind === 'model_delta') {
    if (payload.adapter === 'codex') {
      const message = chat.threads
        .find((thread) => thread.id === link.threadId)
        ?.messages.find((candidate) => candidate.id === link.assistantMessageId)
      const delta = stringValue(payload.delta)
      const thinking = stringValue(payload.thinking)
      chat.updateMessage(link.threadId, link.assistantMessageId, {
        ...(delta ? { content: `${message?.content ?? ''}${delta}` } : {}),
        ...(thinking ? { thinkingContent: `${message?.thinkingContent ?? ''}${thinking}` } : {}),
      })
      return
    }
    const envelope = objectPayload(payload.crew_event)
    if (envelope.localAiCoworkEvent === 'crew_log') {
      applyCrewLog(link, envelope.payload)
    }
    return
  }
  if (event.kind === 'tool_started') {
    updateToolCall(link, {
      id: stringValue(payload.tool_call_id) || event.event_id,
      toolName: stringValue(payload.tool) || 'Tool',
      input: objectPayload(payload.arguments),
      status: 'running',
    })
    return
  }
  if (event.kind === 'tool_completed' || event.kind === 'tool_failed') {
    updateToolCall(link, {
      id: stringValue(payload.tool_call_id) || event.event_id,
      toolName: stringValue(payload.tool) || 'Tool',
      status: event.kind === 'tool_failed' ? 'failed' : 'completed',
      result: stringValue(payload.content),
      error: event.kind === 'tool_failed' ? stringValue(payload.content) : undefined,
      finishedAt: Date.now(),
    })
    return
  }
  if (event.kind === 'approval_requested') {
    const request = objectPayload(payload.request)
    const tool = stringValue(request.tool) || 'Tool'
    const requestId = stringValue(payload.id)
    chat.updateMessage(link.threadId, link.assistantMessageId, {
      durableRunId: runId,
      durableRunState: 'waiting_approval',
      durableRequestId: requestId,
      durableRequestKind: 'approval',
      streaming: false,
    }, { persist: true })
    updateToolCall(link, {
      id: stringValue(request.tool_call_id) || requestId,
      toolName: tool,
      input: objectPayload(request.arguments),
      status: 'approval',
      result: 'Approval required',
    })
    if (chat.activeThreadId === link.threadId) chat.setPendingApproval([`${tool}: Approval required`])
    return
  }
  if (event.kind === 'input_requested') {
    const request = objectPayload(payload.request)
    const question = stringValue(request.question) || 'The agent needs more information.'
    const requestId = stringValue(payload.id)
    chat.updateMessage(link.threadId, link.assistantMessageId, {
      content: `question: ${question}`,
      durableRunId: runId,
      durableRunState: 'waiting_input',
      durableRequestId: requestId,
      durableRequestKind: 'input',
      streaming: false,
    }, { persist: true })
    updateToolCall(link, {
      id: requestId,
      toolName: 'AskUser',
      input: request,
      status: 'waiting_input',
      result: question,
    })
    return
  }
  if (event.kind === 'state_changed') {
    const state = stringValue(payload.to)
    if (state) {
      chat.updateMessage(link.threadId, link.assistantMessageId, {
        durableRunState: state,
        streaming: state === 'queued' || state === 'running',
      })
    }
  }
}

function finalContent(run: RunRecord): string {
  const result = objectPayload(run.result)
  const content = stringValue(result.content)
  if (content) return content
  if (run.error?.message) return `Local run ${run.state}: ${run.error.message}`
  return `Local run ${run.state}.`
}

function applyFinalState(link: ClientRunLink, run: RunRecord): void {
  const chat = useChatStore.getState()
  chat.updateMessage(link.threadId, link.assistantMessageId, {
    content: finalContent(run),
    durableRunId: run.spec.id,
    durableRunState: run.state,
    durableRequestId: undefined,
    durableRequestKind: undefined,
    streaming: false,
  }, { persist: true })
  if (chat.activeThreadId === link.threadId) {
    chat.setBusy(false)
    chat.clearApproval()
  }
  finishCrewMonitor(link, run)
  if (link.taskId) {
    useWorkTasksStore.getState().updateTask(link.taskId, {
      status: run.state === 'completed'
        ? 'completed'
        : run.state === 'canceled'
          ? 'canceled'
          : 'failed',
      output: finalContent(run),
      error: run.state === 'completed' ? null : run.error?.message ?? `Run ${run.state}`,
      lastRunAt: Date.now(),
    })
  }
  void reconcileDurableLocalEntities().catch((error) => {
    console.warn('[local-daemon] Entity reconciliation failed', error)
  })
}

export function attachDurableLocalRun(
  client: LocalDaemonRuntimeClient,
  run: RunRecord,
): Promise<RunRecord> {
  const link = runLink(run)
  if (!link) return Promise.reject(new Error('Local run is missing its client message correlation'))
  ensureRunMessages(run, link)
  const existingWatcher = activeWatchers.get(run.spec.id)
  if (existingWatcher) return existingWatcher.done
  useChatStore.getState().updateMessage(link.threadId, link.assistantMessageId, {
    durableRunId: run.spec.id,
    durableRunState: run.state,
    streaming: run.state === 'queued' || run.state === 'running',
  }, { persist: true })
  const watcher = watchDurableLocalRun(client, run.spec.id, {
    onEvent: (event) => applyEvent(link, run.spec.id, event),
    onError: (error) => {
      useChatStore.getState().setError(error.message)
    },
  })
  const done = watcher.done.then((finalRun) => {
    activeWatchers.delete(run.spec.id)
    applyFinalState(link, finalRun)
    return finalRun
  }, (error) => {
    activeWatchers.delete(run.spec.id)
    throw error
  })
  activeWatchers.set(run.spec.id, { unsubscribe: watcher.unsubscribe, done })
  return done
}

export async function reconcileDurableLocalRuns(): Promise<void> {
  const client = createLocalDaemonRuntimeClient()
  await client.health()
  const [recentRuns, activeRuns] = await Promise.all([
    client.listRuns(),
    client.listActiveRuns(),
  ])
  const runs = [...new Map(
    [...recentRuns, ...activeRuns].map((run) => [run.spec.id, run]),
  ).values()]
  for (const run of runs) {
    const link = runLink(run)
    if (!link) continue
    await useChatStore.getState().reloadThreadMessages(link.threadId)
    ensureRunMessages(run, link)
    if (['completed', 'failed', 'canceled', 'expired', 'interrupted'].includes(run.state)) {
      applyFinalState(link, run)
    } else {
      void attachDurableLocalRun(client, run)
    }
  }
}

export function latestDurableMessage(threadId: string | null) {
  if (!threadId) return null
  return [...(useChatStore.getState().threads.find((thread) => thread.id === threadId)?.messages ?? [])]
    .reverse()
    .find((message) => message.role === 'assistant' && message.durableRunId) ?? null
}

export async function resolveLatestDurableApproval(threadId: string, approved: boolean): Promise<boolean> {
  const message = latestDurableMessage(threadId)
  if (!message?.durableRunId || message.durableRequestKind !== 'approval' || !message.durableRequestId) return false
  await createLocalDaemonRuntimeClient().resolveApproval(
    message.durableRunId,
    message.durableRequestId,
    approved,
  )
  useChatStore.getState().updateMessage(threadId, message.id, {
    durableRunState: 'running',
    durableRequestId: undefined,
    durableRequestKind: undefined,
    streaming: true,
  }, { persist: true })
  return true
}

export async function respondToLatestDurableInput(threadId: string, response: unknown): Promise<boolean> {
  const message = latestDurableMessage(threadId)
  if (!message?.durableRunId || message.durableRequestKind !== 'input' || !message.durableRequestId) return false
  await createLocalDaemonRuntimeClient().respondToInput(
    message.durableRunId,
    message.durableRequestId,
    response,
  )
  useChatStore.getState().updateMessage(threadId, message.id, {
    durableRunState: 'running',
    durableRequestId: undefined,
    durableRequestKind: undefined,
    streaming: true,
  }, { persist: true })
  return true
}

export async function cancelLatestDurableRun(threadId: string): Promise<boolean> {
  const message = latestDurableMessage(threadId)
  if (!message?.durableRunId || ['completed', 'failed', 'canceled', 'expired'].includes(message.durableRunState ?? '')) return false
  await createLocalDaemonRuntimeClient().cancelRun(message.durableRunId)
  return true
}
