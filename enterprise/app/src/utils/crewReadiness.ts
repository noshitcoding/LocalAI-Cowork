import type { Crew } from '../stores/crewStore'
import { useConfigStore } from '../stores/configStore'
import { resolveCrewProviderConfigsFromSettings, resolveEffectiveCrewProvider } from '../engine/crew/workTaskCrewRuntime'
import { safeInvoke } from './safeInvoke'

export type CrewReadinessIssue = { code: string; severity: 'error' | 'warning'; message: string; subjectId: string | null }
export type CrewReadinessReport = {
  crewId: string; definitionRevision: string; checkedAt: string; state: 'ready' | 'blocked'
  canRun: boolean; runtimeReady: boolean; issues: CrewReadinessIssue[]
}

export function crewReadinessPayload(crew: Crew) {
  const { llmProfiles, defaultLlmProfileIds, llmProfileModels, llmProfileModelCatalogs, ollama } = useConfigStore.getState()
  const resolution = resolveCrewProviderConfigsFromSettings(crew, llmProfiles, defaultLlmProfileIds, llmProfileModels, llmProfileModelCatalogs)
  const provider = resolveEffectiveCrewProvider(crew.defaultProvider ?? 'ollama', ollama, resolution.providerConfigs)
  return {
    id: crew.id, name: crew.name, process: crew.process, managerAgentId: crew.managerAgentId,
    defaultProvider: provider, defaultModel: crew.defaultModel,
    selectedProfileMissing: resolution.selectedProfileMissing,
    config: crew.runtimeConfig?.enabled ? crew.runtimeConfig : ollama,
    providerConfigs: resolution.providerConfigs,
    // Keep readiness invalidation tied to per-agent pins as well as the Crew default.
    pinnedProfileContext: crew.agents.map((agent) => agent.backendSelection ?? crew.defaultBackendSelection)
      .filter((selection) => selection?.backend === 'openai-compatible')
      .map((selection) => {
        const profile = llmProfiles.find((entry) => entry.id === selection.profileId)
        return profile ? { id: profile.id, model: profile.model, baseUrl: profile.baseUrl,
          authMode: profile.authMode, hasApiKey: profile.hasApiKey, verifyTlsCertificates: profile.verifyTlsCertificates,
          catalog: llmProfileModelCatalogs[profile.id] } : { id: selection.profileId, missing: true }
      }),
    agents: crew.agents.map((agent) => ({ id: agent.id, enabled: agent.enabled,
      modelOverride: agent.modelOverride, providerKind: provider,
      backendSelection: agent.backendSelection ?? crew.defaultBackendSelection,
      tools: agent.tools, mcpServerNames: agent.mcpServerNames })),
    tasks: crew.tasks.map((task) => ({ id: task.id, agentId: task.agentId, dependencies: task.dependencies })),
  }
}

export function crewReadinessKey(crew: Crew): string {
  return JSON.stringify(crewReadinessPayload(crew))
}

export async function inspectCrewReadiness(crew: Crew): Promise<CrewReadinessReport> {
  return safeInvoke<CrewReadinessReport>('crew_readiness_inspect', { request: { payload: crewReadinessPayload(crew) } })
}
