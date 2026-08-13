import { Bot, KeyRound, Plus, Save, Trash2, X } from 'lucide-react'
import { useCallback, useEffect, useState, type FormEvent } from 'react'

import type { ProviderProfile, TeamRecord } from '../runtime/contracts'
import { providerEndpointBinding, providerModelLabel } from '../runtime/remoteExecutionOptions'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import './RemoteManagement.css'

type Props = { client: RemoteRuntimeClient; compact?: boolean }
type Defaults = {
  baseUrl: string
  model: string
  authMode: 'none' | 'bearer'
  timeoutMs: number
  maxSteps: number
  verifyTls: boolean
}

function messageOf(error: unknown): string { return error instanceof Error ? error.message : String(error) }
function defaultsOf(profile: ProviderProfile): Defaults {
  const value = typeof profile.model_defaults === 'object' && profile.model_defaults !== null
    ? profile.model_defaults as Record<string, unknown>
    : {}
  return {
    baseUrl: typeof value.base_url === 'string' ? value.base_url : '',
    model: typeof value.model === 'string' ? value.model : '',
    authMode: value.auth_mode === 'none' ? 'none' : 'bearer',
    timeoutMs: typeof value.timeout_ms === 'number' ? value.timeout_ms : 1_200_000,
    maxSteps: typeof value.max_steps === 'number' ? value.max_steps : 64,
    verifyTls: value.verify_tls_certificates !== false,
  }
}

