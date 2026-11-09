import { Download, File, RefreshCw } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'

import type { RunArtifact } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'

type RunArtifactPanelProps = {
  client: RemoteRuntimeClient
  runId: string
  reloadKey?: number
}

type ArtifactPreview = {
  artifact: RunArtifact
  url: string
  text?: string
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`
}

function isTextMediaType(mediaType: string): boolean {
  return mediaType.startsWith('text/')
    || mediaType === 'application/json'
    || mediaType.endsWith('+json')
    || mediaType === 'application/xml'
}

export default function RunArtifactPanel({ client, runId, reloadKey = 0 }: RunArtifactPanelProps) {
  const [artifactState, setArtifactState] = useState<{ runId: string; items: RunArtifact[] }>({ runId, items: [] })
  const [loading, setLoading] = useState(true)
  const [previewLoading, setPreviewLoading] = useState(false)
  const [preview, setPreview] = useState<ArtifactPreview | null>(null)
  const [error, setError] = useState<string | null>(null)
  const listGeneration = useRef(0)
  const previewGeneration = useRef(0)
  const currentRunId = useRef(runId)
  const currentClient = useRef(client)
  const artifacts = artifactState.runId === runId ? artifactState.items : []

  useEffect(() => {
    currentRunId.current = runId
    currentClient.current = client
  }, [client, runId])

  const load = useCallback(async () => {
    const generation = ++listGeneration.current
    const requestedRunId = runId
    setLoading(true)
    setError(null)
    setArtifactState({ runId: requestedRunId, items: [] })
    try {
      const items = await client.listArtifacts(requestedRunId)
      if (listGeneration.current !== generation || currentRunId.current !== requestedRunId || currentClient.current !== client) return
      setArtifactState({ runId: requestedRunId, items })
    } catch (cause) {
      if (listGeneration.current !== generation || currentRunId.current !== requestedRunId || currentClient.current !== client) return
      setError(messageOf(cause))
    } finally {
      if (listGeneration.current === generation && currentRunId.current === requestedRunId && currentClient.current === client) {
        setLoading(false)
      }
    }
  }, [client, runId])

  useEffect(() => {
    previewGeneration.current += 1
    setPreview(null)
    setPreviewLoading(false)
    void load()
    return () => {
      listGeneration.current += 1
      previewGeneration.current += 1
    }
  }, [load, reloadKey])

  useEffect(() => () => {
    if (preview?.url) URL.revokeObjectURL(preview.url)
  }, [preview])

  const openArtifact = async (artifact: RunArtifact) => {
    const requestedRunId = artifactState.runId
    if (requestedRunId !== runId || artifact.run_id !== requestedRunId) return
    const generation = ++previewGeneration.current
    setPreviewLoading(true)
    setError(null)
    let url: string | null = null
    try {
      const blob = await client.downloadArtifact(requestedRunId, artifact.id)
      url = URL.createObjectURL(blob)
      const text = isTextMediaType(artifact.media_type) ? await blob.text() : undefined
      if (previewGeneration.current !== generation || currentRunId.current !== requestedRunId || currentClient.current !== client) {
        URL.revokeObjectURL(url)
        return
      }
      setPreview({ artifact, url, text })
      url = null
    } catch (cause) {
      if (url) URL.revokeObjectURL(url)
      if (previewGeneration.current !== generation || currentRunId.current !== requestedRunId || currentClient.current !== client) return
      setError(messageOf(cause))
    } finally {
      if (previewGeneration.current === generation && currentRunId.current === requestedRunId && currentClient.current === client) {
        setPreviewLoading(false)
      }
    }
  }

  return (
    <section className="run-artifact-panel">
      <header className="remote-section-header">
        <div>
          <h2>Artifacts</h2>
          <p>Encrypted run outputs, decrypted only after an authorized download.</p>
        </div>
        <button className="ui-button ui-button--ghost ui-button--sm" type="button" onClick={() => { void load() }} disabled={loading}>
          <RefreshCw size={14} /> Refresh
        </button>
      </header>
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
      {loading ? <p className="remote-muted">Loading artifacts…</p> : null}
      {!loading && artifacts.length === 0 ? <p className="remote-muted">This run has no artifacts yet.</p> : null}
      <div className="run-artifact-list">
        {artifacts.map((artifact) => (
          <button
            type="button"
            key={artifact.id}
            className={`run-artifact-row${preview?.artifact.id === artifact.id ? ' selected' : ''}`}
            onClick={() => { void openArtifact(artifact) }}
            disabled={previewLoading}
          >
            <File size={16} />
            <span><strong>{artifact.name}</strong><small>{artifact.media_type} · {formatBytes(artifact.size_bytes)}</small></span>
          </button>
        ))}
      </div>
      {preview ? (
        <div className="run-artifact-preview">
          <header>
            <strong>{preview.artifact.name}</strong>
            <a className="ui-button ui-button--secondary ui-button--sm" href={preview.url} download={preview.artifact.name}>
              <Download size={14} /> Download
            </a>
          </header>
          {preview.artifact.media_type.startsWith('image/') ? <img src={preview.url} alt={preview.artifact.name} /> : null}
          {preview.artifact.media_type.startsWith('video/') ? <video src={preview.url} controls aria-label={preview.artifact.name} /> : null}
          {preview.artifact.media_type.startsWith('audio/') ? <audio src={preview.url} controls aria-label={preview.artifact.name} /> : null}
          {preview.artifact.media_type === 'application/pdf' ? <iframe src={preview.url} title={preview.artifact.name} /> : null}
          {preview.text !== undefined ? <pre>{preview.text}</pre> : null}
          {!preview.artifact.media_type.startsWith('image/')
            && !preview.artifact.media_type.startsWith('video/')
            && !preview.artifact.media_type.startsWith('audio/')
            && preview.artifact.media_type !== 'application/pdf'
            && preview.text === undefined
            ? <p className="remote-muted">No inline preview is available for this file type. Use Download to open it locally.</p>
            : null}
        </div>
      ) : null}
    </section>
  )
}
