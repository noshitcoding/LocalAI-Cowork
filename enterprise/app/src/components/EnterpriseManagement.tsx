import { useCallback, useEffect, useMemo, useState, type FormEvent } from 'react'

import { z } from 'zod'
import { Building2, CircleHelp } from 'lucide-react'
import { useSearchParams } from 'react-router-dom'

import { useTranslation } from 'react-i18next'

import { useRemoteRuntimeStore } from '../stores/remoteRuntimeStore'

import { ManagementClient, type EnterprisePolicy, type InstanceAction, type InstanceBackup, type InstanceRelease, type ProvisioningRelease, type InstanceRollback, type ManagedInstance, type Organisation } from '../runtime/managementClient'
import RemoteServerView from './RemoteServerView'
import EnterpriseRollouts from './EnterpriseRollouts'
import EnterpriseInstanceMembers from './EnterpriseInstanceMembers'
import EnterpriseInvitations from './EnterpriseInvitations'
import EnterpriseSso from './EnterpriseSso'
import EnterpriseDirectorySync from './EnterpriseDirectorySync'
import EnterpriseMfaPolicy from './EnterpriseMfaPolicy'
import EnterpriseCapacity from './EnterpriseCapacity'
import EnterpriseEnrollmentReview from './EnterpriseEnrollmentReview'
import EnterpriseInstanceImports from './EnterpriseInstanceImports'
import EnterpriseContentSharing from './EnterpriseContentSharing'
import EnterpriseDiskQuotas from './EnterpriseDiskQuotas'
import EnterpriseConsoleLayout from './EnterpriseConsoleLayout'
import { managementAreas, type ManagementArea } from '../runtime/managementAreas'
import EnterpriseOverview from './EnterpriseOverview'
import EnterpriseOrganisationSettings from './EnterpriseOrganisationSettings'
import EnterpriseRuntimeArea from './EnterpriseRuntimeArea'
import EnterpriseInstanceCard from './EnterpriseInstanceCard'
import EnterpriseResourceCatalog from './EnterpriseResourceCatalog'
import EnterpriseMemberResources from './EnterpriseMemberResources'
import EnterpriseMemberAccess from './EnterpriseMemberAccess'
import EnterprisePolicyEditor from './EnterprisePolicyEditor'
import EnterpriseWallet from './EnterpriseWallet'
import { legacyManagementCapabilities, readManagementCapabilities } from '../runtime/managementCapabilities'

import './EnterpriseManagement.css'



const recordList = z.array(z.record(z.string(), z.unknown()))
const capacityRecovery = (row:Record<string,unknown>) => {
  const args=row.arguments as Record<string,unknown>|undefined
  return Array.isArray(args?.reconcile_reservation_ids) && args.reconcile_reservation_ids.length>0
}

type Tab = ManagementArea
const instanceAreas: readonly ManagementArea[] = ['instances', 'environments', 'sharing', 'imports', 'members']



export default function EnterpriseManagement() {

  const account = useRemoteRuntimeStore()

  return <RemoteServerView headingLevel={1} managementContent={<EnterpriseConsole key={`${account.serverUrl}:${account.userId}`} />} />

}



