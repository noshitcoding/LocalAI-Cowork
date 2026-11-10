import { create } from 'zustand'
import type { Crew } from './crewStore'
import { safeInvoke } from '../utils/safeInvoke'
import { crewReadinessKey, inspectCrewReadiness, type CrewReadinessReport } from '../utils/crewReadiness'

export type CrewDefinitionRow = {
  id: string
  name: string
  description: string
  definitionJson: string
  flowJson: string | null
  versionCount: number
  createdAt: string
  updatedAt: string
}

export type CrewDefinitionVersionRow = {
  id: string
  crewId: string
  versionNumber: number
  changeSummary: string | null
  definitionJson: string
  createdAt: string
}

export type CrewRoleBindingRow = {
  id: string
  scopeType: string
  scopeRef: string | null
  role: string
  subject: string
  createdAt: string
  updatedAt: string
}

export type CrewApprovalRow = {
  id: string
  crewId: string | null
  runId: string | null
  approvalType: string
  scopeRef: string | null
  status: string
  requestedBy: string | null
  resolvedBy: string | null
  payloadJson: string | null
  resolutionNote: string | null
  requestedAt: string
  resolvedAt: string | null
  createdAt: string
  updatedAt: string
}

export type CrewValidationResult = {
  valid: boolean
  issues: string[]
  normalized: Record<string, unknown> | null
}

type CrewControlPlaneState = {
  definitions: CrewDefinitionRow[]
  versions: CrewDefinitionVersionRow[]
  roleBindings: CrewRoleBindingRow[]
  approvals: CrewApprovalRow[]
  validation: CrewValidationResult | null
  readiness: CrewReadinessReport | null
  readinessKey: string | null
  loading: boolean
  error: string | null
  loadDefinitions: () => Promise<void>
  saveCrewDefinition: (crew: Crew, changeSummary?: string) => Promise<void>
  loadVersions: (crewId: string) => Promise<void>
  validateCrew: (crew: Crew) => Promise<void>
  loadRoleBindings: (scopeType?: string, scopeRef?: string) => Promise<void>
  upsertRoleBinding: (request: { id: string; scopeType: string; scopeRef?: string | null; role: string; subject: string }) => Promise<void>
  loadApprovals: (status?: string, crewId?: string) => Promise<void>
  createApproval: (request: { id: string; crewId?: string | null; runId?: string | null; approvalType: string; scopeRef?: string | null; status?: string; requestedBy?: string | null; payloadJson?: string | null }) => Promise<void>
  resolveApproval: (request: { id: string; status: string; crewId?: string | null; resolvedBy?: string | null; resolutionNote?: string | null }) => Promise<void>
}

function serializeCrewDefinition(crew: Crew): Record<string, unknown> {
  return {
    id: crew.id,
    name: crew.name,
    description: crew.description,
    executionSubject: crew.executionSubject,
    executionGuidelines: crew.executionGuidelines,
    knowledgeFocus: crew.knowledgeFocus,
    governanceMode: crew.governanceMode,
    outputMode: crew.outputMode,
    stopOnFailure: crew.stopOnFailure,
    retryCount: crew.retryCount,
    managerReviewEnabled: crew.managerReviewEnabled,
    managerReviewGuidelines: crew.managerReviewGuidelines,
    shareAllTaskOutputs: crew.shareAllTaskOutputs,
    sharedOutputCharLimit: crew.sharedOutputCharLimit,
    defaultProvider: crew.defaultProvider,
    defaultModel: crew.defaultModel,
    providerProfiles: crew.providerProfiles,
    process: crew.process,
    managerAgentId: crew.managerAgentId,
    verbose: crew.verbose,
    maxRpm: crew.maxRpm,
    maxParallelTasks: crew.maxParallelTasks,
    runtimeConfig: crew.runtimeConfig,
    agents: crew.agents,
    tasks: crew.tasks,
  }
}

let readinessRequestGeneration = 0

