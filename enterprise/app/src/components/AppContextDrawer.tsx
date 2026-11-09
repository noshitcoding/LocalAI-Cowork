import { useMemo } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import {
  Activity,
  Blocks,
  CheckCircle2,
  ExternalLink,
  FolderKanban,
  GitPullRequest,
  Globe2,
  ListTodo,
  Settings2,
  Trash2,
  UsersRound,
  X,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useCommandRegistry } from '../stores/commandRegistryStore'
import { useCrewStore } from '../stores/crewStore'
import { useProjectStore } from '../stores/projectStore'
import { useWorkTasksStore } from '../stores/workTasksStore'
import { tr } from '../i18n'
import { SETTINGS_SUBROUTES } from '../product/routeRegistry'
import { dispatchShellContextAction, type ShellContextAction } from '../product/shellContextActions'

type AppContextDrawerProps = {
  open: boolean
  onClose: () => void
}

type Metric = {
  label: string
  value: string | number
}

type Fact = {
  label: string
  value: string
}

type DrawerModel = {
  title: string
  icon: typeof Activity
  metrics: Metric[]
  facts: Fact[]
  description?: string
  actions?: Array<{
    label: string
    path?: string
    event?: ShellContextAction
    danger?: boolean
  }>
}

function activeTaskCount(tasks: ReturnType<typeof useWorkTasksStore.getState>['tasks']): number {
  return tasks.filter((task) => task.status === 'running' || task.status === 'waiting_approval').length
}

