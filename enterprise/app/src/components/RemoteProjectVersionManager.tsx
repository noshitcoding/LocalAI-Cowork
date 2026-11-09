import { GitMerge, History, RefreshCw, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'

import type {
  MergeFileStatus,
  MergeResolutionChoice,
  ProjectMergeReview,
  ProjectRecord,
  ProjectVersion,
} from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import './RemoteManagement.css'

type Props = { client: RemoteRuntimeClient; compact?: boolean }
type ResolutionMap = Record<string, Exclude<MergeResolutionChoice, 'auto_merged'> | undefined>

const conflictStatuses = new Set<MergeFileStatus>(['text_conflict', 'binary_conflict'])

function messageOf(error: unknown): string { return error instanceof Error ? error.message : String(error) }
function statusLabel(status: MergeFileStatus): string { return status.replaceAll('_', ' ') }

export default function RemoteProjectVersionManager({ client, compact = false }: Props) {
  const [open, setOpen] = useState(false)
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const [projectId, setProjectId] = useState('')
  const [versions, setVersions] = useState<ProjectVersion[]>([])
  const [resultVersionId, setResultVersionId] = useState('')
  const [review, setReview] = useState<ProjectMergeReview | null>(null)
  const [resolutions, setResolutions] = useState<ResolutionMap>({})
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  const loadProjects = useCallback(async () => {
    const next = await client.listProjects()
    setProjects(next)
    setProjectId((current) => next.some((project) => project.id === current) ? current : next[0]?.id ?? '')
    return next
  }, [client])

  const loadVersions = useCallback(async (nextProjectId: string, currentVersionId?: string | null) => {
    if (!nextProjectId) { setVersions([]); setResultVersionId(''); return [] }
    const next = await client.listProjectVersions(nextProjectId)
    setVersions(next)
    setResultVersionId((current) => next.some((version) => version.id === current)
      ? current
      : next.find((version) => version.id !== currentVersionId)?.id ?? next[0]?.id ?? '')
    return next
  }, [client])

  useEffect(() => {
    if (!open) return
    setBusy(true)
    loadProjects().then(() => setError(null)).catch((cause) => setError(messageOf(cause))).finally(() => setBusy(false))
  }, [loadProjects, open])

  useEffect(() => {
    if (!open || !projectId) return
    setBusy(true)
    loadVersions(projectId, projects.find((item) => item.id === projectId)?.current_version_id)
      .then(() => setError(null)).catch((cause) => setError(messageOf(cause))).finally(() => setBusy(false))
  }, [loadVersions, open, projectId, projects])

  useEffect(() => { setReview(null); setResolutions({}); setSuccess(null) }, [projectId])

  const project = projects.find((item) => item.id === projectId)
  const resultVersion = versions.find((version) => version.id === resultVersionId)
  const baseVersionId = resultVersion?.merge_base_version_id ?? resultVersion?.parent_version_id ?? null
  const action = useMemo(() => {
    if (!project || !resultVersion) return 'unavailable' as const
    if (project.current_version_id === resultVersion.id) return 'current' as const
    if (!project.current_version_id) return 'direct' as const
    if (resultVersion.parent_version_id === project.current_version_id
      || resultVersion.merge_base_version_id === project.current_version_id) return 'direct' as const
    return baseVersionId ? 'merge' as const : 'unavailable' as const
  }, [baseVersionId, project, resultVersion])
  const changedFiles = review?.files.filter((file) => file.status !== 'unchanged') ?? []
  const conflicts = changedFiles.filter((file) => conflictStatuses.has(file.status))
  const unresolved = conflicts.filter((file) => !resolutions[file.path])

  const refresh = async () => {
    setBusy(true); setError(null)
    try {
      const nextProjects = await loadProjects()
      if (projectId) await loadVersions(
        projectId, nextProjects.find((item) => item.id === projectId)?.current_version_id,
      )
    }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }

  const beginReview = async () => {
    if (!project?.current_version_id || !resultVersion || !baseVersionId) return
    setBusy(true); setError(null); setSuccess(null)
    try {
      const next = await client.reviewProjectMerge(
        project.id, baseVersionId, project.current_version_id, resultVersion.id,
      )
      setReview(next); setResolutions({})
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }

  const applyDirectly = async () => {
    if (!project || !resultVersion) return
    setBusy(true); setError(null); setSuccess(null)
    try {
      const applied = await client.applyProjectVersion(
        project.id, resultVersion.id, project.revision, project.current_version_id,
      )
      setSuccess(`Version ${applied.revision} is now current.`)
      const nextProjects = await loadProjects()
      await loadVersions(project.id, nextProjects.find((item) => item.id === project.id)?.current_version_id)
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }

  const applyMerge = async () => {
    if (!project || !review || unresolved.length > 0) return
    setBusy(true); setError(null); setSuccess(null)
    try {
      const applied = await client.applyProjectMerge(project.id, {
        base_version_id: review.base_version_id,
        current_version_id: review.current_version_id,
        result_version_id: review.result_version_id,
        expected_project_revision: project.revision,
        resolutions: Object.entries(resolutions).flatMap(([path, choice]) => choice ? [{ path, choice }] : []),
      })
      setReview(null); setResolutions({}); setSuccess(`Merged version ${applied.revision} is now current.`)
      const nextProjects = await loadProjects()
      await loadVersions(project.id, nextProjects.find((item) => item.id === project.id)?.current_version_id)
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }

  if (!open) return <button className={compact ? '' : 'ui-button ui-button--secondary ui-button--sm'} type="button" onClick={() => setOpen(true)}><History size={14} /> Versions</button>
  return (
    <section className={`remote-management-panel remote-version-manager${compact ? ' compact' : ''}`}>
      <header><div><GitMerge size={16} /><strong>Project versions</strong></div><button type="button" aria-label="Close project versions" onClick={() => setOpen(false)}><X size={15} /></button></header>
      <p className="remote-management-hint">Review run results against the current project state. Applying a merge creates a new immutable version and updates the project atomically.</p>
      <div className="remote-version-toolbar">
        <label>Project<select aria-label="Version project" value={projectId} onChange={(event) => setProjectId(event.target.value)} disabled={busy}>{projects.map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}</select></label>
        <label>Result version<select aria-label="Result version" value={resultVersionId} onChange={(event) => { setResultVersionId(event.target.value); setReview(null); setResolutions({}); setSuccess(null) }} disabled={busy || versions.length === 0}>{versions.map((version) => <option key={version.id} value={version.id}>Version {version.revision}{version.id === project?.current_version_id ? ' (current)' : ''}</option>)}</select></label>
        <button type="button" aria-label="Refresh project versions" disabled={busy} onClick={() => { void refresh() }}><RefreshCw size={14} /></button>
      </div>
      {versions.length === 0 && !busy ? <p className="remote-management-hint">This project has no uploaded result versions yet.</p> : null}
      {action === 'direct' ? <div className="remote-management-actions"><button type="button" disabled={busy} onClick={() => { void applyDirectly() }}>Apply version</button></div> : null}
      {action === 'merge' && !review ? <div className="remote-management-actions"><button type="button" disabled={busy} onClick={() => { void beginReview() }}><GitMerge size={14} /> Review three-way merge</button></div> : null}
      {action === 'current' ? <p className="remote-inline-success">This is the current project version.</p> : null}
      {action === 'unavailable' && resultVersion ? <p className="remote-management-hint">This result has no common base version and cannot be merged safely.</p> : null}
      {review ? (
        <div className="remote-merge-review">
          <div className="remote-merge-summary"><strong>{changedFiles.length} changed file{changedFiles.length === 1 ? '' : 's'}</strong><small>{conflicts.length} conflict{conflicts.length === 1 ? '' : 's'}</small></div>
          <div className="remote-management-list">
            {changedFiles.length === 0 ? <p>No file changes.</p> : changedFiles.map((file) => (
              <article key={file.path} className={conflictStatuses.has(file.status) ? 'remote-merge-conflict' : ''}>
                <span><strong>{file.path}</strong><small>{statusLabel(file.status)}{file.renamed_from ? ` from ${file.renamed_from}` : ''}</small>{file.conflict_preview ? <pre>{file.conflict_preview}</pre> : null}</span>
                {conflictStatuses.has(file.status) ? <label>Resolve<select aria-label={`Resolve ${file.path}`} value={resolutions[file.path] ?? ''} onChange={(event) => setResolutions((current) => ({ ...current, [file.path]: event.target.value as ResolutionMap[string] }))}><option value="">Choose…</option><option value="current">Keep current</option><option value="result">Use result</option><option value="delete">Delete</option></select></label> : null}
              </article>
            ))}
          </div>
          <div className="remote-management-actions"><button type="button" disabled={busy || unresolved.length > 0} onClick={() => { void applyMerge() }}><GitMerge size={14} /> Apply merge</button><button type="button" disabled={busy} onClick={() => { setReview(null); setResolutions({}) }}>Cancel review</button></div>
          {unresolved.length > 0 ? <p className="remote-management-hint">Resolve every text and binary conflict before applying.</p> : null}
        </div>
      ) : null}
      {success ? <p className="remote-inline-success" role="status">{success}</p> : null}
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
