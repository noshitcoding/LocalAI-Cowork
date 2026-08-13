import type { Project, ProjectResource } from '../stores/projectStore'
import { safeInvoke } from './safeInvoke'

type WebFetchResponse = {
  url: string
  status: number
  ok: boolean
  title: string | null
  content: string
  truncated: boolean
}

export async function buildProjectLinkPromptContext(
  links: ProjectResource[],
): Promise<{ context: string; notice: string | null }> {
  if (links.length === 0) return { context: '', notice: null }

  const lines: string[] = ['Manually fetched project links:']
  const failures: string[] = []

  for (const link of links) {
    try {
      const response = await safeInvoke<WebFetchResponse>('web_fetch_url', {
        request: { url: link.path, maxChars: 4000 },
      })
      if (!response.ok) {
        failures.push(`${link.label ?? link.path}: HTTP ${response.status}`)
        continue
      }
      lines.push(`source: ${link.label ?? response.title ?? link.path}`)
      lines.push(`URL: ${response.url}`)
      if (response.title) lines.push(`Titel: ${response.title}`)
      lines.push(response.truncated ? `${response.content}\n[truncated]` : response.content)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      failures.push(`${link.label ?? link.path}: ${message}`)
    }
  }

  return {
    context: lines.length > 1 ? lines.join('\n\n') : '',
    notice: failures.length > 0
      ? `Not all project links could be fetched: ${failures.join('; ')}`
      : null,
  }
}

export function buildProjectInstructionsPromptContext(
  project: Pick<Project, 'title' | 'instructions'> | null | undefined,
): string {
  if (!project) return ''
  const instructions = project.instructions.trim()
  if (!instructions) return ''
  return `Project instructions for "${project.title}":\n${instructions}`
}

