import type { AppMode } from '../stores/uiStore'

export type ProductRoute = {
  id: string
  path: `/${string}`
  viewId: `view:${string}`
  group: 'workspace' | 'development' | 'system'
  titleKey: string
  navLabelKey: string
  shortcutLabelKey: string
  shortcut: string
  shortcutKey: string
  commandId: string
  commandLabel: string
  navButtonDocId: `button:${string}`
  activeMode?: AppMode
  subroutes?: readonly ProductSubroute[]
}

export type ProductSubroute = {
  id: string
  labelKey: string
  descriptionKey?: string
  queryKey: 'tab' | 'section'
  queryValue: string
  default?: boolean
}

export const FEATURE_SUBROUTES = [
  { id: 'mcp', labelKey: 'MCP Server', descriptionKey: 'Connect external tools and runtimes', queryKey: 'tab', queryValue: 'mcp', default: true },
  { id: 'knowledge', labelKey: 'Knowledge base', descriptionKey: 'Curate reusable workspace context', queryKey: 'tab', queryValue: 'knowledge' },
  { id: 'skills', labelKey: 'Skills', descriptionKey: 'Shape repeatable expert workflows', queryKey: 'tab', queryValue: 'skills' },
  { id: 'commands', labelKey: 'Slash commands', descriptionKey: 'Launch actions without leaving chat', queryKey: 'tab', queryValue: 'commands' },
] as const satisfies readonly ProductSubroute[]

export const SETTINGS_SUBROUTES = [
  { id: 'ai', labelKey: 'AI & model', queryKey: 'section', queryValue: 'ai', default: true },
  { id: 'agent', labelKey: 'Agent & Skills', queryKey: 'section', queryValue: 'agent' },
  { id: 'memory', labelKey: 'Memory', queryKey: 'section', queryValue: 'memory' },
  { id: 'runs', labelKey: 'Runs & Insights', queryKey: 'section', queryValue: 'runs' },
  { id: 'terminal', labelKey: 'Terminal & Processes', queryKey: 'section', queryValue: 'terminal' },
  { id: 'mcp', labelKey: 'MCP Server', queryKey: 'section', queryValue: 'mcp' },
  { id: 'ui', labelKey: 'Interface', queryKey: 'section', queryValue: 'ui' },
  { id: 'sandbox', labelKey: 'AI Sandbox', queryKey: 'section', queryValue: 'sandbox' },
  { id: 'security', labelKey: 'Security & data', queryKey: 'section', queryValue: 'security' },
  { id: 'system', labelKey: 'System & Info', queryKey: 'section', queryValue: 'system' },
] as const satisfies readonly ProductSubroute[]

export const CREW_SUBROUTES = [
  { id: 'general', labelKey: 'General', queryKey: 'section', queryValue: 'general', default: true },
  { id: 'execution', labelKey: 'Execution', queryKey: 'section', queryValue: 'execution' },
  { id: 'provider', labelKey: 'Provider & Model', queryKey: 'section', queryValue: 'provider' },
  { id: 'diagnostics', labelKey: 'Diagnostics', queryKey: 'section', queryValue: 'diagnostics' },
  { id: 'members', labelKey: 'Crew members', queryKey: 'section', queryValue: 'members' },
  { id: 'mission', labelKey: 'Task-Flow', queryKey: 'section', queryValue: 'mission' },
] as const satisfies readonly ProductSubroute[]

