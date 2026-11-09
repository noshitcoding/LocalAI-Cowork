import type {Project} from "@cowork/client";

// Storage still needs a project FK; the personal chat space is not a user project.
export const independentChats = "__cowork_independent_chats__";
export function visibleProjects(projects: readonly Project[]): Project[] {
  return projects.filter(project => project.id !== independentChats);
}