function EnterpriseConsole() {

  const { i18n } = useTranslation()

  const de = i18n.language.startsWith('de')

  const t = (en: string, german: string) => de ? german : en

  const account = useRemoteRuntimeStore()

  const client = useMemo(() => account.serverUrl ? new ManagementClient(account.serverUrl) : null, [account.serverUrl])

  const [orgs, setOrgs] = useState<Organisation[]>([])

  const [searchParams,setSearchParams] = useSearchParams()
  const [orgId, setOrgId] = useState(searchParams.get('organisation') ?? '')

  const [platformAdmin, setPlatformAdmin] = useState(false)
  const [admissionEnabled,setAdmissionEnabled]=useState(false)
  const [releaseSelectionEnabled,setReleaseSelectionEnabled]=useState(false)
  const [importsEnabled, setImportsEnabled] = useState(false)
  const [creationCatalog,setCreationCatalog]=useState<{orgId:string;items:ProvisioningRelease[]}|null>(null)
  const [creationRelease,setCreationRelease]=useState('')
  const [creationCatalogError,setCreationCatalogError]=useState('')

  const [instances, setInstances] = useState<ManagedInstance[]>([])

  const [tab, setTab] = useState<Tab>(managementAreas.includes(searchParams.get('view') as Tab) ? searchParams.get('view') as Tab : searchParams.has('organisation') ? 'instances' : 'overview')
  const needsInstances = instanceAreas.includes(tab)
  useEffect(() => {
    setSearchParams(previous => {
      const next = new URLSearchParams(previous)
      if (orgId) next.set('organisation', orgId); else next.delete('organisation')
      next.set('view', tab)
      return next.toString() === previous.toString() ? previous : next
    }, { replace: true })
  }, [orgId, tab, setSearchParams])
  const [capabilities, setCapabilities] = useState(legacyManagementCapabilities)
  const [instanceQuery, setInstanceQuery] = useState('')
  const [instanceState, setInstanceState] = useState('all')
  const [reload, setReload] = useState(0)
  const [resourceMember, setResourceMember] = useState<{ user_id: string; display_name: string; email: string } | null>(null)
  const [walletMemberId, setWalletMemberId] = useState('')
  const [accessMember, setAccessMember] = useState<{ user_id: string; display_name: string; email: string } | null>(null)
  useEffect(() => { setResourceMember(null); setAccessMember(null); setWalletMemberId('') }, [orgId])
  useEffect(() => {
    if (!client || account.status !== 'authenticated') return
    const abort = new AbortController()
    void readManagementCapabilities(client, abort.signal).then(value => { if (!abort.signal.aborted) setCapabilities(value) }).catch(() => { if (!abort.signal.aborted) setCapabilities(legacyManagementCapabilities) })
    return () => abort.abort()
  }, [client, account.status, reload])

  const [newOrg, setNewOrg] = useState('')

  const [newInstance, setNewInstance] = useState('')

  const [backend, setBackend] = useState<'compose' | 'kubernetes'>('compose')

  const [policy, setPolicy] = useState<EnterprisePolicy | null>(null)

  const [rows, setRows] = useState<Record<string, unknown>[]>([])

  const [operationsFor, setOperationsFor] = useState<ManagedInstance | null>(null)
  const [environmentsFor,setEnvironmentsFor]=useState('')
  const [diskQuotasFor,setDiskQuotasFor]=useState('')

  const [email, setEmail] = useState('')

  const [role, setRole] = useState('member')
  const [contentAdmin, setContentAdmin] = useState(false)
  useEffect(() => { setContentAdmin(false); setEmail(''); setRole('member') }, [orgId])

  const [error, setError] = useState('')

  const [connectionError, setConnectionError] = useState('')

  const [busy, setBusy] = useState(false)

  const [pending, setPending] = useState<{ instance: ManagedInstance; action: InstanceAction } | null>(null)

  const [argument, setArgument] = useState('')
  const [purgeBackups, setPurgeBackups] = useState(false)
  const [enrollmentAcknowledged,setEnrollmentAcknowledged]=useState(false)
  const enrollmentInvalid=pending?.action==='enroll_execution' && !enrollmentAcknowledged
  const deletionInvalid = pending?.action === 'delete' && (argument !== pending.instance.name || !purgeBackups)

  const [recovery, setRecovery] = useState<{ instance: ManagedInstance; operationId: string; deletion: boolean; enrollment:boolean } | null>(null)

  const [membersInstanceId, setMembersInstanceId] = useState('')
  const membersInstance = instances.find(value => value.id === membersInstanceId && value.organisation_id === orgId)
  useEffect(() => { setMembersInstanceId('') }, [orgId])

  const [backupInstance, setBackupInstance] = useState<ManagedInstance | null>(null)

  const [backups, setBackups] = useState<InstanceBackup[]>([])

  const [backupsLoading, setBackupsLoading] = useState(false)
  const [releases, setReleases] = useState<InstanceRelease[]>([])
  const [rollbacks, setRollbacks] = useState<InstanceRollback[]>([])
  const [rollbacksLoading, setRollbacksLoading] = useState(false)
  const [rollbackAcknowledged, setRollbackAcknowledged] = useState(false)
  const rollbackInvalid = pending?.action === 'rollback' && (rollbacksLoading || !rollbackAcknowledged || !rollbacks.some(value => value.operation_id === argument && value.source_release === pending.instance.current_release))
  const [releasesLoading, setReleasesLoading] = useState(false)
  const [qualificationOnly, setQualificationOnly] = useState(true)
  const selectedRelease = releases.find(value => value.id === argument && value.source_release === pending?.instance.current_release)
  const updateInvalid = pending?.action === 'upgrade' && (releasesLoading || !selectedRelease || (!qualificationOnly && !selectedRelease.qualified))
  const org = orgs.find(value => value.id === orgId)

  const admin = platformAdmin || org?.role === 'admin'
  const choosingRelease = releaseSelectionEnabled && backend === 'compose'
  const creationInvalid = choosingRelease && (org?.policy.minimum_isolation !== 'container' || creationCatalog?.orgId !== orgId || !creationCatalog.items.some(item => item.id === creationRelease))

  useEffect(() => {
    setCreationCatalog(null); setCreationRelease(''); setCreationCatalogError('')
    if (!client || !orgId || tab !== 'instances' || org?.role === 'auditor' || !choosingRelease) return
    const abort=new AbortController()
    void client.provisioningReleases(orgId,abort.signal)
      .then(items => {if (!abort.signal.aborted) setCreationCatalog({orgId,items})})
      .catch(reason => {if (!abort.signal.aborted) setCreationCatalogError(String(reason))})
    return () => abort.abort()
  },[client,orgId,org?.role,tab,choosingRelease,reload])

  useEffect(() => {
    setRollbacks([])
    if (!client || pending?.action !== 'rollback' || pending.instance.organisation_id !== orgId) { setRollbacksLoading(false); return }
    const abort = new AbortController()
    setRollbacksLoading(true)
    void client.rollbacks(pending.instance, abort.signal)
      .then(value => { if (!abort.signal.aborted) setRollbacks(value.filter(point => point.source_release === pending.instance.current_release)) })
      .catch(cause => { if (!abort.signal.aborted) setError(String(cause)) })
      .finally(() => { if (!abort.signal.aborted) setRollbacksLoading(false) })
    return () => abort.abort()
  }, [client, pending, orgId, reload])

  const pendingChanged = !!pending && !instances.some(value => value.id === pending.instance.id && value.revision === pending.instance.revision && value.available_actions?.includes(pending.action))

  useEffect(() => {
    setReleases([])
    if (!client || pending?.action !== 'upgrade' || pending.instance.organisation_id !== orgId) { setReleasesLoading(false); return }
    const abort = new AbortController()
    setReleasesLoading(true)
    void client.releases(pending.instance, abort.signal)
      .then(value => { if (!abort.signal.aborted) setReleases(value.filter(release => release.source_release === pending.instance.current_release)) })
      .catch(reason => { if (!abort.signal.aborted) setError(String(reason)) })
      .finally(() => { if (!abort.signal.aborted) setReleasesLoading(false) })
    return () => abort.abort()
  }, [client, pending, orgId, reload])


  useEffect(() => {

    if (!client || account.status !== 'authenticated') return

    const abort = new AbortController()

    void client.organisations(abort.signal).then(result => {

      if (abort.signal.aborted) return

      setOrgs(result.items); setPlatformAdmin(result.platform_admin);setAdmissionEnabled(result.execution_admission_enabled===true);setReleaseSelectionEnabled(result.release_selection_enabled===true); setImportsEnabled(result.instance_imports_enabled===true)

      setOrgId(current => result.items.some(item => item.id === current) ? current : result.items[0]?.id ?? '')

    }).catch(reason => { if (!abort.signal.aborted) setError(String(reason)) })

    return () => abort.abort()

  }, [client, account.status, reload])



  useEffect(() => {

    setInstances([]); setRows([]); setPending(null); setRecovery(null); setArgument(''); setBackupInstance(null); setConnectionError('')

    if (!client || !orgId || account.status !== 'authenticated') return

    const abort = new AbortController()

    if (needsInstances) void client.instances(orgId, abort.signal)
      .then(value => { if (!abort.signal.aborted) { setInstances(value); setConnectionError(''); setOperationsFor(current => current && !value.some(item => item.id === current.id) ? null : current); setBackupInstance(current => current && !value.some(item => item.id === current.id) ? null : current) } })
      .catch(reason => { if (!abort.signal.aborted) setConnectionError(String(reason)) })
    if (tab === 'members' || tab === 'audit') void client.request(`/organisations/${orgId}/${tab}`, recordList, undefined, 'GET', abort.signal)
      .then(value => { if (!abort.signal.aborted) setRows(value) })
      .catch(reason => { if (!abort.signal.aborted) setError(String(reason)) })

    return () => abort.abort()

  }, [client, orgId, tab, needsInstances, reload, account.status])



  useEffect(() => { setPolicy(org?.policy ? structuredClone(org.policy) : null) }, [org])

  useEffect(() => { setOperationsFor(null) }, [orgId, tab])



  useEffect(() => {

    if (!client || !operationsFor || operationsFor.organisation_id !== orgId || tab !== 'instances') { if (tab === 'instances') setRows([]); return }

    const abort = new AbortController()

    const load = () => { void client.request(`/organisations/${orgId}/instances/${operationsFor.id}/operations`, recordList, undefined, 'GET', abort.signal)

      .then(value => { if (!abort.signal.aborted) setRows(value) })

      .catch(async reason => {
        if (abort.signal.aborted) return
        // Completed deletion removes this endpoint. Confirm absence from an
        // authoritative listing before treating a lost response as unavailable.
        try {
          const visible = await client.instances(orgId, abort.signal)
          if (!abort.signal.aborted && !visible.some(value => value.id === operationsFor.id)) {
            setOperationsFor(null); setRows([]); return
          }
        } catch { /* Preserve the original operation error if the check fails. */ }
        if (!abort.signal.aborted) setError(String(reason))
      }) }

    load()

    const timer = window.setInterval(load, 3000)

    return () => { abort.abort(); window.clearInterval(timer) }

  }, [client, operationsFor, orgId, tab, reload])



  useEffect(() => {

    setBackups([])

    if (!client || !backupInstance || backupInstance.organisation_id !== orgId) { setBackupsLoading(false); return }

    const abort = new AbortController()

    setBackupsLoading(true)

    void client.backups(backupInstance, abort.signal).then(value => { if (!abort.signal.aborted) setBackups(value.filter(backup => backup.instance_id === backupInstance.id && backup.organisation_id === orgId)) })

      .catch(reason => { if (!abort.signal.aborted) setError(String(reason)) })

      .finally(() => { if (!abort.signal.aborted) setBackupsLoading(false) })

    return () => abort.abort()

  }, [client, backupInstance, orgId, reload])



  useEffect(() => {

    if (!client || !orgId || tab !== 'instances') return

    const abort = new AbortController()

    const timer = window.setInterval(() => {

      void client.instances(orgId, abort.signal).then(value => { if (!abort.signal.aborted) { setInstances(value); setConnectionError(''); setOperationsFor(current => current && !value.some(item => item.id === current.id) ? null : current); setBackupInstance(current => current && !value.some(item => item.id === current.id) ? null : current) } })

        .catch(reason => { if (!abort.signal.aborted) setConnectionError(String(reason)) })

    }, 3000)

    return () => { abort.abort(); window.clearInterval(timer) }

  }, [client, orgId, tab])



  const run = useCallback(async (operation: () => Promise<unknown>, refresh = true) => {

    setBusy(true); setError('')

    try { await operation(); if (refresh) setReload(value => value + 1) } catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)) }

    finally { setBusy(false) }

  }, [])

  const createOrganisation = (event: FormEvent) => { event.preventDefault(); if (client) void run(async () => { const result = await client.createOrganisation(newOrg); setOrgId(result.id); setNewOrg('') }) }

  const createInstance = (event: FormEvent) => { event.preventDefault(); if (client && !creationInvalid) void run(async () => {

    const created = await client.createInstance(orgId, newInstance, backend, choosingRelease ? creationRelease : undefined)

    setNewInstance('')

    const updated = await client.instances(orgId)

    setInstances(updated)

    const instance = updated.find(value => value.id === created.id)

    if (instance?.available_actions?.includes('provision')) await client.action(instance, 'provision')

  }) }

  const openInstance = (instance: ManagedInstance) => void run(async () => {

    const launch = await client!.launch(instance)

    if (import.meta.env.MODE === 'web' || import.meta.env.VITE_COWORK_WEB === 'true') {

      window.location.assign(`${launch.url}/server#${new URLSearchParams({ fleet: launch.ticket, email: launch.email })}`)

    } else {

      await account.loginFleet(launch.url, launch.email, launch.ticket)

    }

  }, false)

  const labels: Record<InstanceAction, string> = { provision: t('Provision', 'Bereitstellen'), start: t('Start', 'Starten'), stop: t('Stop', 'Stoppen'), suspend: t('Suspend', 'Sperren'), resume: t('Unlock', 'Entsperren'), backup: t('Back up', 'Sichern'), restore: t('Restore', 'Wiederherstellen'), upgrade: t('Upgrade', 'Aktualisieren'), rollback: t('Roll back update', 'Update zurücksetzen'), enroll_execution:t('Enroll in central limits','Zentrale Limits einrichten'), delete: t('Delete', 'Löschen') }

  const selectAction = (instance: ManagedInstance, action: InstanceAction) => {

    setPending({ instance, action }); setArgument(''); setQualificationOnly(true); setRollbackAcknowledged(false); setPurgeBackups(false);setEnrollmentAcknowledged(false)
    if (action === 'restore') setBackupInstance(instance)

  }



  if (account.status !== 'authenticated' || !client) return <RemoteServerView headingLevel={1} />

  const chooseOrganisation = (id: string) => { setOrgId(id); setSearchParams(previous => { const next = new URLSearchParams(previous); if (id) next.set('organisation', id); else next.delete('organisation'); return next }); setError(''); setConnectionError(''); setTab('instances'); setInstanceQuery(''); setInstanceState('all') }
  const visibleInstances = instances.filter(instance => instance.name.toLocaleLowerCase().includes(instanceQuery.toLocaleLowerCase()) && (instanceState === 'all' || instance.state === instanceState))
  return <EnterpriseConsoleLayout area={tab} onArea={area => { setTab(area); setError(''); setConnectionError('') }} organisations={orgs} organisationId={orgId} onOrganisation={chooseOrganisation} platformAdmin={platformAdmin} busy={busy} serverUrl={account.serverUrl} email={account.email} admissionEnabled={admissionEnabled} importsEnabled={importsEnabled} onRefresh={() => setReload(value => value + 1)} onLogout={() => void account.logout()}>

    {error && <p role="alert" className="enterprise-error">{error}</p>}

    {needsInstances && connectionError && <p role="alert" className="enterprise-error">{tab === 'instances' ? t('Connection interrupted. Retrying automatically.', 'Verbindung unterbrochen. Der Verbindungsaufbau wird automatisch wiederholt.') : t('Instance list could not be loaded. Refresh to retry.', 'Die Instanzliste konnte nicht geladen werden. Bitte zum erneuten Versuch aktualisieren.')} {connectionError}</p>}

    {tab === 'overview' && <EnterpriseOverview key={reload} client={client} organisations={orgs} serverUrl={account.serverUrl} onOrganisation={chooseOrganisation} />}
    {tab === 'catalog' && platformAdmin && <EnterpriseResourceCatalog client={client} enabled={capabilities.resource_catalog} />}
    {tab === 'organisations' && <><section><h2>{t('Your organisations', 'Deine Organisationen')}</h2><p>{t('An organisation holds members and common rules. Its instances are independent Cowork servers; teams are configured inside their app.', 'Eine Organisation enthält Mitglieder und gemeinsame Regeln. Ihre Instanzen sind eigenständige Cowork-Server; Teams werden in deren App eingerichtet.')}</p><div className="enterprise-grid">{orgs.map(item => <article key={item.id}><Building2 size={22} aria-hidden="true" /><h3>{item.name}</h3><p className="enterprise-muted">{item.role === 'admin' ? t('Organisation administrator', 'Organisationsadministrator') : item.role === 'auditor' ? t('Audit reader', 'Audit-Leser') : t('Member', 'Mitglied')}</p><button type="button" onClick={() => { chooseOrganisation(item.id); setTab('organisations') }}>{t('Organisation settings', 'Organisationseinstellungen')}</button><button type="button" onClick={() => chooseOrganisation(item.id)}>{t('View instances', 'Instanzen anzeigen')}</button></article>)}</div>{platformAdmin && <form onSubmit={createOrganisation}><h3>{t('Create an organisation', 'Organisation erstellen')}</h3><label>{t('New organisation', 'Neue Organisation')}<input required maxLength={120} value={newOrg} onChange={event => setNewOrg(event.target.value)} /></label><button disabled={busy}>{t('Create organisation', 'Organisation erstellen')}</button></form>}</section>{org && <EnterpriseOrganisationSettings key={org.id} client={client} org={org} enabled={capabilities.organisation_settings} canArchive={capabilities.organisation_archive} canManage={admin} onChanged={archived => { if (archived) setOrgId(''); setReload(value => value + 1) }} />}</>}
    {!org && !['overview', 'organisations', 'catalog'].includes(tab) && <div className="enterprise-empty"><Building2 size={28} aria-hidden="true" /><h2>{t('Start with an organisation', 'Beginne mit einer Organisation')}</h2><p>{t('Create an organisation or ask your administrator for membership.', 'Erstelle eine Organisation oder lasse dir eine Mitgliedschaft zuweisen.')}</p><button type="button" className="enterprise-primary" onClick={() => setTab('organisations')}>{t('Manage organisations', 'Organisationen verwalten')}</button></div>}

    {org && <>

      {tab === 'wallet' && <EnterpriseWallet key={`${orgId}:${account.userId}`} client={client} orgId={orgId} canManage={admin} enabled={capabilities.wallets} initialUserId={walletMemberId} />}
      {tab === 'environments' && <EnterpriseRuntimeArea key={orgId} client={client} instances={instances} admin={admin} enabled={capabilities.environments} accessEnabled={capabilities.environment_access} initialInstanceId={environmentsFor} />}
      {tab==='sharing'&&capabilities.schema_version>=1&&account.userId&&['admin','member'].includes(org.role)&&<EnterpriseContentSharing key={`${account.serverUrl}:${account.userId}:${orgId}`} client={client} orgId={orgId} serverUrl={account.serverUrl} userId={account.userId} instances={instances} onOpen={openInstance}/>}
      {tab === 'imports' && importsEnabled && admin && account.userId && <EnterpriseInstanceImports key={`${account.serverUrl}:${account.userId}:${orgId}`} client={client} orgId={orgId} serverUrl={account.serverUrl} userId={account.userId} instances={instances} canManage={admin} onChanged={() => setReload(value => value + 1)} onOpen={openInstance} />}
      {tab==='capacity' && admissionEnabled && (admin || org.role==='auditor') && <EnterpriseCapacity key={orgId} client={client} orgId={orgId} canManage={admin}/>}

      {tab==='mfa'&&admin&&(capabilities.organisation_mfa ? <EnterpriseMfaPolicy key={`${account.serverUrl}:${account.userId}:${orgId}`} client={client} orgId={orgId} canManage={admin}/> : <section><p>{t('Organisation MFA settings require a newer control server. Platform administrators continue to require MFA.', 'MFA-Einstellungen pro Organisation benötigen einen neueren Verwaltungsserver. Für Plattformadministratoren bleibt MFA erforderlich.')}</p></section>)}
      {tab === 'sso' && admin && <EnterpriseSso key={`${account.serverUrl}:${orgId}`} client={client} orgId={orgId} serverUrl={account.serverUrl} />}
      {tab === 'directory' && admin && (capabilities.directory ? <EnterpriseDirectorySync key={`${account.serverUrl}:${orgId}`} client={client} orgId={orgId} serverUrl={account.serverUrl} /> : <section><p>{t('Directory synchronisation is unavailable on this server version.', 'Verzeichnissynchronisierung ist auf dieser Serverversion nicht verfügbar.')}</p></section>)}
      {tab === 'rollouts' && (admin || org.role === 'auditor') && <EnterpriseRollouts key={orgId} client={client} orgId={orgId} canManage={admin} />}
      {tab === 'instances' && <section><h2>{t('Cowork instances', 'Cowork-Instanzen')}</h2>
        <div className="enterprise-notice"><CircleHelp size={18} aria-hidden="true" /><p>{t('Each card is a separate Cowork server in this organisation. Open instance enters its app. Instance members controls who can sign in; runtime environments provide its isolated execution workspaces.', 'Jede Karte ist ein eigener Cowork-Server dieser Organisation. Instanz öffnen führt in seine App. Instanzmitglieder legt fest, wer sich anmelden darf; Laufzeitumgebungen stellen isolierte Ausführungsbereiche bereit.')}</p></div>

        {org.role !== 'auditor' && <details className="enterprise-create-instance" open={!instances.length}><summary>{t('Create a new instance', 'Neue Instanz erstellen')}</summary><form onSubmit={createInstance}><label>{t('Instance name', 'Instanzname')}<input required maxLength={120} value={newInstance} onChange={event => setNewInstance(event.target.value)} /></label><label>{t('Deployment', 'Bereitstellung')}<select value={backend} onChange={event => setBackend(event.target.value as typeof backend)}><option value="compose">Docker Compose</option><option value="kubernetes">Kubernetes</option></select></label>{choosingRelease && <label>{t('Initial version','Anfangsversion')}<select aria-label={t('Initial version','Anfangsversion')} required value={creationRelease} disabled={!creationCatalog || creationCatalog.orgId!==orgId || busy} onChange={event=>setCreationRelease(event.target.value)}><option value="">{t('Select version','Version auswählen')}</option>{creationCatalog?.orgId===orgId && creationCatalog.items.map(item=><option key={item.id} value={item.id}>{item.name} ({item.id.slice(0,12)})</option>)}</select></label>}<button disabled={busy || creationInvalid}>{t('Create instance', 'Instanz erstellen')}</button>{choosingRelease && <p>{creationCatalogError || (creationCatalog?.orgId===orgId && !creationCatalog.items.length ? t('No approved initial version is available. Refresh after the operator publishes a version.','Keine freigegebene Anfangsversion verfügbar. Nach der Veröffentlichung durch den Betreiber bitte aktualisieren.') : t('The selected version stays fixed for provisioning. Later changes require an instance update.','Die gewählte Version bleibt für die Bereitstellung festgelegt. Spätere Änderungen erfolgen über ein Instanz-Update.'))}</p>}</form></details>}

        {instances.length === 0 && <p>{t('No instances yet.', 'Noch keine Instanzen vorhanden.')}</p>}
        {!!instances.length && <div className="enterprise-instance-filters"><label>{t('Find an instance', 'Instanz suchen')}<input type="search" value={instanceQuery} onChange={event => setInstanceQuery(event.target.value)} placeholder={t('Search by server name', 'Nach Servernamen suchen')} /></label><label>{t('State', 'Status')}<select value={instanceState} onChange={event => setInstanceState(event.target.value)}><option value="all">{t('All states', 'Alle Zustände')}</option>{Array.from(new Set(instances.map(instance => instance.state))).map(state => <option key={state} value={state}>{state}</option>)}</select></label></div>}

        {client && admin && account.userId && instances.filter(instance=>instance.organisation_id===orgId&&instance.id===diskQuotasFor).map(instance=><EnterpriseDiskQuotas key={`${account.serverUrl}:${account.userId}:${orgId}:${instance.id}`} client={client} instance={instance} admin={admin} serverUrl={account.serverUrl} userId={account.userId!} onClose={()=>setDiskQuotasFor('')} />)}

        {client && membersInstance && <EnterpriseInstanceMembers key={membersInstance.id} client={client} instance={membersInstance} onClose={() => setMembersInstanceId('')} onChanged={() => setReload(value => value + 1)} />}<div className="enterprise-instances">{visibleInstances.map(instance => <EnterpriseInstanceCard key={instance.id} instance={instance} admin={admin} busy={busy} labels={labels} environmentsEnabled={capabilities.environments} onOpen={() => openInstance(instance)} onAction={action => selectAction(instance, action)} onMembers={() => setMembersInstanceId(instance.id)} onStorage={() => setDiskQuotasFor(instance.id)} storageEnabled={capabilities.schema_version>=1} onBackups={() => setBackupInstance(instance)} onOperations={() => { setRows([]); setOperationsFor(instance) }} onEnvironments={() => { setEnvironmentsFor(instance.id); setTab('environments') }} />)}</div>
        {!!instances.length && !visibleInstances.length && <p>{t('No instances match your filters.', 'Keine Instanz entspricht deiner Suche.')}</p>}
        {instances.some(instance => instance.execution_required) && !admissionEnabled && <p>{t('Enrolled; waiting for platform activation of central task limits.','Übernommen; die Aktivierung zentraler Aufgabenlimits für die Plattform steht noch aus.')}</p>}

        {backupInstance && <section aria-label={t('Instance backups', 'Instanzsicherungen')}><h3>{t('Backups', 'Sicherungen')}: {backupInstance.name}</h3>

          <p>{t('Stop the instance before backing up or restoring. A restore keeps the instance stopped, signs users out and pauses unfinished tasks and schedules for review.', 'Stoppe die Instanz vor einer Sicherung oder Wiederherstellung. Nach einer Wiederherstellung bleibt sie gestoppt; Nutzer werden abgemeldet und unfertige Aufgaben sowie Zeitpläne zur Prüfung angehalten.')}</p>

          {backupsLoading ? <p role="status">{t('Loading backups…', 'Sicherungen werden geladen…')}</p> : backups.length === 0 ? <p>{t('No verified backups available.', 'Keine geprüften Sicherungen vorhanden.')}</p> : <ul>{backups.map(backup => <li key={backup.id}><time dateTime={backup.created_at}>{new Date(backup.created_at).toLocaleString(i18n.language)}</time> · {(backup.size_bytes / 1024 / 1024).toFixed(2)} MiB · <code>{backup.id}</code></li>)}</ul>}

        </section>}

        {pending && <form className="enterprise-confirm" onSubmit={event => { event.preventDefault(); if (busy || pendingChanged || updateInvalid || rollbackInvalid || deletionInvalid || enrollmentInvalid) return; if (pending.action === 'restore' && !backups.some(value => value.id === argument && value.instance_id === pending.instance.id && value.organisation_id === orgId)) return; void run(async () => { await client.action(pending.instance, pending.action, pending.action === 'enroll_execution' ? {acknowledge_execution_enrollment:enrollmentAcknowledged} : pending.action === 'delete' ? { confirmation_name: argument, purge_backups: purgeBackups } : pending.action === 'rollback' ? { rollback_operation_id: argument } : pending.action === 'restore' ? { backup_id: argument } : pending.action === 'upgrade' ? { release: argument, qualification_only: qualificationOnly } : {}); setOperationsFor(pending.instance); setPending(null) }) }}><h3>{labels[pending.action]}: {pending.instance.name}</h3><p>{pending.action === 'delete' ? t('This permanently deletes all managed instance data, earlier restored versions, workspaces, encryption keys and managed backups. Central audit records remain. Operator exports outside the managed backup store are not removed.', 'Dieser Vorgang löscht sämtliche verwalteten Instanzdaten, frühere Wiederherstellungsstände, Arbeitsverzeichnisse, Schlüssel und verwaltete Sicherungen endgültig. Zentrale Audit-Einträge bleiben erhalten. Exporte außerhalb des verwalteten Sicherungsspeichers werden nicht entfernt.') : pending.action === 'restore' ? t('Changes made after the selected backup will no longer be active. Restore requires the same qualified release.', 'Änderungen nach der gewählten Sicherung sind anschließend nicht mehr aktiv. Die Wiederherstellung erfordert dieselbe freigegebene Version.') : t('The operation is queued and its actual status is reported by the provisioner.', 'Der Vorgang wird eingereiht; sein tatsächlicher Zustand wird von der Bereitstellung gemeldet.')}</p>

          {pending.action === 'enroll_execution' && <EnterpriseEnrollmentReview checked={enrollmentAcknowledged} onChange={setEnrollmentAcknowledged} />}
          {pending.action === 'delete' && <><label>{t('Type the instance name', 'Instanznamen eingeben')}<input required autoComplete="off" value={argument} onChange={event => setArgument(event.target.value)} /></label><label><input type="checkbox" checked={purgeBackups} onChange={event => setPurgeBackups(event.target.checked)} />{t('Permanently delete all managed backups as well.', 'Auch alle verwalteten Sicherungen endgültig löschen.')}</label></>}

          {pending.action === 'restore' && <label>{t('Backup', 'Sicherung')}<select required value={argument} disabled={backupsLoading} onChange={event => setArgument(event.target.value)}><option value="">{t('Select a verified backup', 'Geprüfte Sicherung auswählen')}</option>{backups.filter(value => value.instance_id === pending.instance.id && value.organisation_id === orgId).map(value => <option key={value.id} value={value.id}>{new Date(value.created_at).toLocaleString(i18n.language)} · {value.id}</option>)}</select></label>}

          {pending.action === 'upgrade' && <label>{t('Approved release', 'Freigegebene Version')}<select required value={argument} disabled={releasesLoading} onChange={event => { setArgument(event.target.value); setQualificationOnly(true) }}><option value="">{t('Select an available release', 'Verfügbare Version auswählen')}</option>{releases.map(value => <option key={value.id} value={value.id}>{value.name} · {value.id.slice(0, 12)}</option>)}</select></label>}
          {pending.action === 'upgrade' && <><label>{t('Update mode', 'Update-Modus')}<select value={qualificationOnly ? 'test' : 'deploy'} onChange={event => setQualificationOnly(event.target.value === 'test')}><option value="test">{t('Test and restore current version', 'Testen und zur bisherigen Version zurückkehren')}</option><option value="deploy" disabled={!selectedRelease?.qualified}>{t('Install tested version', 'Geprüfte Version installieren')}</option></select></label><p>{t('A backup is created first. Tests use a separate copy without user access. The instance remains stopped afterwards; existing sessions and device connections must be renewed after installation.', 'Zuerst wird eine Sicherung erstellt. Der Test verwendet eine getrennte Kopie ohne Benutzerzugriff. Die Instanz bleibt anschließend gestoppt; nach der Installation müssen Sitzungen und Geräteverbindungen erneuert werden.')}</p>{selectedRelease && !selectedRelease.qualified && <p>{t('This transition needs a successful test before installation.', 'Dieser Versionswechsel muss vor der Installation erfolgreich getestet werden.')}</p>}</>}

          {pending.action === 'rollback' && <><label>{t('Update to undo', 'Zurückzusetzendes Update')}<select required value={argument} disabled={rollbacksLoading} onChange={event => { setArgument(event.target.value); setRollbackAcknowledged(false) }}><option value="">{t('Select the installed update', 'Installiertes Update auswählen')}</option>{rollbacks.map(value => <option key={value.operation_id} value={value.operation_id}>{new Date(value.created_at).toLocaleString(i18n.language)} · {value.source_release.slice(0, 12)} → {value.release.slice(0, 12)}</option>)}</select></label><p>{t('The current state is backed up first. The previous version and its pre-update data are restored into a separate copy. The instance remains stopped; sessions and devices must reconnect, and schedules require review.', 'Zuerst wird der aktuelle Stand gesichert. Die vorherige Version und ihr Datenstand vor dem Update werden in einer getrennten Kopie wiederhergestellt. Die Instanz bleibt gestoppt; Sitzungen und Geräte müssen sich erneut verbinden, Zeitpläne erfordern eine Prüfung.')}</p><label><input type="checkbox" checked={rollbackAcknowledged} onChange={event => setRollbackAcknowledged(event.target.checked)} />{t('I understand that changes after the pre-update backup will no longer be active.', 'Ich verstehe, dass Änderungen seit der Sicherung vor dem Update anschließend nicht mehr aktiv sind.')}</label>{!rollbacksLoading && !rollbacks.length && <p>{t('No rollback is available for the installed generation.', 'Für den installierten Stand ist kein Rollback verfügbar.')}</p>}</>}
          {pendingChanged && <p role="status">{t('The instance changed. Close this dialog and reopen the operation.', 'Die Instanz hat sich geändert. Schließe diesen Dialog und öffne den Vorgang erneut.')}</p>}

          <button disabled={busy || pendingChanged || !!updateInvalid || !!rollbackInvalid || !!deletionInvalid || !!enrollmentInvalid || (pending.action === 'restore' && (backupsLoading || !argument))}>{t('Confirm operation', 'Vorgang bestätigen')}</button><button type="button" onClick={() => setPending(null)}>{t('Cancel', 'Abbrechen')}</button></form>}

      </section>}

      {tab === 'policy' && admin && policy && <EnterprisePolicyEditor policy={policy} onChange={setPolicy} busy={busy} onSave={() => void run(() => client.policy(orgId, { ...policy, revision: policy.revision + 1 }))} />}

      {tab === 'members' && admin && <section><h2>{t('Add an existing account', 'Bestehendes Konto hinzufügen')}</h2><p>{t('Assign an organisation role to an existing server account. To create a new account, use the invitation below.', 'Weise einem bestehenden Serverkonto eine Organisationsrolle zu. Für ein neues Konto verwende die Einladung unter der Mitgliederliste.')}</p><form onSubmit={event => { event.preventDefault(); void run(() => client.request(`/organisations/${orgId}/members`, z.object({ user_id: z.uuid(), role: z.string() }), { email, role, content_admin: role === 'admin' && contentAdmin })) }}><label>E-Mail<input type="email" required value={email} onChange={event => setEmail(event.target.value)} /></label><label>{t('Role', 'Rolle')}<select value={role} onChange={event => { setRole(event.target.value); setContentAdmin(false) }}><option value="member">{t('Member', 'Mitglied')}</option><option value="admin">{t('Organisation administrator', 'Organisationsadministrator')}</option><option value="auditor">{t('Audit reader', 'Audit-Leser')}</option></select></label>{role === 'admin' && <label className="enterprise-check"><input type="checkbox" checked={contentAdmin} onChange={event => setContentAdmin(event.target.checked)} />{t('Permit administrative reading without individual approval when the organisation and instance policies allow it. Reads are audited.', 'Administratives Lesen ohne einzelne Freigabe erlauben, sofern Organisations- und Instanzrichtlinien es zulassen. Zugriffe werden protokolliert.')}</label>}<button disabled={busy}>{t('Assign membership', 'Mitgliedschaft zuweisen')}</button></form></section>}

      {recovery && <form className="enterprise-confirm" aria-label={t('Recover interrupted operation', 'Unterbrochenen Vorgang abgleichen')} onSubmit={event => {

        event.preventDefault()

        if (busy || (recovery.enrollment && !enrollmentAcknowledged) || (recovery.deletion && (argument !== recovery.instance.name || !purgeBackups)) || !rows.some(row => row.id === recovery.operationId && row.state === 'reconciling')) return

        void run(async () => { await client.request(`/organisations/${recovery.instance.organisation_id}/instances/${recovery.instance.id}/operations/${recovery.operationId}/recover`, z.object({ id: z.uuid(), state: z.literal('reconciling') }), recovery.enrollment ? {mode:'continue_enrollment'} : recovery.deletion ? { mode: 'continue_deletion', confirmation_name: argument, purge_backups: purgeBackups } : { mode: 'stop_if_unproven' }); setRecovery(null) })

      }}>

        <h3>{t('Recover interrupted operation', 'Unterbrochenen Vorgang abgleichen')}: {recovery.instance.name}</h3>

        {recovery.enrollment ? <EnterpriseEnrollmentReview recovery checked={enrollmentAcknowledged} onChange={setEnrollmentAcknowledged} /> : recovery.deletion ? <><p>{t('This authorizes one further attempt to complete permanent deletion. Data already deleted cannot be restored. Another interruption requires a new confirmation.', 'Dies erlaubt einen weiteren Versuch, die endgültige Löschung abzuschließen. Bereits gelöschte Daten können nicht wiederhergestellt werden. Nach einer weiteren Unterbrechung ist eine neue Bestätigung erforderlich.')}</p><label>{t('Type the instance name', 'Instanznamen eingeben')}<input required autoComplete="off" value={argument} onChange={event => setArgument(event.target.value)} /></label><label><input type="checkbox" checked={purgeBackups} onChange={event => setPurgeBackups(event.target.checked)} />{t('Permanently delete all managed backups as well.', 'Auch alle verwalteten Sicherungen endgültig löschen.')}</label></> : <p>{t('A verified completion is reported without repeating the operation. If no completion can be proved, all running work in this instance is stopped. For an interrupted update, the recorded previous version and data generation are restored if they can be verified. The instance remains stopped for review. No operation is automatically repeated.', 'Ein belegter Abschluss wird nachgemeldet. Fehlt der Abschlussbeleg, werden alle laufenden Aufgaben dieser Instanz gestoppt. Bei einem unterbrochenen Update werden die protokollierte vorige Version und ihr Datenbestand wiederhergestellt, sofern sie sich verifizieren lassen. Die Instanz bleibt zur Prüfung gestoppt. Der Vorgang wird nicht automatisch wiederholt.')}</p>}

        <button disabled={busy || (recovery.enrollment && !enrollmentAcknowledged) || (recovery.deletion && (argument !== recovery.instance.name || !purgeBackups)) || !rows.some(row => row.id === recovery.operationId && row.state === 'reconciling')}>{recovery.enrollment ? t('Continue enrollment review','Übernahmeabgleich fortsetzen') : recovery.deletion ? t('Continue permanent deletion', 'Endgültige Löschung fortsetzen') : t('Reconcile; stop if unproven', 'Abgleichen; ohne Beleg stoppen')}</button>

        <button type="button" onClick={() => setRecovery(null)}>{t('Cancel', 'Abbrechen')}</button>

      </form>}

      {rows.length > 0 && (tab === 'members' || tab === 'audit' || tab === 'instances' && operationsFor) && <section aria-label={t('Records', 'Einträge')}>
        {tab === 'members' && <h2>{t('Organisation members', 'Organisationsmitglieder')} <span className="enterprise-status">{rows.length}</span></h2>}

        {tab === 'instances' && operationsFor && <h3>{t('Operations', 'Vorgänge')}: {operationsFor.name}</h3>}

        <div className="enterprise-records"><table><thead><tr>

          {(tab === 'members' ? [t('Member', 'Mitglied'), t('Role', 'Rolle'), t('Administrative content access', 'Administrativer Inhaltszugriff'), t('Access & resources', 'Zugang & Ressourcen')] : tab === 'audit' ? [t('Event', 'Ereignis'), t('Time', 'Zeit'), t('Resource', 'Ressource')] : [t('Operation', 'Vorgang'), t('State', 'Status'), t('Time', 'Zeit'), t('Result', 'Ergebnis'), t('Recovery', 'Wiederaufnahme')]).map(label => <th key={label} scope="col">{label}</th>)}

        </tr></thead><tbody>{rows.map((row, index) => <tr key={String(row.id ?? row.sequence ?? row.user_id ?? index)}>

          {tab === 'members' ? <><td><strong>{String(row.display_name ?? '')}</strong><small className="enterprise-person-email">{String(row.email ?? '')}</small></td><td><span className="enterprise-status">{row.role === 'admin' ? t('Administrator', 'Administrator') : row.role === 'auditor' ? t('Audit reader', 'Audit-Leser') : t('Member', 'Mitglied')}</span><small className="enterprise-person-email">{row.sso_provider_id ? t('SSO managed', 'Über SSO verwaltet') : t('Assigned directly', 'Direkt zugewiesen')}</small></td><td>{row.role !== 'admin' ? t('No administrative content access', 'Kein administrativer Inhaltszugriff') : row.content_admin ? t('Permitted by policy', 'Nach Richtlinie erlaubt') : t('Approval required', 'Freigabe erforderlich')}</td></> : (tab === 'audit' ? [row.action, row.created_at ? new Date(String(row.created_at)).toLocaleString(i18n.language) : '—', row.target_id] : [labels[row.action as InstanceAction] ?? row.action, row.state, row.created_at ? new Date(String(row.created_at)).toLocaleString(i18n.language) : '—', typeof row.result === 'object' && row.result ? (row.result as Record<string, unknown>).error_code ?? (row.result as Record<string, unknown>).receipt_id : '—']).map((value, cell) => <td key={cell}>{String(value ?? '—')}</td>)}
          {tab === 'members' && <td><div className="enterprise-actions"><button type="button" disabled={busy || !admin} onClick={() => { setResourceMember(null); setAccessMember({ user_id: String(row.user_id), display_name: String(row.display_name ?? ''), email: String(row.email ?? '') }) }}>{t('Instance access', 'Instanzzugänge')}</button>{capabilities.wallets && <button type="button" disabled={busy || !admin} onClick={() => { setWalletMemberId(String(row.user_id)); setTab('wallet') }}>{t('API budget', 'API-Budget')}</button>}{capabilities.resource_catalog && <button type="button" disabled={busy || !admin} onClick={() => { setAccessMember(null); setResourceMember({ user_id: String(row.user_id), display_name: String(row.display_name ?? ''), email: String(row.email ?? '') }) }}>{t('Resource assignments', 'Ressourcenzuweisungen')}</button>}</div></td>}

          {tab === 'instances' && <td>{row.state === 'reconciling' && operationsFor && instances.some(value => value.id === operationsFor.id && value.can_manage && value.backend === 'compose' && value.policy.minimum_isolation === 'container') && (admin || (!['suspend', 'resume', 'enroll_execution'].includes(String(row.action)) && !capacityRecovery(row))) ? <>

            {row.recovery_requested_at && !['delete','enroll_execution'].includes(String(row.action)) ? <span role="status">{t('Recovery requested. Awaiting verified result.', 'Abgleich angefordert. Ein geprüftes Ergebnis steht noch aus.')}</span> : null}

            <button type="button" disabled={busy} onClick={() => { setArgument(''); setPurgeBackups(false);setEnrollmentAcknowledged(false); setPending(null); setRecovery({ instance: operationsFor, operationId: String(row.id), deletion: row.action === 'delete', enrollment:row.action==='enroll_execution' }) }}>{t('Review interrupted operation', 'Unterbrochenen Vorgang prüfen')}</button>

          </> : '—'}</td>}

        </tr>)}</tbody></table></div>

      </section>}

      {tab === 'members' && admin && accessMember && <EnterpriseMemberAccess key={`${orgId}:${accessMember.user_id}`} client={client} member={accessMember} instances={instances} onClose={() => setAccessMember(null)} />}
      {tab === 'members' && admin && capabilities.resource_catalog && resourceMember && <EnterpriseMemberResources key={`${orgId}:${resourceMember.user_id}`} client={client} organisationId={orgId} member={resourceMember} instances={instances} onClose={() => setResourceMember(null)} />}
      {tab === 'members' && admin && <details className="enterprise-invite-details"><summary>{t('Invite a new person to this organisation', 'Eine neue Person in diese Organisation einladen')}</summary><EnterpriseInvitations key={orgId} client={client} orgId={orgId} serverUrl={account.serverUrl} /></details>}

    </>}

  </EnterpriseConsoleLayout>

}

