import { Activity, ArrowUpCircle, Boxes, Building2, ChevronRight, Database, FolderInput, Gauge, LayoutDashboard, LogOut, Network, RefreshCw, Server, Settings2, ShieldCheck, Users, Wallet, Workflow, type LucideIcon } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useContext, useEffect, useId, useRef, useState, type ReactNode } from 'react'
import type { Organisation } from '../runtime/managementClient'
import type { ManagementArea } from '../runtime/managementAreas'
import { ManagementSecurityContext } from './managementSecurityContext'
import RemoteSecuritySettings from './RemoteSecuritySettings'
import SignInDialog from './SignInDialog'


type Props = {
  children: ReactNode; area: ManagementArea; onArea: (area: ManagementArea) => void
  organisations: Organisation[]; organisationId: string; onOrganisation: (id: string) => void
  platformAdmin: boolean; busy: boolean; serverUrl: string; email: string
  admissionEnabled: boolean; importsEnabled: boolean; onRefresh: () => void; onLogout: () => void
}

export default function EnterpriseConsoleLayout(props: Props) {
  const [navOpen, setNavOpen] = useState(false)
  const [securityOpen, setSecurityOpen] = useState(false)
  const [securityCloseBlocked, setSecurityCloseBlocked] = useState(false)
  const accountSecurity = useContext(ManagementSecurityContext)
  const securityHeadingId = useId()
  const mainRef = useRef<HTMLElement>(null)
  useEffect(() => { mainRef.current?.scrollTo?.(0, 0) }, [props.area])
  const { i18n } = useTranslation()
  const de = i18n.language.startsWith('de')
  const t = (en: string, german: string) => de ? german : en
  const org = props.organisations.find(item => item.id === props.organisationId)
  const admin = props.platformAdmin || org?.role === 'admin'
  const names: Record<ManagementArea, string> = {
    overview: t('Server overview', 'Serverübersicht'), organisations: t('Organisations', 'Organisationen'), catalog: t('Resource catalog', 'Ressourcenkatalog'),
    instances: t('Instances', 'Instanzen'), environments: t('Runtime environments', 'Laufzeitumgebungen'), members: t('Members', 'Mitglieder'),
    sharing: t('Shared content', 'Freigegebene Inhalte'), imports: t('Instance imports', 'Instanzimporte'), rollouts: t('Batch updates', 'Gruppenupdates'),
    policy: t('Policies', 'Richtlinien'), audit: t('Audit', 'Audit'), sso: 'SSO', directory: t('Directory', 'Verzeichnis'),
    capacity: t('Task capacity', 'Aufgabenkapazität'), mfa: t('MFA requirement', 'MFA-Pflicht'),
    wallet: t('API budgets', 'API-Budgets'),
  }
  const descriptions: Record<ManagementArea, string> = {
    overview: t('Your control server, organisations and managed Cowork servers in one place.', 'Dein Verwaltungsserver, deine Organisationen und die verwalteten Cowork-Server auf einen Blick.'),
    organisations: t('Manage the administrative boundary for members, servers and shared rules.', 'Verwalte den gemeinsamen Rahmen für Mitglieder, Server und Richtlinien.'),
    catalog: t('Define models, endpoints, MCP servers, skills and agent roles centrally.', 'Modelle, Endpunkte, MCP-Server, Skills und Agentrollen zentral anlegen.'),
    instances: t('Start, open and maintain the Cowork servers belonging to this organisation.', 'Die Cowork-Server dieser Organisation öffnen, starten und warten.'),
    environments: t('Inspect execution environments and their actual isolation backend.', 'Ausführungsumgebungen und ihr tatsächlich verwendetes Isolationsbackend verwalten.'),
    members: t('Manage organisation membership, instance access and resource assignments.', 'Organisationsmitglieder, Instanzzugänge und Ressourcenzuweisungen verwalten.'),
    sharing: t('Review content shared between members and instances.', 'Zwischen Mitgliedern und Instanzen freigegebene Inhalte prüfen.'),
    imports: t('Bring an existing server into this organisation.', 'Einen bestehenden Server in diese Organisation übernehmen.'),
    rollouts: t('Choose tested releases and schedule controlled updates across servers.', 'Geprüfte Versionen auswählen und Updates über mehrere Server steuern.'),
    policy: t('Set resource ceilings, isolation and access rules for this organisation.', 'Ressourcenlimits, Isolation und Zugriffsregeln für diese Organisation festlegen.'),
    audit: t('Trace administrative changes and the resources they affected.', 'Administrative Änderungen und betroffene Ressourcen nachvollziehen.'),
    sso: t('Connect your organisation’s identity provider.', 'Den Identitätsanbieter deiner Organisation verbinden.'),
    directory: t('Synchronise membership with your identity directory.', 'Mitgliedschaften mit dem Identitätsverzeichnis abgleichen.'),
    capacity: t('Control concurrent tasks and inspect reservations.', 'Gleichzeitige Aufgaben begrenzen und Reservierungen prüfen.'),
    mfa: t('Manage additional sign-in protection for this organisation.', 'Zusätzlichen Anmeldeschutz für diese Organisation verwalten.'),
    wallet: t('Member credits, API cost limits and a traceable spending ledger.', 'Mitgliederguthaben, API-Kostenlimits und nachvollziehbare Buchungen.'),
  }
  const icons: Record<ManagementArea, LucideIcon> = { overview: LayoutDashboard, organisations: Building2, catalog: Boxes, instances: Server, environments: Workflow, members: Users, sharing: Database, imports: FolderInput, rollouts: ArrowUpCircle, policy: Settings2, audit: Activity, sso: Network, directory: Users, capacity: Gauge, mfa: ShieldCheck, wallet: Wallet }
  const navButton = (area: ManagementArea) => {
    const Icon = icons[area]
    return <button key={area} type="button" aria-current={props.area === area ? 'page' : undefined} disabled={props.busy} onClick={() => { props.onArea(area); setNavOpen(false) }}><Icon size={17} aria-hidden="true" /><span>{names[area]}</span>{props.area === area && <ChevronRight size={14} aria-hidden="true" />}</button>
  }
  const orgAreas: ManagementArea[] = ['instances', 'environments', ...(admin ? ['members' as const] : []), ...(['admin', 'member'].includes(org?.role ?? '') ? ['sharing' as const] : [])]
  const operations: ManagementArea[] = ['wallet', ...(admin || org?.role === 'auditor' ? ['rollouts' as const, 'audit' as const, ...(props.admissionEnabled ? ['capacity' as const] : [])] : []), ...(admin && props.importsEnabled ? ['imports' as const] : [])]
  const security: ManagementArea[] = admin ? ['policy', 'sso', 'directory', 'mfa'] : []
  return <div className="enterprise-shell">
    <aside className="enterprise-sidebar" data-open={navOpen} id="enterprise-navigation">
      <button type="button" className="enterprise-mobile-nav" onClick={() => setNavOpen(false)}>{t('Close navigation', 'Navigation schließen')}</button>
      <a className="enterprise-brand" href="/management"><span className="enterprise-brand-icon"><Boxes size={22} aria-hidden="true" /></span><span>Open Cowork<small>{t('Server management', 'Serververwaltung')}</small></span></a>
      <nav aria-label={t('Management areas', 'Verwaltungsbereiche')}>
        <div className="enterprise-nav-group"><p>{t('Control plane', 'Verwaltung')}</p>{navButton('overview')}{navButton('organisations')}{props.platformAdmin && navButton('catalog')}</div>
        <label className="enterprise-org-picker">{t('Organisation', 'Organisation')}<select value={props.organisationId} disabled={props.busy} onChange={event => { props.onOrganisation(event.target.value); setNavOpen(false) }}><option value="">{t('Select organisation', 'Organisation auswählen')}</option>{props.organisations.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select></label>
        {org && <><div className="enterprise-nav-group"><p>{t('Workspace servers', 'Arbeitsumgebung')}</p>{orgAreas.map(navButton)}</div>{operations.length > 0 && <div className="enterprise-nav-group"><p>{t('Operations', 'Betrieb')}</p>{operations.map(navButton)}</div>}{security.length > 0 && <div className="enterprise-nav-group"><p>{t('Security', 'Sicherheit')}</p>{security.map(navButton)}</div>}</>}
      </nav>
      <div className="enterprise-sidebar-note"><ShieldCheck size={16} aria-hidden="true" /><span>{t('Organisation access and instance access are managed separately.', 'Organisations- und Instanzzugänge werden getrennt vergeben.')}</span></div>
    </aside>
    <div className="enterprise-workspace">
      <header className="enterprise-topbar"><span className="enterprise-server-address"><Server size={15} aria-hidden="true" /><span>{props.serverUrl}</span></span><div><span className="enterprise-account">{props.email || t('Administrator', 'Administrator')}</span>{accountSecurity && <button type="button" aria-label={t('Account security', 'Kontosicherheit')} title={t('Account security', 'Kontosicherheit')} onClick={() => setSecurityOpen(true)}><ShieldCheck size={17} aria-hidden="true" /></button>}<button type="button" aria-label={t('Refresh', 'Aktualisieren')} title={t('Refresh', 'Aktualisieren')} disabled={props.busy} onClick={props.onRefresh}><RefreshCw size={17} aria-hidden="true" /></button><button type="button" aria-label={t('Sign out / switch server', 'Abmelden / Server wechseln')} title={t('Sign out / switch server', 'Abmelden / Server wechseln')} onClick={props.onLogout}><LogOut size={17} aria-hidden="true" /></button></div></header>
      <button type="button" className="enterprise-mobile-nav enterprise-nav-toggle" aria-expanded={navOpen} aria-controls="enterprise-navigation" onClick={() => setNavOpen(!navOpen)}>{t('Navigation', 'Navigation')}</button>
      <main className="enterprise-management" id="enterprise-main" ref={mainRef}>
        <div className="enterprise-page-heading"><p className="enterprise-eyebrow">{['overview', 'organisations', 'catalog'].includes(props.area) ? t('Server management', 'Serververwaltung') : org?.name}</p><h1>{names[props.area]}</h1><p>{descriptions[props.area]}</p></div>
        {props.children}
      </main>
    </div>
    {accountSecurity && securityOpen && <SignInDialog labelledBy={securityHeadingId} busy={securityCloseBlocked} onCancel={() => setSecurityOpen(false)}><div className="enterprise-management enterprise-account-dialog"><RemoteSecuritySettings {...accountSecurity} compact initiallyOpen headingId={securityHeadingId} onClose={() => setSecurityOpen(false)} onCloseBlockedChange={setSecurityCloseBlocked} /></div></SignInDialog>}
  </div>
}
