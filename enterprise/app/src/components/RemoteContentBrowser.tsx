import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { ProjectRecord, ThreadRecord } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import RemoteThreadMessages from './RemoteThreadMessages'
import InstanceContentSharing from './InstanceContentSharing'

/** Uses the same object authorisation as runs; never supplies a reply context. */
export default function RemoteContentBrowser({ client }: { client: RemoteRuntimeClient }) {
  const { i18n } = useTranslation()
  const t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const [open, setOpen] = useState(false)
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const [projectId, setProjectId] = useState('')
  const [loadedThreads, setLoadedThreads] = useState<{ projectId: string; items: ThreadRecord[] } | null>(null)
  const [threadId, setThreadId] = useState('')
  const [error, setError] = useState('')
  const [reload, setReload] = useState(0)
  useEffect(() => {
    if (!open) return
    let canceled = false
    void client.listProjects().then(rows => {
      if (!canceled) { setProjects(rows); setProjectId(current => rows.some(row => row.id === current) ? current : rows[0]?.id ?? ''); setError('') }
    }).catch(cause => { if (!canceled) { setProjects([]); setProjectId(''); setError(String(cause)) } })
    return () => { canceled = true }
  }, [client, open, reload])
  useEffect(() => {
    if (!open || !projectId) return
    let canceled = false
    setLoadedThreads(null); setThreadId('')
    void client.listProjectThreads(projectId).then(items => {
      if (!canceled) { setLoadedThreads({ projectId, items }); setThreadId(items[0]?.id ?? ''); setError('') }
    }).catch(cause => { if (!canceled) { setLoadedThreads(null); setError(String(cause)) } })
    return () => { canceled = true }
  }, [client, open, projectId, reload])
  if (!open) return <button type="button" onClick={() => setOpen(true)}>{t('Read content', 'Inhalte lesen')}</button>
  const threads = loadedThreads?.projectId === projectId ? loadedThreads.items : []
  return <section className="remote-management-panel" aria-label={t('Read content', 'Inhalte lesen')}>
    <header><strong>{t('Read content', 'Inhalte lesen')}</strong><button type="button" onClick={() => setOpen(false)}>{t('Close', 'Schließen')}</button></header>
    <p>{t('Only projects and chats authorised for this account are shown.', 'Hier erscheinen nur Projekte und Chats, für die dieses Konto berechtigt ist.')}</p>
    <button type="button" onClick={() => setReload(value => value + 1)}>{t('Refresh content', 'Inhalte aktualisieren')}</button>
    <label>{t('Project', 'Projekt')}<select value={projectId} onChange={event => setProjectId(event.target.value)}><option value="">{t('Select project', 'Projekt auswählen')}</option>{projects.map(row => <option key={row.id} value={row.id}>{row.name}</option>)}</select></label>
    {projectId && <label>{t('Chat', 'Chat')}<select value={threads.some(row => row.id === threadId) ? threadId : ''} onChange={event => setThreadId(event.target.value)}><option value="">{t('Select chat', 'Chat auswählen')}</option>{threads.map(row => <option key={row.id} value={row.id}>{row.title}</option>)}</select></label>}
    {projects.find(row => row.id === projectId)?.description && <p>{projects.find(row => row.id === projectId)?.description}</p>}
    {error && <p role="alert">{error}</p>}
    <InstanceContentSharing key={client.enterpriseSource} client={client} projectId={projectId} />
    {threads.some(row => row.id === threadId) && <RemoteThreadMessages key={threadId} client={client} threadId={threadId} reloadKey={reload} />}
  </section>
}