export const useCrewControlPlaneStore = create<CrewControlPlaneState>()((set, get) => ({
  definitions: [],
  versions: [],
  roleBindings: [],
  approvals: [],
  validation: null,
  readiness: null,
  readinessKey: null,
  loading: false,
  error: null,

  loadDefinitions: async () => {
    set({ loading: true, error: null })
    try {
      const definitions = await safeInvoke<CrewDefinitionRow[]>('crew_definition_list', undefined, [])
      set({ definitions: Array.isArray(definitions) ? definitions : [], loading: false })
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error), loading: false })
    }
  },

  saveCrewDefinition: async (crew, changeSummary) => {
    set({ loading: true, error: null })
    try {
      await safeInvoke<CrewDefinitionRow>('crew_definition_upsert', {
        request: {
          id: crew.id,
          name: crew.name,
          description: crew.description,
          definitionJson: JSON.stringify(serializeCrewDefinition(crew), null, 2),
          flowJson: JSON.stringify({ process: crew.process, managerAgentId: crew.managerAgentId }, null, 2),
          changeSummary: changeSummary?.trim() || null,
        },
      }, undefined)
      await Promise.all([get().loadDefinitions(), get().loadVersions(crew.id)])
      set({ loading: false })
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error), loading: false })
    }
  },

  loadVersions: async (crewId) => {
    try {
      const versions = await safeInvoke<CrewDefinitionVersionRow[]>('crew_definition_versions_list', { crewId }, [])
      set({ versions: Array.isArray(versions) ? versions : [] })
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error) })
    }
  },

  validateCrew: async (crew) => {
    const generation = ++readinessRequestGeneration
    const key = crewReadinessKey(crew)
    set({ loading: true, error: null, validation: null, readiness: null, readinessKey: key })
    try {
      const readiness = await inspectCrewReadiness(crew)
      if (generation !== readinessRequestGeneration || get().readinessKey !== key) return
      set({ readiness, validation: { valid: readiness.canRun,
        issues: readiness.issues.map((issue) => issue.message), normalized: null }, loading: false })
    } catch (error) {
      if (generation !== readinessRequestGeneration || get().readinessKey !== key) return
      set({ error: error instanceof Error ? error.message : String(error), loading: false })
    }
  },

  loadRoleBindings: async (scopeType, scopeRef) => {
    try {
      const roleBindings = await safeInvoke<CrewRoleBindingRow[]>('crew_role_binding_list', {
        scopeType: scopeType ?? null,
        scopeRef: scopeRef ?? null,
      }, [])
      set({ roleBindings: Array.isArray(roleBindings) ? roleBindings : [] })
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error) })
    }
  },

  upsertRoleBinding: async (request) => {
    set({ loading: true, error: null })
    try {
      await safeInvoke<CrewRoleBindingRow>('crew_role_binding_upsert', {
        request: {
          ...request,
          scopeRef: request.scopeRef ?? null,
        },
      }, undefined)
      await get().loadRoleBindings(request.scopeType, request.scopeRef ?? undefined)
      set({ loading: false })
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error), loading: false })
    }
  },

  loadApprovals: async (status, crewId) => {
    try {
      const approvals = await safeInvoke<CrewApprovalRow[]>('crew_approval_list', {
        status: status ?? null,
        crewId: crewId ?? null,
      }, [])
      set({ approvals: Array.isArray(approvals) ? approvals : [] })
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error) })
    }
  },

  createApproval: async (request) => {
    set({ loading: true, error: null })
    try {
      await safeInvoke<CrewApprovalRow>('crew_approval_create', {
        request: {
          ...request,
          crewId: request.crewId ?? null,
          runId: request.runId ?? null,
          scopeRef: request.scopeRef ?? null,
          status: request.status ?? 'pending',
          requestedBy: request.requestedBy ?? null,
          payloadJson: request.payloadJson ?? null,
        },
      }, undefined)
      await get().loadApprovals(undefined, request.crewId ?? undefined)
      set({ loading: false })
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error), loading: false })
    }
  },

  resolveApproval: async (request) => {
    set({ loading: true, error: null })
    try {
      await safeInvoke<CrewApprovalRow>('crew_approval_resolve', {
        request: {
          ...request,
          resolvedBy: request.resolvedBy ?? null,
          resolutionNote: request.resolutionNote ?? null,
        },
      }, undefined)
      await get().loadApprovals(undefined, request.crewId ?? undefined)
      set({ loading: false })
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error), loading: false })
    }
  },
}))