export const PRODUCT_ROUTES = [
  {
    id: 'cowork',
    path: '/',
    viewId: 'view:/',
    group: 'workspace',
    titleKey: 'nav.cowork',
    navLabelKey: 'nav.cowork',
    shortcutLabelKey: 'shortcuts.workspace',
    shortcut: 'Ctrl+1',
    shortcutKey: '1',
    commandId: 'switch-work',
    commandLabel: 'Switch to workspace',
    navButtonDocId: 'button:/app/top-navigation/cowork',
    activeMode: 'work',
  },
  {
    id: 'tasks',
    path: '/tasks',
    viewId: 'view:/tasks',
    group: 'workspace',
    titleKey: 'nav.tasks',
    navLabelKey: 'nav.tasks',
    shortcutLabelKey: 'shortcuts.tasks',
    shortcut: 'Ctrl+2',
    shortcutKey: '2',
    commandId: 'switch-tasks',
    commandLabel: 'Switch to tasks',
    navButtonDocId: 'button:/app/top-navigation/tasks',
    activeMode: 'work',
  },
  {
    id: 'crew',
    path: '/crew',
    viewId: 'view:/crew',
    group: 'workspace',
    titleKey: 'nav.crew',
    navLabelKey: 'nav.crew',
    shortcutLabelKey: 'shortcuts.crew',
    shortcut: 'Ctrl+3',
    shortcutKey: '3',
    commandId: 'switch-crew',
    commandLabel: 'Switch to crew area',
    navButtonDocId: 'button:/app/top-navigation/crew',
    activeMode: 'crew',
    subroutes: CREW_SUBROUTES,
  },
  {
    id: 'projects',
    path: '/projects',
    viewId: 'view:/projects',
    group: 'workspace',
    titleKey: 'nav.projects',
    navLabelKey: 'nav.projects',
    shortcutLabelKey: 'shortcuts.projects',
    shortcut: 'Ctrl+4',
    shortcutKey: '4',
    commandId: 'switch-projects',
    commandLabel: 'Switch to projects',
    navButtonDocId: 'button:/app/top-navigation/projects',
    activeMode: 'work',
  },
  {
    id: 'features',
    path: '/features',
    viewId: 'view:/features',
    group: 'workspace',
    titleKey: 'nav.features',
    navLabelKey: 'nav.features',
    shortcutLabelKey: 'shortcuts.features',
    shortcut: 'Ctrl+5',
    shortcutKey: '5',
    commandId: 'switch-features',
    commandLabel: 'Switch to features',
    navButtonDocId: 'button:/app/top-navigation/features',
    activeMode: 'work',
    subroutes: FEATURE_SUBROUTES,
  },
  {
    id: 'browser',
    path: '/browser',
    viewId: 'view:/browser',
    group: 'development',
    titleKey: 'nav.browser',
    navLabelKey: 'nav.browser',
    shortcutLabelKey: 'shortcuts.browser',
    shortcut: 'Ctrl+6',
    shortcutKey: '6',
    commandId: 'switch-browser',
    commandLabel: 'Switch to developer browser',
    navButtonDocId: 'button:/app/top-navigation/browser',
    activeMode: 'work',
  },
  {
    id: 'github',
    path: '/github',
    viewId: 'view:/github',
    group: 'development',
    titleKey: 'nav.github',
    navLabelKey: 'nav.github',
    shortcutLabelKey: 'shortcuts.github',
    shortcut: 'Ctrl+7',
    shortcutKey: '7',
    commandId: 'switch-github',
    commandLabel: 'Switch to GitHub workbench',
    navButtonDocId: 'button:/app/top-navigation/github',
    activeMode: 'work',
  },
  {
    id: 'server',
    path: '/server',
    viewId: 'view:/server',
    group: 'workspace',
    titleKey: 'nav.server',
    navLabelKey: 'nav.server',
    shortcutLabelKey: 'shortcuts.server',
    shortcut: 'Ctrl+8',
    shortcutKey: '8',
    commandId: 'switch-server',
    commandLabel: 'Switch to server runs',
    navButtonDocId: 'button:/app/top-navigation/server',
    activeMode: 'work',
  },
  {
    id: 'settings',
    path: '/settings',
    viewId: 'view:/settings',
    group: 'system',
    titleKey: 'nav.settings',
    navLabelKey: 'nav.settings',
    shortcutLabelKey: 'shortcuts.settings',
    shortcut: 'Ctrl+9',
    shortcutKey: '9',
    commandId: 'switch-settings',
    commandLabel: 'Switch to settings',
    navButtonDocId: 'button:/app/top-navigation/settings',
    activeMode: 'settings',
    subroutes: SETTINGS_SUBROUTES,
  },
] as const satisfies readonly ProductRoute[]

export type ProductRouteId = (typeof PRODUCT_ROUTES)[number]['id']
export type ProductRoutePath = (typeof PRODUCT_ROUTES)[number]['path']

export function getProductRouteById(id: ProductRouteId): (typeof PRODUCT_ROUTES)[number] {
  const route = PRODUCT_ROUTES.find((route) => route.id === id)
  if (!route) {
    throw new Error(`Unknown product route: ${id}`)
  }
  return route
}

export function getProductRouteByShortcutKey(key: string) {
  return PRODUCT_ROUTES.find((route) => route.shortcutKey === key)
}
