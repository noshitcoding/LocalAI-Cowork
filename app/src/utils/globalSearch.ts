import type { SessionSummary } from '../engine'
import type { ChatThread } from '../stores/chatStore'
import type { Project } from '../stores/projectStore'
import type { WorkTask } from '../stores/workTasksStore'

export type SearchIndexEntryType =
  | 'thread'
  | 'message'
  | 'task'
  | 'project'
  | 'project-resource'
  | 'session'
  | 'setting'
  | 'feature'

export type SearchIndexAction =
  | 'open-thread'
  | 'open-project'
  | 'open-task'
  | 'open-session'
  | 'open-settings'
  | 'open-features'

export type SearchIndexEntry = {
  id: string
  type: SearchIndexEntryType
  title: string
  subtitle: string
  body: string
  href: string
  action: SearchIndexAction
  targetId: string
  score?: number
  updatedAt?: number
}

type BuildSearchIndexInput = {
  threads: ChatThread[]
  tasks: WorkTask[]
  projects: Project[]
  sessions: SessionSummary[]
}

const STATIC_ENTRIES: SearchIndexEntry[] = [
  {
    id: 'setting-models',
    type: 'setting',
    title: 'AI and models',
    subtitle: 'Providers, model selection, Ollama, and profiles',
    body: 'Ollama OpenAI OpenRouter Anthropic Provider Model Gesatdheit Diagnose',
    href: '/settings',
    action: 'open-settings',
    targetId: 'models',
  },
  {
    id: 'setting-agent',
    type: 'setting',
    title: 'Agent and skills',
    subtitle: 'Execution, plan-only mode, policies, and tools',
    body: 'agent Skills Tools approvaln Plat Mode Policies',
    href: '/settings',
    action: 'open-settings',
    targetId: 'agent',
  },
  {
    id: 'setting-sessions',
    type: 'setting',
    title: 'Sessions and Memory',
    subtitle: 'Persistence, context, history, and compression',
    body: 'Sessions Memory Context Compaction history Persistence',
    href: '/settings',
    action: 'open-settings',
    targetId: 'sessions',
  },
  {
    id: 'setting-safety',
    type: 'setting',
    title: 'Security and file access',
    subtitle: 'Allowlist, approvals, audit, and secrets',
    body: 'Security allowlist release audit secrets files',
    href: '/settings',
    action: 'open-settings',
    targetId: 'safety',
  },
  {
    id: 'setting-interface',
    type: 'setting',
    title: 'Interface',
    subtitle: 'Theme, focus mode, shortcuts, and layout',
    body: 'Interface Theme Fokusmodus Shortcuts Layout Language',
    href: '/settings',
    action: 'open-settings',
    targetId: 'interface',
  },
  {
    id: 'feature-overview',
    type: 'feature',
    title: 'Features',
    subtitle: 'MCP servers, skills, plugins, Crew AI, and artifacts',
    body: 'MCP Server Skills Plugins Connector Registry Crew AI Artefakte Diff Preview',
    href: '/features',
    action: 'open-features',
    targetId: 'features',
  },
]

function normalize(value: string): string {
  return value
    .normalize('NFD')
    .replace(/\p{Diacritic}/gu, '')
    .toLowerCase()
}

function compact(parts: Array<string | null | undefined>): string {
  return parts
    .filter((part): part is string => typeof part === 'string' && part.trim().length > 0)
    .map((part) => part.trim())
    .join(' ')
}

export function getTaskSearchTitle(task: WorkTask): string {
  const title = task.title.trim()
  if (title) return title
  const prompt = task.prompt.trim().replace(/\s+/g, ' ')
  if (!prompt) return task.id
  return prompt.length > 48 ? `${prompt.slice(0, 48)}...` : prompt
}

