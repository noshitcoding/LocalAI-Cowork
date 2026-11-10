export const SHELL_CONTEXT_ACTION_EVENT = 'open-cowork:shell-context-action'

export type ShellContextAction =
  | 'crew-duplicate'
  | 'crew-export'
  | 'crew-import'
  | 'crew-delete'
  | 'project-delete'

export function dispatchShellContextAction(action: ShellContextAction) {
  window.dispatchEvent(new CustomEvent<ShellContextAction>(SHELL_CONTEXT_ACTION_EVENT, { detail: action }))
}
