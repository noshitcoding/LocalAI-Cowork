import { useEffect, useRef, useState } from 'react'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import { enterprisePost } from './enterpriseRecovery'

type Destination = { kind: 'device'; id: string; grant_id: string } | { kind: 'environment'; id: string }
type Step = { id: string; ordinal: number; state: string; dependencies: string[]; destination: Destination; has_original_receipt: boolean }
type Detail = { workflow: { id: string; revision: number; state: string; chat_id: string }; steps: Step[] }
export default function EnterpriseWorkflowInspection({ value, client, user, grants, context, tr, onChanged }: { value: unknown; client: RemoteRuntimeClient; user: string; grants: { id: string; device_id: string; chat_id: string; state: string }[]; context?: { environments: { id: string; name: string }[] }; tr: (de: string, en: string) => string; onChanged: () => void }) {
  const detail = value as Detail | undefined, [selected, setSelected] = useState(''), [destination, setDestination] = useState(''), [dependencies, setDependencies] = useState<string[]>([]), [receipt, setReceipt] = useState<unknown>(), [busy, setBusy] = useState(false), [error, setError] = useState('')
  const originalScope=JSON.stringify([client.enterpriseSource,user,detail?.workflow?.id,detail?.workflow?.revision]),currentScope=useRef(originalScope),working=useRef(false);currentScope.current=originalScope
  useEffect(() => { setSelected(''); setDestination(''); setDependencies([]); setReceipt(undefined); setError('') }, [client, user, detail?.workflow?.id, detail?.workflow?.revision])
  if (!detail?.workflow?.id || !Array.isArray(detail.steps) || detail.steps.length > 256) return null
  const current = detail.steps.find(s => s.id === selected)
  async function act(work: () => Promise<void>) { if (working.current) return;const source=originalScope;working.current=true; setBusy(true); try { await work();if(currentScope.current===source)setError('') } catch (cause) {if(currentScope.current===source)setError(String(cause)) } finally { working.current=false;if(currentScope.current===source){setBusy(false);onChanged()} } }
  async function reassign() {
    if (!current || !detail) return; const [kind, id] = destination.split(':'); let target: Destination
    if (kind === 'device') { const grant = grants.find(g => g.id === id && g.chat_id === detail.workflow.chat_id && g.state === 'active'); if (!grant) throw new Error('Current confirmed device grant required'); target = { kind: 'device', id: grant.device_id, grant_id: grant.id } }
    else { const environment = context?.environments.find(e => e.id === id); if (!environment) throw new Error('Current environment required'); target = { kind: 'environment', id: environment.id } }
    const receipt = await enterprisePost(client, user, '/workflows/' + detail.workflow.id + '/reassign', { request_id: crypto.randomUUID(), expected_revision: detail.workflow.revision, step_id: current.id, destination: target, depends_on: dependencies }); if(currentScope.current===originalScope)setReceipt(receipt)
  }
  return <article><p>{tr('Originalworkflow', 'Original workflow')} {detail.workflow.id} · {detail.workflow.state}</p>
    {detail.steps.map(step => <div key={step.id}><p>{tr('Schritt', 'Step')} {step.ordinal + 1} · {step.destination.kind} · {step.state}</p><button disabled={busy || !step.has_original_receipt} onClick={() => void act(async () => { const result = await client.enterprise(`/workflows/${detail.workflow.id}/steps/${step.id}`,'GET',undefined,user); if(currentScope.current===originalScope)setReceipt(result) })}>{tr('Originalergebnis lesen', 'Read original result')}</button>{step.state === 'pending' && detail.workflow.state === 'held' && <button disabled={busy} onClick={() => { setSelected(step.id); setDestination(''); setDependencies(step.dependencies) }}>{tr('Noch nicht ausgeführten Schritt neu zuordnen', 'Reassign an undispatched step')}</button>}</div>)}
    {current && <fieldset><legend>{tr('Neue Zuordnung vor der ersten Ausführung', 'New assignment before first execution')}</legend><p>{tr('Der ursprüngliche Auftrag bleibt erhalten. Wähle das neue Ausführungsziel und die weiterhin benötigten Vorgängerschritte.', 'The original task is retained. Select the new destination and the predecessor steps it still needs.')}</p><select value={destination} onChange={e => setDestination(e.target.value)}><option value="">—</option>{current.destination.kind === 'device' ? grants.filter(g => g.chat_id === detail.workflow.chat_id && g.state === 'active').map(g => <option value={'device:' + g.id} key={g.id}>{g.device_id}</option>) : context?.environments.map(e => <option value={'environment:' + e.id} key={e.id}>{e.name}</option>)}</select>
      {detail.steps.filter(s => s.id !== current.id).map(s => <label key={s.id}><input type="checkbox" checked={dependencies.includes(s.id)} onChange={e => setDependencies(values => e.target.checked ? [...values, s.id] : values.filter(id => id !== s.id))} />{tr('Warten auf Schritt', 'Wait for step')} {s.ordinal + 1} · {s.state}</label>)}<button disabled={busy || !destination} onClick={() => void act(reassign)}>{tr('Neue Zuordnung bestätigen', 'Confirm new assignment')}</button>
    </fieldset>}{error && <p role="alert">{error}</p>}{receipt !== undefined && <pre>{JSON.stringify(receipt, null, 2)}</pre>}
  </article>
}
