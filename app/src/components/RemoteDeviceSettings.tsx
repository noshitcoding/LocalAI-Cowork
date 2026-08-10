import { Laptop, RefreshCw, Save, X } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'

import type {
  ExecutorRecord,
  PersonalDeviceRemoteControlMode,
} from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'

type Props = { client: RemoteRuntimeClient; compact?: boolean }

const MODES: Array<{ value: PersonalDeviceRemoteControlMode; label: string }> = [
  { value: 'confirm_each_session', label: 'Confirm on device' },
  { value: 'off', label: 'Remote access off' },
  { value: 'unattended', label: 'Unattended access' },
]

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function modeLabel(mode: string | undefined): string {
  return MODES.find((entry) => entry.value === mode)?.label ?? 'Unknown'
}

export default function RemoteDeviceSettings({ client, compact = false }: Props) {
  const [open, setOpen] = useState(false)
  const [devices, setDevices] = useState<ExecutorRecord[]>([])
  const [drafts, setDrafts] = useState<Record<string, PersonalDeviceRemoteControlMode>>({})
  const [busyId, setBusyId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const catalog = await client.capabilities()
      const personal = catalog.executors.filter((item) => item.registration.kind === 'personal_device')
      setDevices(personal)
      setDrafts(Object.fromEntries(personal.map((item) => [
        item.registration.executor_id,
        item.registration.personal_device_remote_control ?? 'confirm_each_session',
      ])))
      setError(null)
    } catch (cause) {
      setError(messageOf(cause))
    }
  }, [client])

  useEffect(() => { if (open) void load() }, [load, open])

  const save = async (device: ExecutorRecord) => {
    const id = device.registration.executor_id
    setBusyId(id)
    setError(null)
    try {
      const updated = await client.setPersonalDeviceRemoteControl(device, drafts[id])
      setDevices((current) => current.map((item) => (
        item.registration.executor_id === id ? updated : item
      )))
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusyId(null)
    }
  }

  if (!open) {
    return <button className={compact ? '' : 'ui-button ui-button--secondary ui-button--sm'} type="button" onClick={() => setOpen(true)}><Laptop size={14} /> Devices</button>
  }

  return (
    <section className={`remote-governance-panel remote-device-settings${compact ? ' compact' : ''}`}>
      <header><div><Laptop size={16} /><strong>Personal devices</strong></div><button type="button" aria-label="Close device settings" onClick={() => setOpen(false)}><X size={15} /></button></header>
      <div className="remote-section-header">
        <div><h2>Remote desktop policy</h2><p>The server policy is a ceiling. The local agent can remain stricter and never exposes its credential or model keys.</p></div>
        <button type="button" aria-label="Refresh personal devices" onClick={() => { void load() }} disabled={busyId !== null}><RefreshCw size={14} /></button>
      </div>
      {devices.length === 0 ? <p className="remote-muted">No personal devices are registered for this account.</p> : (
        <ul className="remote-device-list">
          {devices.map((device) => {
            const registration = device.registration
            const serverMode = registration.personal_device_remote_control ?? 'confirm_each_session'
            const draft = drafts[registration.executor_id] ?? serverMode
            const localMode = registration.labels.local_remote_control_mode
            return (
              <li key={registration.executor_id}>
                <div><strong>{registration.display_name}</strong><small>{device.online ? 'Online' : 'Offline'} · {registration.labels.os ?? 'unknown OS'}</small><small>Local enforcement: {modeLabel(localMode)}</small></div>
                <label>Server allowance<select value={draft} onChange={(event) => setDrafts((current) => ({ ...current, [registration.executor_id]: event.target.value as PersonalDeviceRemoteControlMode }))}>{MODES.map((mode) => <option key={mode.value} value={mode.value}>{mode.label}</option>)}</select></label>
                <button type="button" disabled={busyId !== null || draft === serverMode} onClick={() => { void save(device) }}><Save size={14} /> Save</button>
              </li>
            )
          })}
        </ul>
      )}
      <p className="remote-muted">To permit more access than the local enforcement shown here, change <code>COWORK_PERSONAL_REMOTE_CONTROL</code> on the device and restart its agent. A remote account cannot weaken that local setting.</p>
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
