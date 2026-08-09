import { Download, File, RefreshCw } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'

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
  const [artifacts, setArtifacts] = useState<RunArtifact[]>([])
  const [loading, setLoading] = useState(true)
  const [previewLoading, setPreviewLoading] = useState(false)
  const [preview, setPreview] = useState<ArtifactPreview | null>(null)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setArtifacts(await client.listArtifacts(runId))
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setLoading(false)
    }
  }, [client, runId])

  useEffect(() => {
    void load()
  }, [load, reloadKey])

  useEffect(() => () => {
    if (preview?.url) URL.revokeObjectURL(preview.url)
  }, [preview])

  const openArtifact = async (artifact: RunArtifact) => {
    setPreviewLoading(true)
    setError(null)
    try {
      const blob = await client.downloadArtifact(runId, artifact.id)
      const url = URL.createObjectURL(blob)
      const text = isTextMediaType(artifact.media_type) ? await blob.text() : undefined
      setPreview({ artifact, url, text })
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setPreviewLoading(false)
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
