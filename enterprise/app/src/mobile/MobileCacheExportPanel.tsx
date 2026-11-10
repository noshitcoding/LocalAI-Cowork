import {useEffect, useRef, useState} from 'react'
import {exportOriginalMobileCache} from './mobileCacheExport'
import type {MobileCacheBinding} from './mobileOfflineStore'

type Prepared = {url: string; filename: string; sha256: string; size: number; payload: string; source: string; legacy: boolean}
/** A manual local export only. No remote runtime client or outbox is called. */
export default function MobileCacheExportPanel({binding}: {binding: MobileCacheBinding | null}) {
  const [prepared, setPrepared] = useState<Prepared>(), [busy, setBusy] = useState(false)
  const [reviewed, setReviewed] = useState(false), [error, setError] = useState('')
  const generation = useRef(0), current = useRef(binding)
  current.current = binding
  useEffect(() => {generation.current++; setPrepared(undefined); setReviewed(false); setError(''); setBusy(false)},
    [binding?.serverOrigin, binding?.userId])
  useEffect(() => () => {if (prepared) URL.revokeObjectURL(prepared.url)}, [prepared])
  useEffect(() => () => {generation.current++}, [])
  async function prepare(legacy: boolean) {
    if (busy) return
    const original = legacy ? null : current.current, epoch = generation.current
    if (!legacy && !original) return
    setBusy(true); setError(''); setReviewed(false)
    try {
      const value = await exportOriginalMobileCache(original)
      const bytes = new TextEncoder().encode(JSON.stringify(value))
      const sha256 = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)))
        .map(v => v.toString(16).padStart(2, '0')).join('')
      if (epoch !== generation.current) return
      const blob = new Blob([bytes], {type: 'application/json'})
      setPrepared({url: URL.createObjectURL(blob), filename: `mobile-cache-${sha256}.json`, sha256,
        size: bytes.length, payload: value.payload_sha256, source: value.source.storage_key, legacy})
    } catch (cause) {if (epoch === generation.current) setError(cause instanceof Error ? cause.message : String(cause))}
    finally {if (epoch === generation.current) setBusy(false)}
  }
  return <details className="mobile-outbox-list"><summary>Export original encrypted cache for V2</summary>
    <p>This local export contains private run history, messages and saved actions. Original encrypted bytes and exact decrypted JSON are preserved. The device encryption key is excluded. Nothing is sent to the server or replayed.</p>
    <button type="button" disabled={busy || !binding} onClick={() => {void prepare(false)}}>Prepare current account cache</button>
    <button type="button" disabled={busy} onClick={() => {void prepare(true)}}>Prepare original legacy cache</button>
    {error && <p role="alert">{error}</p>}
    {prepared && <><p>{prepared.legacy ? 'Legacy rows may belong to several historical accounts. They grant no current permissions.' : 'The original account binding is preserved as provenance; it grants no current permissions.'}</p>
      <p>{prepared.size.toLocaleString()} bytes · {prepared.source}</p><p>Export SHA-256: <code>{prepared.sha256}</code></p>
      <p>Original payload SHA-256: <code>{prepared.payload}</code></p>
      <label><input type="checkbox" checked={reviewed} onChange={event => setReviewed(event.target.checked)} />I want to save this private original export for an explicit V2 quarantine import.</label>
      {reviewed && <a className="ui-button ui-button--secondary ui-button--sm" href={prepared.url} download={prepared.filename}>Save original export JSON</a>}
    </>}
  </details>
}
