import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useServerProfilesStore } from '../stores/serverProfilesStore'

export default function ServerProfiles({ serverUrl, email, onSelect }: {
  serverUrl: string; email: string; onSelect: (url: string, email: string) => void
}) {
  const { i18n } = useTranslation()
  const t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const { profiles, save, remove } = useServerProfilesStore()
  const [error, setError] = useState('')
  const [name, setName] = useState('')
  const selected = profiles.find(profile => profile.serverUrl === serverUrl.replace(/\/$/, '') && profile.email === email.trim().toLowerCase())
  return <fieldset className="server-profiles">
    <legend>{t('Saved servers', 'Gespeicherte Server')}</legend>
    <label>{t('Connection', 'Verbindung')}<select value={selected?.id ?? ''} onChange={event => {
      const profile = profiles.find(item => item.id === event.target.value)
      setError(''); setName(profile?.name ?? '')
      onSelect(profile?.serverUrl ?? '', profile?.email ?? '')
    }}><option value="">{t('New server', 'Neuer Server')}</option>{profiles.map(profile => <option key={profile.id} value={profile.id}>{profile.name} · {profile.email || profile.serverUrl}</option>)}</select></label>
    <details><summary>{t('Manage saved address', 'Gespeicherte Adresse verwalten')}</summary>
    <label>{t('Connection name (optional)', 'Verbindungsname (optional)')}<input maxLength={100} value={name} onChange={event => setName(event.target.value)} placeholder={t('Team server', 'Team-Server')} /></label>
    <div className="server-profile-actions">
      <button type="button" className="ui-button ui-button--secondary" disabled={!serverUrl} onClick={() => {
        try { save(serverUrl, email, name); setError('') } catch (cause) { setError(String(cause instanceof Error ? cause.message : cause)) }
      }}>{t('Save address', 'Adresse speichern')}</button>
      {selected && <button type="button" className="ui-button ui-button--ghost" onClick={() => { remove(selected.id); setName('') }}>{t('Remove saved address', 'Gespeicherte Adresse entfernen')}</button>}
    </div>
    {error && <p role="alert">{error}</p>}
    </details>
    <small>{t('Switching servers requires a new sign-in. No laptop access is granted by connecting.', 'Ein Serverwechsel erfordert eine neue Anmeldung. Die Verbindung erteilt keinen Laptop-Zugriff.')}</small>
  </fieldset>
}
