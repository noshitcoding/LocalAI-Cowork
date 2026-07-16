import type { AppMode } from '../stores/uiStore'

export type ProductRoute = {
  id: string
  path: `/${string}`
  viewId: `view:${string}`
  navLabelKey: string
  shortcutLabelKey: string
  shortcut: string
  shortcutKey: string
  commandId: string
  commandLabel: string
  navButtonDocId: `button:${string}`
  activeMode?: AppMode
}

export const PRODUCT_ROUTES = [
  {
    id: 'cowork',
    path: '/',
    viewId: 'view:/',
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
    navLabelKey: 'nav.crew',
    shortcutLabelKey: 'shortcuts.crew',
    shortcut: 'Ctrl+3',
    shortcutKey: '3',
    commandId: 'switch-crew',
    commandLabel: 'Switch to crew area',
    navButtonDocId: 'button:/app/top-navigation/crew',
    activeMode: 'crew',
  },
  {
    id: 'projects',
    path: '/projects',
    viewId: 'view:/projects',
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
    navLabelKey: 'nav.features',
    shortcutLabelKey: 'shortcuts.features',
    shortcut: 'Ctrl+5',
    shortcutKey: '5',
    commandId: 'switch-features',
    commandLabel: 'Switch to features',
    navButtonDocId: 'button:/app/top-navigation/features',
    activeMode: 'work',
  },
  {
    id: 'settings',
    path: '/settings',
    viewId: 'view:/settings',
    navLabelKey: 'nav.settings',
    shortcutLabelKey: 'shortcuts.settings',
    shortcut: 'Ctrl+6',
    shortcutKey: '6',
    commandId: 'switch-settings',
    commandLabel: 'Switch to settings',
    navButtonDocId: 'button:/app/top-navigation/settings',
    activeMode: 'settings',
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
