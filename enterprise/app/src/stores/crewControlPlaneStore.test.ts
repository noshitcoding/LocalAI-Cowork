import { describe, expect, it, vi } from 'vitest'
import type { Crew } from './crewStore'
import { useCrewControlPlaneStore } from './crewControlPlaneStore'
import { inspectCrewReadiness, type CrewReadinessReport } from '../utils/crewReadiness'

vi.mock('../utils/crewReadiness', () => ({ crewReadinessKey: () => 'unchanged-definition', inspectCrewReadiness: vi.fn() }))
const inspect = vi.mocked(inspectCrewReadiness)
const crew = { id: 'crew' } as Crew
const report = (canRun: boolean): CrewReadinessReport => ({ crewId: 'crew', definitionRevision: 'same',
  checkedAt: '', state: canRun ? 'ready' : 'blocked', canRun, runtimeReady: canRun, issues: [] })

describe('readiness request generations', () => {
  it('discards an older success for the same definition after a newer readiness result', async () => {
    let oldResult!: (value: CrewReadinessReport) => void
    inspect.mockImplementationOnce(() => new Promise((resolve) => { oldResult = resolve }))
      .mockResolvedValueOnce(report(false))
    const older = useCrewControlPlaneStore.getState().validateCrew(crew)
    await useCrewControlPlaneStore.getState().validateCrew(crew)
    oldResult(report(true))
    await older
    expect(useCrewControlPlaneStore.getState().readiness?.canRun).toBe(false)
  })

  it('discards an older failure for the same definition after a newer success', async () => {
    let failOld!: (error: Error) => void
    inspect.mockImplementationOnce(() => new Promise((_, reject) => { failOld = reject }))
      .mockResolvedValueOnce(report(true))
    const older = useCrewControlPlaneStore.getState().validateCrew(crew)
    await useCrewControlPlaneStore.getState().validateCrew(crew)
    failOld(new Error('stale failure'))
    await older
    expect(useCrewControlPlaneStore.getState().readiness?.canRun).toBe(true)
    expect(useCrewControlPlaneStore.getState().error).toBeNull()
  })
})