export default function RemoteProviderProfileManager({ client, compact = false }: Props) {
  const [open, setOpen] = useState(false)
  const [profiles, setProfiles] = useState<ProviderProfile[]>([])
  const [teams, setTeams] = useState<TeamRecord[]>([])
  const [editing, setEditing] = useState<ProviderProfile | null>(null)
  const [creating, setCreating] = useState(false)
  const [teamId, setTeamId] = useState('')
  const [name, setName] = useState('')
  const [baseUrl, setBaseUrl] = useState('https://api.openai.com/v1')
  const [model, setModel] = useState('gpt-5')
  const [authMode, setAuthMode] = useState<'none' | 'bearer'>('bearer')
  const [apiKey, setApiKey] = useState('')
  const [timeoutMs, setTimeoutMs] = useState(1_200_000)
  const [maxSteps, setMaxSteps] = useState(64)
  const [verifyTls, setVerifyTls] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const [nextProfiles, nextTeams] = await Promise.all([
        client.listProviderProfiles(), client.listTeams(),
      ])
      setProfiles(nextProfiles.filter((profile) => !profile.deleted_at))
      setTeams(nextTeams.filter((team) => !team.deleted_at))
      setError(null)
    } catch (cause) { setError(messageOf(cause)) }
  }, [client])
  useEffect(() => { if (open) void load() }, [load, open])

  const resetForm = () => {
    setEditing(null); setCreating(false); setTeamId(''); setName('')
    setBaseUrl('https://api.openai.com/v1'); setModel('gpt-5'); setAuthMode('bearer')
    setApiKey(''); setTimeoutMs(1_200_000); setMaxSteps(64); setVerifyTls(true)
  }
  const edit = (profile: ProviderProfile) => {
    const defaults = defaultsOf(profile)
    setEditing(profile); setCreating(true); setTeamId(profile.team_id ?? '')
    setName(profile.name); setBaseUrl(defaults.baseUrl); setModel(defaults.model)
    setAuthMode(defaults.authMode); setApiKey(''); setTimeoutMs(defaults.timeoutMs)
    setMaxSteps(defaults.maxSteps); setVerifyTls(defaults.verifyTls)
  }
  const save = async (event: FormEvent) => {
    event.preventDefault(); setBusy(true); setError(null)
    try {
      const modelDefaults = {
        base_url: baseUrl.trim(), model: model.trim(), auth_mode: authMode,
        timeout_ms: timeoutMs, max_steps: maxSteps, verify_tls_certificates: verifyTls,
      }
      if (editing) {
        let updated = await client.updateProviderProfile(editing.id, {
          expected_revision: editing.revision,
          name: name.trim(),
          provider_kind: 'openai_compatible',
          model_defaults: modelDefaults,
        })
        if (apiKey.trim()) {
          updated = await client.setProviderProfileSecret(updated.id, updated.revision, apiKey.trim())
        }
        setEditing(updated)
      } else {
        await client.createProviderProfile({
          team_id: teamId || null,
          name: name.trim(),
          provider_kind: 'openai_compatible',
          model_defaults: modelDefaults,
          api_key: apiKey.trim() || null,
        })
      }
      resetForm(); await load()
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const clearSecret = async (profile: ProviderProfile) => {
    setBusy(true); setError(null)
    try { await client.setProviderProfileSecret(profile.id, profile.revision, null); await load() }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const remove = async (profile: ProviderProfile) => {
    setBusy(true); setError(null)
    try { await client.deleteProviderProfile(profile.id, profile.revision); await load() }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }

  if (!open) return <button className={compact ? '' : 'ui-button ui-button--secondary ui-button--sm'} type="button" onClick={() => setOpen(true)}><Bot size={14} /> Models</button>
  return (
    <section className={`remote-management-panel remote-provider-manager${compact ? ' compact' : ''}`}>
      <header><div><Bot size={16} /><strong>Model profiles</strong></div><button type="button" aria-label="Close model profiles" onClick={() => setOpen(false)}><X size={15} /></button></header>
      <p className="remote-management-hint">Server-bound credentials stay encrypted on the control plane. Device-bound Ollama/vLLM endpoints remain configured only on that personal device.</p>
      <div className="remote-management-list">
        {profiles.length === 0 ? <p>No model profiles.</p> : profiles.map((profile) => {
          const binding = providerEndpointBinding(profile)
          return <article key={profile.id}><span><strong>{providerModelLabel(profile)}</strong><small>{binding === 'server' ? 'Server endpoint' : 'Personal-device endpoint'} · {profile.team_id ? `Team ${profile.team_id.slice(0, 8)}` : 'Personal'} · {profile.has_secret ? 'secret stored' : 'no secret'}</small></span><div>{binding === 'server' ? <button type="button" disabled={busy} onClick={() => edit(profile)}>Edit</button> : null}{profile.has_secret && binding === 'server' ? <button type="button" aria-label="Clear stored secret" disabled={busy} onClick={() => { void clearSecret(profile) }}><KeyRound size={14} /></button> : null}<button type="button" aria-label="Delete model profile" disabled={busy} onClick={() => { void remove(profile) }}><Trash2 size={14} /></button></div></article>
        })}
      </div>
      {creating ? <form onSubmit={save}>
        <label>Scope<select value={teamId} onChange={(event) => setTeamId(event.target.value)} disabled={Boolean(editing)}><option value="">Personal</option>{teams.map((team) => <option key={team.id} value={team.id}>Team · {team.name}</option>)}</select></label>
        <label>Name<input value={name} onChange={(event) => setName(event.target.value)} maxLength={200} required /></label>
        <label>Base URL<input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://api.example.com/v1" required /></label>
        <label>Model<input value={model} onChange={(event) => setModel(event.target.value)} required /></label>
        <label>Authentication<select value={authMode} onChange={(event) => setAuthMode(event.target.value as 'none' | 'bearer')}><option value="bearer">Bearer token</option><option value="none">None</option></select></label>
        {authMode === 'bearer' ? <label>{editing ? 'Replace API key (optional)' : 'API key'}<input type="password" value={apiKey} onChange={(event) => setApiKey(event.target.value)} autoComplete="new-password" required={!editing} /></label> : null}
        <label>Timeout (ms)<input type="number" min={1000} max={86_400_000} value={timeoutMs} onChange={(event) => setTimeoutMs(Number(event.target.value))} required /></label>
        <label>Maximum agent steps<input type="number" min={1} max={256} value={maxSteps} onChange={(event) => setMaxSteps(Number(event.target.value))} required /></label>
        <label className="remote-management-check"><input type="checkbox" checked={verifyTls} onChange={(event) => setVerifyTls(event.target.checked)} /> Verify TLS certificates</label>
        <div className="remote-management-actions"><button type="submit" disabled={busy || !name.trim() || !baseUrl.trim() || !model.trim()}><Save size={14} /> Save profile</button><button type="button" onClick={resetForm}>Cancel</button></div>
      </form> : <button className="remote-management-add" type="button" onClick={() => setCreating(true)}><Plus size={14} /> Add server profile</button>}
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
