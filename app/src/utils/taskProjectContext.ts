import type { Project } from '../stores/projectStore'
import { getProjectForThread, useProjectStore } from '../stores/projectStore'
import { useWorkTasksStore } from '../stores/workTasksStore'
import { hasTauriRuntime, safeInvoke } from './safeInvoke'

export type AuthorizedTaskPath = {
  path: string
  kind: 'file' | 'folder'
  access: 'read_write'
}

export type TaskProjectRunContext = {
  projectId: string | null
  projectTitle: string | null
  promptContext: string
  authorizedPaths: AuthorizedTaskPath[]
  preferredCwd: string | null
  warnings: string[]
}

type ResolveTaskProjectRunContextInput = {
  taskId?: string
  threadId?: string
  prompt: string
  workDir?: string
}

function emptyContext(workDir?: string): TaskProjectRunContext {
  const normalizedWorkDir = workDir?.trim() ?? ''
  return {
    projectId: null,
    projectTitle: null,
    promptContext: '',
    authorizedPaths: normalizedWorkDir
      ? [{ path: normalizedWorkDir, kind: 'folder', access: 'read_write' }]
      : [],
    preferredCwd: normalizedWorkDir || null,
    warnings: [],
  }
}

function resolveBrowserProject(input: ResolveTaskProjectRunContextInput): Project | null {
  const task = input.taskId
    ? useWorkTasksStore.getState().tasks.find((entry) => entry.id === input.taskId)
    : null
  const threadId = input.threadId?.trim() || task?.threadId || null
  return getProjectForThread(useProjectStore.getState().projects, threadId)
}

function buildBrowserFallback(input: ResolveTaskProjectRunContextInput): TaskProjectRunContext {
  const project = resolveBrowserProject(input)
  const result = emptyContext(input.workDir)
  if (!project) return result

  const enabledResources = project.resources.filter((resource) => resource.enabled)
  const authorizedPaths = enabledResources
    .filter((resource): resource is typeof resource & { kind: 'file' | 'folder' } => (
      resource.kind === 'file' || resource.kind === 'folder'
    ))
    .map((resource) => ({
      path: resource.path,
      kind: resource.kind,
      access: 'read_write' as const,
    }))
  const preferredProjectFolder = authorizedPaths.find((entry) => entry.kind === 'folder')?.path ?? null
  const sourceLines = enabledResources.map((resource) => (
    `- ${resource.kind}: ${resource.label?.trim() || resource.path} (${resource.path})`
  ))

  return {
    projectId: project.id,
    projectTitle: project.title,
    promptContext: [
      `Project context: "${project.title}"`,
      project.instructions.trim()
        ? `Project instructions:\n${project.instructions.trim()}`
        : '',
      sourceLines.length > 0
        ? `Enabled project sources:\n${sourceLines.join('\n')}`
        : 'Enabled project sources: none',
    ].filter(Boolean).join('\n\n'),
    authorizedPaths: [
      ...result.authorizedPaths,
      ...authorizedPaths.filter((entry) => (
        !result.authorizedPaths.some((existing) => existing.path === entry.path)
      )),
    ],
    preferredCwd: result.preferredCwd || preferredProjectFolder,
    warnings: [],
  }
}

export async function resolveTaskProjectRunContext(
  input: ResolveTaskProjectRunContextInput,
): Promise<TaskProjectRunContext> {
  if (!hasTauriRuntime()) return buildBrowserFallback(input)

  return safeInvoke<TaskProjectRunContext>('task_project_context_resolve', {
    request: {
      taskId: input.taskId?.trim() || null,
      threadId: input.threadId?.trim() || null,
      prompt: input.prompt,
      workDir: input.workDir?.trim() || null,
    },
  }, emptyContext(input.workDir))
}

export function appendTaskProjectPrompt(
  prompt: string,
  context: TaskProjectRunContext,
): string {
  const warnings = context.warnings.length > 0
    ? `Project context warnings:\n${context.warnings.map((warning) => `- ${warning}`).join('\n')}`
    : ''
  return [prompt.trim(), context.promptContext.trim(), warnings]
    .filter(Boolean)
    .join('\n\n')
}
