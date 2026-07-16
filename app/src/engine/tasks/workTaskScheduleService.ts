import type { ScheduledTask } from '../../stores/coworkStore'
import type { Crew } from '../../stores/crewStore'
import { safeInvoke } from '../../utils/safeInvoke'

export type CrewDefinitionVersionRow = {
  id: string
  crewId: string
  versionNumber: number
  changeSummary: string | null
  definitionJson: string
  createdAt: string
}

export type CrewScheduleSnapshotMetadata = {
  snapshotSource: 'live' | 'saved-version'
  definitionVersionId?: string
  definitionVersionNumber?: number
  definitionChangeSummary?: string | null
  definitionSavedAt?: string | null
}

export function findScheduledTask(scheduledTasks: ScheduledTask[], taskId: string): ScheduledTask | null {
  return scheduledTasks.find((entry) => entry.id === taskId) ?? null
}

function hydrateCrewFromDefinition(baseCrew: Crew, rawDefinition: string): Crew | null {
  try {
    const parsed = JSON.parse(rawDefinition) as Partial<Crew>
    return {
      ...baseCrew,
      ...parsed,
      providerProfiles: parsed.providerProfiles ?? baseCrew.providerProfiles,
      agents: Array.isArray(parsed.agents) ? parsed.agents : baseCrew.agents,
      tasks: Array.isArray(parsed.tasks) ? parsed.tasks : baseCrew.tasks,
      runtimeConfig: parsed.runtimeConfig ?? baseCrew.runtimeConfig,
      status: baseCrew.status,
      createdAt: baseCrew.createdAt,
      updatedAt: baseCrew.updatedAt,
    }
  } catch {
    return null
  }
}

export async function resolveCrewScheduleSource(crew: Crew): Promise<{ crew: Crew; metadata: CrewScheduleSnapshotMetadata }> {
  try {
    const versions = await safeInvoke<CrewDefinitionVersionRow[]>('crew_definition_versions_list', { crewId: crew.id }, [])
    const latestVersion = Array.isArray(versions) ? versions[0] : undefined
    if (!latestVersion?.definitionJson?.trim()) {
      return {
        crew,
        metadata: { snapshotSource: 'live' },
      }
    }

    const hydrated = hydrateCrewFromDefinition(crew, latestVersion.definitionJson)
    if (!hydrated) {
      return {
        crew,
        metadata: { snapshotSource: 'live' },
      }
    }

    return {
      crew: hydrated,
      metadata: {
        snapshotSource: 'saved-version',
        definitionVersionId: latestVersion.id,
        definitionVersionNumber: latestVersion.versionNumber,
        definitionChangeSummary: latestVersion.changeSummary,
        definitionSavedAt: latestVersion.createdAt,
      },
    }
  } catch {
    return {
      crew,
      metadata: { snapshotSource: 'live' },
    }
  }
}

export function readCrewScheduleSnapshotMetadata(snapshotJson: string | null | undefined): CrewScheduleSnapshotMetadata | null {
  if (!snapshotJson?.trim()) {
    return null
  }

  try {
    const parsed = JSON.parse(snapshotJson) as Partial<CrewScheduleSnapshotMetadata>
    if (parsed.snapshotSource !== 'live' && parsed.snapshotSource !== 'saved-version') {
      return null
    }

    return {
      snapshotSource: parsed.snapshotSource,
      definitionVersionId: typeof parsed.definitionVersionId === 'string' ? parsed.definitionVersionId : undefined,
      definitionVersionNumber: typeof parsed.definitionVersionNumber === 'number' ? parsed.definitionVersionNumber : undefined,
      definitionChangeSummary: typeof parsed.definitionChangeSummary === 'string' || parsed.definitionChangeSummary === null ? parsed.definitionChangeSummary : undefined,
      definitionSavedAt: typeof parsed.definitionSavedAt === 'string' || parsed.definitionSavedAt === null ? parsed.definitionSavedAt : undefined,
    }
  } catch {
    return null
  }
}
