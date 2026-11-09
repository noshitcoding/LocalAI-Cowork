import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react'
import { useTranslation } from 'react-i18next'
import { ManagementClient, type OrganisationSsoSettings } from '../runtime/managementClient'
import { useRemoteRuntimeStore } from '../stores/remoteRuntimeStore'

export default function EnterpriseSso({ client,orgId,serverUrl }:{client:ManagementClient;orgId:string;serverUrl:string}) {
  const { i18n }=useTranslation()
  const t=(en:string,de:string)=>i18n.language.startsWith('de')?de:en
  const [settings,setSettings]=useState<OrganisationSsoSettings|null>(null)
  const [error,setError]=useState('')
  const [message,setMessage]=useState('')
  const [busy,setBusy]=useState(false)
  const [reload,setReload]=useState(0)
  const [ack,setAck]=useState(false)
  const [publishedScope,setPublishedScope]=useState<{orgId:string;serverUrl:string}|null>(null)
  const generation=useRef(0)
  const invalidatePending=useCallback(()=>{generation.current++},[])
  useEffect(()=>{
    invalidatePending()
    const abort=new AbortController()
    setSettings(null);setPublishedScope(null);setError('');setMessage('');setAck(false);setBusy(false)
    void client.sso(orgId,abort.signal).then(value=>{if(!abort.signal.aborted){setSettings(value);setPublishedScope(value.enabled&&value.available?{orgId,serverUrl}:null)}}).catch(reason=>{if(!abort.signal.aborted)setError(String(reason))})
    return ()=>{abort.abort();invalidatePending()}
  },[client,orgId,serverUrl,reload,invalidatePending])
  const save=async(event:FormEvent)=>{
    event.preventDefault()
    if(!settings || !ack || busy)return
    const current=generation.current
    setBusy(true);setError('');setMessage('')
    try {
      const next=await client.updateSso(orgId,{expected_revision:settings.revision,enabled:settings.enabled,provider_id:settings.provider_id,auto_provision:settings.auto_provision,group_mappings:settings.group_mappings})
      if(generation.current!==current)return
      setSettings(next);setPublishedScope(next.enabled&&next.available?{orgId,serverUrl}:null);setAck(false);setMessage(t('Saved. SSO-managed members must sign in again.','Gespeichert. Über SSO verwaltete Mitglieder müssen sich erneut anmelden.'))
    } catch(reason) {if(generation.current===current)setError(String(reason))} finally {if(generation.current===current)setBusy(false)}
  }
  const provider=settings?.providers.find(item=>item.id===settings.provider_id)
  const published=publishedScope?.orgId===orgId&&publishedScope.serverUrl===serverUrl
  return <section className="enterprise-settings-stack"><h2>{t('Organisation single sign-on','SSO der Organisation')}</h2>
    <div className="enterprise-notice"><div><h3>{t('Set up SSO after initial setup','SSO nach der Ersteinrichtung einrichten')}</h3><ol>
      <li>{t('Create the first local administrator account.','Erstelle zuerst das erste lokale Administratorkonto.')}</li>
      <li>{t('Finish authenticator setup and two-factor verification, then save the recovery codes.','Schließe die Authenticator-Einrichtung und Zwei-Faktor-Bestätigung ab und sichere die Wiederherstellungscodes.')}</li>
      <li>{t('An administrator can then configure and enable SSO here in the organisation settings. SSO is off by default.','Danach kann ein Administrator SSO hier in den Organisationseinstellungen konfigurieren und aktivieren. Standardmäßig ist SSO ausgeschaltet.')}</li>
    </ol></div></div>
    {published ? <><p>{t('Share this organisation-specific sign-in link after configuring SSO.','Teile nach der SSO-Einrichtung diesen organisationsspezifischen Anmeldelink.')}</p><label>{t('Sign-in link','Anmeldelink')}<input readOnly value={`${serverUrl.replace(/\/$/,'')}/server?organisation=${orgId}`} onFocus={e=>e.currentTarget.select()} /></label></> : <p>{t('The sign-in link appears here once an administrator has saved an enabled, available SSO configuration.','Der Anmeldelink erscheint hier, sobald ein Administrator eine aktivierte und verfügbare SSO-Konfiguration gespeichert hat.')}</p>}
    <label>{t('Organisation ID','Organisations-ID')}<input readOnly value={orgId} onFocus={e=>e.currentTarget.select()} /></label>
    {error && <p role="alert">{error}</p>}{message && <p role="status">{message}</p>}
    <button type="button" disabled={busy} onClick={()=>setReload(value=>value+1)}>{t('Reload SSO settings','SSO-Einstellungen neu laden')}</button>
    {!settings ? <p>{t('Loading…','Lädt…')}</p> : <form onSubmit={event=>void save(event)}><fieldset disabled={busy}>
      {settings.providers.length===0 && <p>{t('No provider has been approved for this organisation. The server operator must configure its identity provider and credentials first.','Für diese Organisation wurde noch kein Anbieter freigegeben. Der Serverbetreiber muss zuerst den Identitätsanbieter und dessen Zugangsdaten einrichten.')}</p>}
      {settings.enabled && !settings.available && <p role="status">{t('The saved provider is unavailable or has changed. Select an approved provider and save the settings again.','Der gespeicherte Anbieter ist nicht verfügbar oder wurde geändert. Wähle einen freigegebenen Anbieter und speichere die Einstellungen erneut.')}</p>}
      <label>{t('Identity provider','Identitätsanbieter')}<select value={settings.provider_id??''} onChange={e=>setSettings({...settings,provider_id:e.target.value||null})}>
        <option value="">{t('Select provider','Anbieter auswählen')}</option>{settings.provider_id && !provider && <option value={settings.provider_id} disabled>{settings.provider_id} ({t('unavailable','nicht verfügbar')})</option>}{settings.providers.map(item=><option key={item.id} value={item.id}>{item.label}</option>)}
      </select></label>
      {provider && <p>{provider.issuer} · {t('Group claim','Gruppen-Claim')}: {provider.groups_claim}</p>}
      <label className="enterprise-check"><input type="checkbox" checked={settings.enabled} disabled={!provider&&!settings.enabled} onChange={e=>setSettings({...settings,enabled:e.target.checked})} />{t('Enable organisation SSO','SSO für diese Organisation aktivieren')}</label>
      <label className="enterprise-check"><input type="checkbox" checked={settings.auto_provision} onChange={e=>setSettings({...settings,auto_provision:e.target.checked})} />{t('Create accounts and memberships for matching groups','Konten und Mitgliedschaften für passende Gruppen anlegen')}</label>
      <h3>{t('Group roles','Gruppenrollen')}</h3>
      <p>{t('Group names must match exactly. The strongest matching role applies. Manually assigned memberships stay unchanged. No group match suspends an SSO-managed membership.','Gruppennamen müssen exakt übereinstimmen. Die höchste passende Rolle gilt. Manuell zugewiesene Mitgliedschaften bleiben unverändert. Ohne passende Gruppe wird eine SSO-Mitgliedschaft gesperrt.')}</p>
      {settings.group_mappings.map((mapping,index)=><div key={index} className="enterprise-actions enterprise-sso-group">
        <label>{t('Group','Gruppe')} {index+1}<input required maxLength={256} value={mapping.group} onChange={e=>setSettings({...settings,group_mappings:settings.group_mappings.map((item,i)=>i===index?{...item,group:e.target.value}:item)})} /></label>
        <label>{t('Role','Rolle')} {index+1}<select value={mapping.role} onChange={e=>setSettings({...settings,group_mappings:settings.group_mappings.map((item,i)=>i===index?{...item,role:e.target.value as typeof mapping.role}:item)})}><option value="member">{t('Member','Mitglied')}</option><option value="admin">{t('Organisation administrator','Organisationsadministrator')}</option><option value="auditor">{t('Audit reader','Audit-Leser')}</option></select></label>
        <button type="button" aria-label={`${t('Remove group','Gruppe entfernen')} ${index+1}`} onClick={()=>setSettings({...settings,group_mappings:settings.group_mappings.filter((_,i)=>i!==index)})}>{t('Remove','Entfernen')}</button>
      </div>)}
      <button type="button" disabled={settings.group_mappings.length>=100} onClick={()=>setSettings({...settings,group_mappings:[...settings.group_mappings,{group:'',role:'member'}]})}>{t('Add group','Gruppe hinzufügen')}</button>
      <p>{t('Existing accounts must link SSO while signed in. Equal email addresses never link accounts automatically.','Bestehende Konten müssen SSO im angemeldeten Zustand verknüpfen. Gleiche E-Mail-Adressen verknüpfen keine Konten automatisch.')}</p>
      {published && <button type="button" onClick={()=>{setBusy(true);setError('');void useRemoteRuntimeStore.getState().linkOidc(orgId).catch(reason=>setError(String(reason))).finally(()=>setBusy(false))}}>{t('Link my account to this SSO provider','Mein Konto mit diesem SSO-Anbieter verknüpfen')}</button>}
      <label className="enterprise-check"><input type="checkbox" checked={ack} onChange={e=>setAck(e.target.checked)} />{t('I understand that saving suspends all SSO-managed memberships until they sign in again. A manual administrator must remain.','Mir ist bewusst, dass Speichern alle SSO-Mitgliedschaften bis zur erneuten Anmeldung sperrt. Ein manuell zugewiesener Administrator muss erhalten bleiben.')}</label>
      <button disabled={!ack || (settings.enabled && !provider)}>{t('Save SSO settings','SSO-Einstellungen speichern')}</button>
    </fieldset></form>}
  </section>
}