export function buildSearchIndex({ threads, tasks, projects, sessions }: BuildSearchIndexInput): SearchIndexEntry[] {
  const entries: SearchIndexEntry[] = []

  for (const thread of threads) {
    const visibleMessages = thread.messages.filter((message) => message.role !== 'system')
    const messageBody = visibleMessages.map((message) => message.content).join('\n')
    entries.push({
      id: `thread:${thread.id}`,
      type: 'thread',
      title: thread.title || 'Untitled chat',
      subtitle: `${visibleMessages.length} Messages`,
      body: messageBody,
      href: '/',
      action: 'open-thread',
      targetId: thread.id,
      updatedAt: thread.updatedAt,
    })

    for (const message of visibleMessages) {
      const content = message.content.trim()
      if (!content) continue
      entries.push({
        id: `message:${thread.id}:${message.id}`,
        type: 'message',
        title: content.length > 72 ? `${content.slice(0, 72)}...` : content,
        subtitle: `${message.role} in ${thread.title || 'Untitled chat'}`,
        body: content,
        href: '/',
        action: 'open-thread',
        targetId: thread.id,
        updatedAt: message.timestamp,
      })
    }
  }

  for (const task of tasks) {
    entries.push({
      id: `task:${task.id}`,
      type: 'task',
      title: getTaskSearchTitle(task),
      subtitle: `Task / ${task.status} / ${task.runner === 'crew' ? 'Crew' : 'Model'}`,
      body: compact([task.prompt, task.expectedOutput, task.workDir, task.model, task.error ?? undefined, task.output ?? undefined]),
      href: task.threadId ? '/' : '/tasks',
      action: 'open-task',
      targetId: task.id,
      updatedAt: task.updatedAt,
    })
  }

  for (const project of projects) {
    entries.push({
      id: `project:${project.id}`,
      type: 'project',
      title: project.title || 'Untitled project',
      subtitle: `${project.threadIds.length} Chats / ${project.resources.length} Resources`,
      body: compact([project.instructions, ...project.resources.map((resource) => compact([resource.label, resource.path, resource.kind]))]),
      href: '/projects',
      action: 'open-project',
      targetId: project.id,
      updatedAt: project.updatedAt,
    })

    for (const resource of project.resources) {
      entries.push({
        id: `project-resource:${project.id}:${resource.id}`,
        type: 'project-resource',
        title: resource.label || resource.path,
        subtitle: `${resource.kind} in ${project.title || 'Project'}`,
        body: compact([resource.path, resource.kind, resource.enabled ? 'enabled' : 'disabled']),
        href: '/projects',
        action: 'open-project',
        targetId: project.id,
        updatedAt: resource.addedAt,
      })
    }
  }

  for (const session of sessions) {
    entries.push({
      id: `session:${session.id}`,
      type: 'session',
      title: session.title || 'Untitled session',
      subtitle: `${session.messageCount} Messages`,
      body: compact([session.cwd, session.threadId]),
      href: '/',
      action: 'open-session',
      targetId: session.id,
      updatedAt: session.updatedAt,
    })
  }

  return [...entries, ...STATIC_ENTRIES]
}

export function scoreSearchEntry(entry: SearchIndexEntry, query: string): number {
  const normalizedQuery = normalize(query.trim())
  if (!normalizedQuery) return 1

  const title = normalize(entry.title)
  const subtitle = normalize(entry.subtitle ?? '')
  const body = normalize(entry.body ?? '')

  if (title === normalizedQuery) return 120
  if (title.startsWith(normalizedQuery)) return 95
  if (title.includes(normalizedQuery)) return 80
  if (subtitle.includes(normalizedQuery)) return 56
  if (body.includes(normalizedQuery)) return 36

  const words = normalizedQuery.split(/\s+/).filter(Boolean)
  if (words.length > 1) {
    const haystack = `${title} ${subtitle} ${body}`
    const matchedWords = words.filter((word) => haystack.includes(word)).length
    if (matchedWords === words.length) return 28 + matchedWords
    if (matchedWords > 0) return 12 + matchedWords
  }

  return 0
}

export function filterSearchIndex(entries: SearchIndexEntry[], query: string, limit = 40): SearchIndexEntry[] {
  const normalizedQuery = query.trim()
  const scored = entries
    .map((entry) => ({ ...entry, score: scoreSearchEntry(entry, normalizedQuery) }))
    .filter((entry) => entry.score > 0)

  scored.sort((a, b) => {
    if ((b.score ?? 0) !== (a.score ?? 0)) return (b.score ?? 0) - (a.score ?? 0)
    return (b.updatedAt ?? 0) - (a.updatedAt ?? 0)
  })

  return scored.slice(0, limit)
}




