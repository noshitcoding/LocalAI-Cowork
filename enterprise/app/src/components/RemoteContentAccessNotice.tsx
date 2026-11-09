import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { RuntimeHttpError, type RemoteRuntimeClient } from '../runtime/runtimeClient'
import type { InstanceContentAccess } from '../runtime/enterpriseContracts'

export default function RemoteContentAccessNotice({ client, onReadOnly }: { client: RemoteRuntimeClient; onReadOnly: (value: boolean) => void }) {
  const { i18n } = useTranslation()
  const t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const [loaded, setLoaded] = useState<{ client: RemoteRuntimeClient; info: InstanceContentAccess | null } | null>(null)
  const info = loaded?.client === client ? loaded.info : null
  const [unavailable, setUnavailable] = useState(false)
  useEffect(() => {
    let canceled = false
    let timer: ReturnType<typeof setTimeout>
    const load = async () => {
      try {
        const value = await client.getContentAccess()
        if (!canceled) { setLoaded({ client, info: value }); setUnavailable(false); onReadOnly(value.read_only) }
      } catch (error) {
        // The additive endpoint is absent on older standalone instances.
        if (!canceled) {
          if (error instanceof RuntimeHttpError && error.status === 404) { setLoaded({ client, info: null }); setUnavailable(false); onReadOnly(false) }
          else setUnavailable(true)
        }
      } finally { if (!canceled) timer = setTimeout(() => { void load() }, 30000) }
    }
    void load()
    return () => { canceled = true; clearTimeout(timer) }
  }, [client, onReadOnly])
  if (unavailable) return <p role="status">{t('The administrative access rule could not be verified. Retrying automatically.', 'Die Regel für administrativen Inhaltszugriff konnte nicht geprüft werden. Sie wird automatisch erneut abgefragt.')}</p>
  if (!info?.managed) return null
  return <aside aria-label={t('Administrative content access', 'Administrativer Inhaltszugriff')}>
    <p>{info.content_access === 'organisation_admin'
      ? t('This organisation allows explicitly authorised administrators to read instance content. Each administrative access is audited.', 'Diese Organisation erlaubt ausdrücklich berechtigten Administratoren, Instanzinhalte zu lesen. Jeder administrative Zugriff wird protokolliert.')
      : t('Administrators can read content only with a project or chat approval lasting at most 24 hours. Each administrative access is audited.', 'Administratoren können Inhalte nur nach einer Projekt- oder Chat-Freigabe für höchstens 24 Stunden lesen. Jeder administrative Zugriff wird protokolliert.')}</p>
    {info.read_only && <p role="status">{info.instance_viewer
      ? t('Instance access: read only. Project sharing still determines which content you can see.', 'Instanzzugriff: nur lesen. Projektfreigaben bestimmen weiterhin, welche Inhalte du sehen kannst.')
      : info.automatic_content_access
      ? t('Support access: read only. Your explicit content-reader permission is active.', 'Support-Zugriff: nur lesen. Deine ausdrückliche Berechtigung zum Lesen von Inhalten ist aktiv.')
      : t('Support access: read only. Without an active approval, no project or chat content is visible.', 'Support-Zugriff: nur lesen. Ohne aktive Freigabe sind keine Projekt- oder Chat-Inhalte sichtbar.')}</p>}
  </aside>
}
