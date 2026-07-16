import type { WorkTask, WorkTaskStatus } from '../../stores/workTasksStore'
import type { Crew } from '../../stores/crewStore'
import type { DefaultLlmProfileIds, LlmProfile } from '../../stores/configStore'
import type { ChatProviderSelection } from '../../utils/chatProvider'
import { tr } from '../../i18n'
import type { CrewExecutionResponse } from '../crew/workTaskCrewRuntime'

export type CrewMissionDraft = {
  title: string
  prompt: string
  expectedOutput: string
  workDir: string
  runner: 'crew'
  crewId: string
  model: string
}

type WorkTaskChatProviderContext = {
  crews: readonly Pick<Crew, 'id' | 'defaultProvider' | 'defaultModel'>[]
  ollamaModel: string
  defaultLlmProfileIds: DefaultLlmProfileIds
  llmProfiles: readonly LlmProfile[]
  fallbackProviderSettings?: ChatProviderSelection
}

export function resolveWorkTaskChatProviderSettings(
  task: WorkTask,
  context: WorkTaskChatProviderContext,
): ChatProviderSelection | undefined {
  if (task.runner === 'crew') {
    const crew = task.crewId ? context.crews.find((item) => item.id === task.crewId) : null
    if (!crew) return undefined

    const provider = crew.defaultProvider ?? 'ollama'
    const profileId = provider === 'ollama' ? undefined : context.defaultLlmProfileIds[provider]
    const defaultProfile = provider === 'ollama'
      ? undefined
      : context.llmProfiles.find((profile) => profile.id === profileId && profile.provider === provider)
        ?? context.llmProfiles.find((profile) => profile.provider === provider)
    const model = crew.defaultModel?.trim()
      || (provider === 'ollama' ? context.ollamaModel.trim() : defaultProfile?.model.trim() ?? '')

    return {
      provider,
      ...(model ? { model } : {}),
      ...(defaultProfile?.id ? { profileId: defaultProfile.id } : {}),
    }
  }

  const model = task.model.trim()
  if (!context.fallbackProviderSettings && !model) return undefined

  return {
    ...(context.fallbackProviderSettings ?? { provider: 'ollama' as const }),
    ...(model ? { model } : {}),
  }
}

export function buildCrewMissionId(crewId: string): string {
  return `crew-mission-${crewId}`
}

export function buildCrewMissionDraft(crew: Pick<Crew, 'id' | 'name' | 'description' | 'tasks'>): CrewMissionDraft {
  const crewTasks = crew.tasks ?? []
  const firstStep = crewTasks.find((task) => task.description.trim())
  const finalStep = [...crewTasks].reverse().find((task) => task.expectedOutput.trim())

  return {
    title: `${crew.name.trim() || 'Crew'} · Mission`,
    prompt: crew.description.trim()
      || firstStep?.description.trim()
      || `Run the complete ${crew.name.trim() || 'crew'} workflow.`,
    expectedOutput: finalStep?.expectedOutput.trim() || 'A complete, reviewed result.',
    workDir: '',
    runner: 'crew',
    crewId: crew.id,
    model: '',
  }
}

export function buildCrewMissionTask(
  crew: Pick<Crew, 'id' | 'name' | 'description' | 'tasks'>,
  now = Date.now(),
): WorkTask {
  return {
    id: buildCrewMissionId(crew.id),
    ...buildCrewMissionDraft(crew),
    threadId: null,
    scheduleExpr: '',
    scheduleEnabled: false,
    status: 'idle',
    output: null,
    error: null,
    lastRunAt: null,
    createdAt: now,
    updatedAt: now,
  }
}

export function buildCrewRunOutput(response: CrewExecutionResponse, fallbackTaskId: string): string {
  const directResult = response.taskResults.find((result) => result.taskId === fallbackTaskId)
  if (directResult?.output?.trim()) {
    return directResult.output
  }

  const renderedResults = response.taskResults
    .filter((result) => result.output?.trim())
    .map((result) => [
      `Task: ${result.taskId}`,
      `Agent: ${result.agentId}`,
      result.output?.trim() ?? '',
    ].filter(Boolean).join('\n'))

  if (renderedResults.length > 0) {
    return renderedResults.join('\n\n---\n\n')
  }

  return response.error ?? 'Crew run completed without text output.'
}

export function formatTimestamp(ts: number | null | undefined): string {
  if (!ts) return '-'
  try {
    return new Date(ts).toLocaleString('de-DE')
  } catch {
    return String(ts)
  }
}

export function formatWorkTaskStatus(status: WorkTaskStatus): string {
  switch (status) {
    case 'idle':
      return tr('Idle')
    case 'waiting_approval':
      return tr('Waiting for approval')
    case 'running':
      return tr('Running')
    case 'completed':
      return tr('Completed')
    case 'failed':
      return tr('Failed')
    case 'canceled':
      return tr('Canceled')
  }
}

export function deriveTaskName(task: WorkTask): string {
  const title = task.title.trim()
  if (title) return title
  const prompt = task.prompt.trim()
  if (!prompt) return task.id
  const singleLine = prompt.replace(/\s+/g, ' ').trim()
  return singleLine.length > 48 ? `${singleLine.slice(0, 48)}...` : singleLine
}

export function isAbsolutePath(path: string): boolean {
  const trimmed = path.trim()
  return /^(?:[a-zA-Z]:[\\/]|\\\\|\/)/.test(trimmed)
}

export function createCrewStreamId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return `crew-${crypto.randomUUID()}`
  }

  return `crew-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
}

export function buildTaskThreadSummary(task: WorkTask): string {
  const lines = [
    `${tr('Task created')}: ${deriveTaskName(task)}`,
    `${tr('Runner')}: ${task.runner === 'crew' ? tr('Crew') : tr('Model')}`,
    task.expectedOutput.trim() ? `${tr('Expected output')}: ${task.expectedOutput.trim()}` : '',
    task.workDir.trim() ? `${tr('Working folder')}: ${task.workDir.trim()}` : '',
  ].filter(Boolean)

  return lines.join('\n')
}

export function buildTaskPromptMessage(task: WorkTask): string {
  const parts = [task.prompt.trim()]

  if (task.expectedOutput.trim()) {
    parts.push(`${tr('Expected output')}:\n${task.expectedOutput.trim()}`)
  }

  if (task.workDir.trim()) {
    parts.push(`${tr('Working folder')}:\n${task.workDir.trim()}`)
  }

  return parts.filter(Boolean).join('\n\n')
}