export default function AppContextDrawer({ open, onClose }: AppContextDrawerProps) {
  useTranslation()
  const location = useLocation()
  const navigate = useNavigate()
  const tasks = useWorkTasksStore((state) => state.tasks)
  const crews = useCrewStore((state) => state.crews)
  const activeCrewId = useCrewStore((state) => state.activeCrewId)
  const projects = useProjectStore((state) => state.projects)
  const activeProjectId = useProjectStore((state) => state.activeProjectId)
  const commands = useCommandRegistry((state) => state.commands)

  const model = useMemo<DrawerModel>(() => {
    if (location.pathname === '/tasks') {
      const active = activeTaskCount(tasks)
      const scheduled = tasks.filter((task) => task.scheduleEnabled).length
      const selectedTask = tasks.find((task) => new URLSearchParams(location.search).get('task') === task.id) ?? tasks[0]
      return {
        title: tr('Task overview'),
        icon: ListTodo,
        metrics: [
          { label: tr('Total'), value: tasks.length },
          { label: tr('Active'), value: active },
          { label: tr('Scheduled'), value: scheduled },
        ],
        facts: selectedTask ? [
          { label: tr('Selected task'), value: selectedTask.title || selectedTask.id },
          { label: tr('Status'), value: tr(selectedTask.status) },
          { label: tr('Runner'), value: selectedTask.runner === 'crew' ? tr('Crew') : tr('Model') },
        ] : [],
        description: selectedTask ? undefined : tr('No task selected'),
      }
    }

    if (location.pathname === '/crew') {
      const activeCrew = crews.find((crew) => crew.id === activeCrewId) ?? crews[0]
      const activeMembers = activeCrew?.agents.filter((agent) => agent.enabled).length ?? 0
      const blockers = activeCrew
        ? Number(activeMembers === 0) + Number(activeCrew.tasks.length === 0)
        : 0
      return {
        title: tr('Crew overview'),
        icon: UsersRound,
        metrics: [
          { label: tr('Crews'), value: crews.length },
          { label: tr('Active members'), value: activeMembers },
          { label: tr('Blockers'), value: blockers },
        ],
        facts: activeCrew ? [
          { label: tr('Crew'), value: activeCrew.name },
          { label: tr('Tasks'), value: String(activeCrew.tasks.length) },
          { label: tr('Status'), value: tr(activeCrew.status) },
          { label: tr('Process'), value: tr(activeCrew.process) },
        ] : [],
        description: activeCrew ? undefined : tr('No crew selected'),
        actions: activeCrew ? [
          { label: tr('Prepare mission in Tasks'), path: `/tasks?crew=${encodeURIComponent(activeCrew.id)}` },
          { label: tr('Duplicate'), event: 'crew-duplicate' },
          { label: tr('Export'), event: 'crew-export' },
          { label: tr('Import'), event: 'crew-import' },
          { label: tr('Delete crew'), event: 'crew-delete', danger: true },
        ] : [
          { label: tr('Import'), event: 'crew-import' },
        ],
      }
    }

    if (location.pathname === '/projects') {
      const activeProject = projects.find((project) => project.id === activeProjectId) ?? projects[0]
      return {
        title: tr('Project overview'),
        icon: FolderKanban,
        metrics: [
          { label: tr('Projects'), value: projects.length },
          { label: tr('sources'), value: activeProject?.resources.length ?? 0 },
          { label: tr('Chats'), value: activeProject?.threadIds.length ?? 0 },
        ],
        facts: activeProject ? [
          { label: tr('Project'), value: activeProject.title },
          { label: tr('brief'), value: activeProject.instructions.trim() ? tr('Ready') : tr('Draft') },
          { label: tr('Active sources'), value: String(activeProject.resources.filter((resource) => resource.enabled).length) },
        ] : [],
        description: activeProject ? undefined : tr('No projects yet.'),
        actions: activeProject ? [
          { label: tr('Delete project'), event: 'project-delete', danger: true },
        ] : undefined,
      }
    }

    if (location.pathname === '/features') {
      const activeTab = new URLSearchParams(location.search).get('tab') ?? 'mcp'
      const labels: Record<string, string> = {
        mcp: 'MCP Server',
        knowledge: 'Knowledge base',
        skills: 'Skills',
        commands: 'Slash commands',
      }
      return {
        title: tr('Capability overview'),
        icon: Blocks,
        metrics: [
          { label: tr('workbenches'), value: 4 },
          { label: tr('commands ready'), value: commands.length },
        ],
        facts: [
          { label: tr('Active workbench'), value: tr(labels[activeTab] ?? labels.mcp) },
        ],
        description: tr('Choose another workbench from the main menu.'),
      }
    }

    if (location.pathname === '/settings') {
      const section = new URLSearchParams(location.search).get('section') ?? 'ai'
      const sectionLabel = SETTINGS_SUBROUTES.find((entry) => entry.queryValue === section)?.labelKey ?? 'AI & model'
      return {
        title: tr('Settings'),
        icon: Settings2,
        metrics: [],
        facts: [
          { label: tr('Section'), value: tr(sectionLabel) },
          { label: tr('Save status'), value: tr('Saved automatically') },
        ],
        description: tr('Settings categories are available in the main menu.'),
      }
    }

    if (location.pathname === '/browser') {
      return {
        title: tr('Developer browser'),
        icon: Globe2,
        metrics: [],
        facts: [{ label: tr('Workspace'), value: tr('Browser tools') }],
        description: tr('Preview, inspect, annotate, and verify the active page.'),
      }
    }

    return {
      title: tr('GitHub workbench'),
      icon: GitPullRequest,
      metrics: [],
      facts: [{ label: tr('Workspace'), value: tr('Repository tools') }],
      description: tr('Choose a repository to inspect changes and pull requests.'),
    }
  }, [activeCrewId, activeProjectId, commands.length, crews, location.pathname, location.search, projects, tasks])

  if (!open || location.pathname === '/') return null
  const Icon = model.icon

  return (
    <div className="app-context-layer">
      <button type="button" className="app-drawer-backdrop" onClick={onClose} aria-label={tr('Close context and status')} />
      <aside className="app-context-drawer" role="dialog" aria-modal="true" aria-label={tr('Context & status')}>
        <header className="app-context-header">
          <span className="app-context-icon"><Icon size={17} aria-hidden="true" /></span>
          <div>
            <small>{tr('Context & status')}</small>
            <h2>{model.title}</h2>
          </div>
          <button type="button" onClick={onClose} aria-label={tr('Close context and status')}>
            <X size={17} aria-hidden="true" />
          </button>
        </header>

        <div className="app-context-scroll">
          {model.metrics.length > 0 && (
            <section className="app-context-metrics" aria-label={model.title}>
              {model.metrics.map((metric) => (
                <div key={metric.label}>
                  <strong>{metric.value}</strong>
                  <span>{metric.label}</span>
                </div>
              ))}
            </section>
          )}

          <section className="app-context-section">
            <h3><Activity size={14} aria-hidden="true" />{tr('Overview')}</h3>
            {model.facts.length > 0 ? (
              <dl className="app-context-facts">
                {model.facts.map((fact) => (
                  <div key={fact.label}><dt>{fact.label}</dt><dd>{fact.value}</dd></div>
                ))}
              </dl>
            ) : null}
            {model.description ? <p>{model.description}</p> : null}
          </section>

          {model.actions?.length ? (
            <section className="app-context-section app-context-actions">
              <h3>{tr('Actions')}</h3>
              {model.actions.map((action) => (
                <button
                  key={`${action.label}:${action.path ?? action.event}`}
                  type="button"
                  className={`app-context-action${action.danger ? ' danger' : ''}`}
                  onClick={() => {
                    if (action.path) navigate(action.path)
                    if (action.event) dispatchShellContextAction(action.event)
                    onClose()
                  }}
                >
                  <span>{action.label}</span>
                  {action.danger ? <Trash2 size={14} aria-hidden="true" /> : <ExternalLink size={14} aria-hidden="true" />}
                </button>
              ))}
            </section>
          ) : null}

          <div className="app-context-save-note">
            <CheckCircle2 size={14} aria-hidden="true" />
            <span>{tr('This overview uses existing local workspace data.')}</span>
          </div>
        </div>
      </aside>
    </div>
  )
}
