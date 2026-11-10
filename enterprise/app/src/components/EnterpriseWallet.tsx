import { ArrowDownLeft, ArrowUpRight, Check, CircleHelp, Clock3, Coins, Eye, History, LockKeyhole, RefreshCw, ShieldCheck, Wallet } from 'lucide-react'
import { useEffect, useMemo, useRef, useState, type FormEvent } from 'react'
import { useTranslation } from 'react-i18next'
import type { ManagementClient } from '../runtime/managementClient'
import { wallets, type WalletResolve } from '../runtime/wallet'
import { currencyInput, formatCurrencyMicros, parseCurrencyMicros } from '../runtime/money'
import './EnterpriseWallet.css'

type WalletApi = ReturnType<typeof wallets>
type WalletList = Awaited<ReturnType<WalletApi['list']>>
type WalletDetail = Awaited<ReturnType<WalletApi['read']>>
type Props = { client: ManagementClient; orgId: string; canManage: boolean; enabled?: boolean; initialUserId?: string }
const maximumMicros = 9_000_000_000_000

export default function EnterpriseWallet(props: Props) {
  return <EnterpriseWalletContent key={`${props.orgId}:${props.initialUserId ?? ''}`} {...props} />
}

function EnterpriseWalletContent({ client, orgId, canManage, enabled = false, initialUserId }: Props) {
  const { i18n } = useTranslation(), t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const api = useMemo(() => wallets(client, orgId), [client, orgId])
  const [list, setList] = useState<WalletList | null>(null), [selected, setSelected] = useState(initialUserId ?? ''), [reload, setReload] = useState(0), [refresh, setRefresh] = useState(0)
  const [loading, setLoading] = useState(true), [error, setError] = useState(''), [busy, setBusy] = useState(false)
  useEffect(() => {
    if (!enabled) { setLoading(false); return }
    const abort = new AbortController(); setLoading(true); setError('')
    void api.list(abort.signal).then(result => {
      if (abort.signal.aborted) return
      if (result.organisation_id !== orgId) throw new Error('Wallet response belongs to another organisation.')
      setList(result); setSelected(current => result.wallets.some(item => item.user_id === current) ? current : result.wallets[0]?.user_id ?? '')
    }).catch(reason => { if (!abort.signal.aborted) setError(String(reason)) }).finally(() => { if (!abort.signal.aborted) setLoading(false) })
    return () => abort.abort()
  }, [api, enabled, orgId, reload])
  if (!enabled) return <section className="enterprise-wallet"><div className="enterprise-wallet-empty"><Wallet size={28} aria-hidden="true" /><h2>{t('Member wallets require a server update', 'Mitglieder-Wallets benötigen ein Serverupdate')}</h2><p>{t('This server does not provide enforced API budgets yet.', 'Dieser Server stellt noch keine durchgesetzten API-Budgets bereit.')}</p></div></section>
  const member = list?.wallets.find(item => item.user_id === selected)
  return <div className="enterprise-wallet">
    <section className="enterprise-wallet-coverage" aria-label={t('Budget coverage', 'Geltungsbereich des Budgets')}><div><ShieldCheck size={18} aria-hidden="true" /><p>{t('Active wallets reserve the maximum request cost before the managed model proxy calls the provider. Available credit and the monthly limit both apply.', 'Aktive Wallets reservieren die maximalen Anfragekosten, bevor der verwaltete Modellproxy den Anbieter aufruft. Verfügbares Guthaben und Monatslimit gelten gleichzeitig.')}</p></div><div className="enterprise-wallet-badges"><span><LockKeyhole size={13} aria-hidden="true" />{t('Enforced in the managed model proxy', 'Im verwalteten Modellproxy durchgesetzt')}</span><span>{t('Organisation currency', 'Organisationswährung')}: {list?.currency ?? '—'}</span></div><p>{t('Direct provider requests, local models and other traffic are outside this budget. Credits are internal allocations; no payment is collected and no provider invoice is created here.', 'Direkte Anbieteranfragen, lokale Modelle und anderer Datenverkehr liegen außerhalb dieses Budgets. Guthaben sind interne Zuteilungen; hier wird keine Zahlung eingezogen und keine Anbieterrechnung erstellt.')}</p></section>
    <section className="enterprise-wallet-members"><header className="enterprise-wallet-heading"><div><h2><Wallet size={21} aria-hidden="true" />{t('Member API budgets', 'API-Budgets der Mitglieder')}</h2><p className="enterprise-muted">{t('Inspect credit, reservations and recorded charges for the members visible to you.', 'Guthaben, Reservierungen und verbuchte Kosten der für dich sichtbaren Mitglieder prüfen.')}</p></div><button type="button" disabled={busy || loading} onClick={() => { setReload(value => value + 1); setRefresh(value => value + 1) }}><RefreshCw size={15} aria-hidden="true" />{t('Refresh wallets', 'Wallets aktualisieren')}</button></header>
      {error && <p role="alert" className="enterprise-wallet-error">{error}</p>}
      {loading && !list ? <p role="status">{t('Loading wallets…', 'Wallets werden geladen…')}</p> : list && !list.wallets.length ? <div className="enterprise-wallet-empty"><Coins size={25} aria-hidden="true" /><h3>{t('No member wallets available', 'Keine Mitglieder-Wallets verfügbar')}</h3><p>{t('No wallets are visible in this organisation.', 'In dieser Organisation sind keine Wallets sichtbar.')}</p></div> : member && <>
        <div className="enterprise-wallet-member-controls"><div><h3>{member.display_name || member.email}</h3><p className="enterprise-muted">{member.email}</p></div><label>{t('Member wallet', 'Wallet des Mitglieds')}<select disabled={busy || loading} value={selected} onChange={event => setSelected(event.target.value)}>{list?.wallets.map(item => <option key={item.user_id} value={item.user_id}>{item.display_name || item.email} · {item.email}</option>)}</select></label></div>
        <WalletMember key={`${selected}:${refresh}`} api={api} orgId={orgId} userId={selected} canManage={canManage} onBusy={setBusy} onChanged={() => setReload(value => value + 1)} />
      </>}
    </section>
  </div>
}

function WalletMember({ api, orgId, userId, canManage, onBusy, onChanged }: { api: WalletApi; orgId: string; userId: string; canManage: boolean; onBusy: (busy: boolean) => void; onChanged: () => void }) {
  const { i18n } = useTranslation(), t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const [detail, setDetail] = useState<WalletDetail | null>(null), [loading, setLoading] = useState(true), [error, setError] = useState(''), [saved, setSaved] = useState('')
  const [active, setActive] = useState(false), [limit, setLimit] = useState(''), [entryKind, setEntryKind] = useState<'credit' | 'debit'>('credit'), [amount, setAmount] = useState(''), [reason, setReason] = useState(''), [busy, setBusy] = useState(false)
  const mounted = useRef(true), mutation = useRef(false), entryRequest = useRef<{ fingerprint: string; id: string } | null>(null)
  useEffect(() => {
    mounted.current = true
    const abort = new AbortController()
    void api.read(userId, abort.signal).then(result => {
      if (abort.signal.aborted) return
      if (result.organisation_id !== orgId || result.user_id !== userId) throw new Error('Wallet response does not match the selected member wallet.')
      setDetail(result); setActive(result.enabled); setLimit(result.monthly_limit_micros === null ? '' : currencyInput(result.monthly_limit_micros))
    }).catch(cause => { if (!abort.signal.aborted) setError(String(cause)) }).finally(() => { if (!abort.signal.aborted) setLoading(false) })
    return () => { mounted.current = false; abort.abort() }
  }, [api, orgId, userId])
  const parsedLimit = limit.trim() ? parseCurrencyMicros(limit) : null, parsedAmount = parseCurrencyMicros(amount)
  const validLimit = !limit.trim() || parsedLimit !== null && parsedLimit <= maximumMicros
  const validAmount = parsedAmount !== null && parsedAmount > 0 && parsedAmount <= maximumMicros
  const exceedsAvailable = entryKind === 'debit' && detail !== null && parsedAmount !== null && parsedAmount > detail.available_micros
  const dirty = !!detail && (active !== detail.enabled || (limit.trim() ? parsedLimit : null) !== detail.monthly_limit_micros)
  async function run(operation: () => Promise<WalletDetail>, success: string, resetEntry = false) {
    if (!canManage || mutation.current || !detail) return
    mutation.current = true; setBusy(true); onBusy(true); setError(''); setSaved('')
    try {
      const result = await operation()
      if (!mounted.current) return
      if (result.organisation_id !== orgId || result.user_id !== userId) throw new Error('Wallet response does not match the selected member wallet.')
      setDetail(result); setActive(result.enabled); setLimit(result.monthly_limit_micros === null ? '' : currencyInput(result.monthly_limit_micros)); setSaved(success)
      if (resetEntry) { entryRequest.current = null; setAmount(''); setReason('') }
      onChanged()
    } catch (cause) { if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { mutation.current = false; if (mounted.current) { setBusy(false); onBusy(false) } }
  }
  function saveLimits(event: FormEvent) {
    event.preventDefault(); if (!detail || !validLimit || !dirty) return
    void run(() => api.update(userId, { expected_revision: detail.revision, enabled: active, monthly_limit_micros: limit.trim() ? parsedLimit : null }), t('Wallet limits saved.', 'Wallet-Limits gespeichert.'))
  }
  function saveEntry(event: FormEvent) {
    event.preventDefault(); if (!detail || !validAmount || exceedsAvailable || parsedAmount === null || !reason.trim()) return
    const fingerprint = `${entryKind}:${parsedAmount}:${reason.trim()}`
    if (entryRequest.current?.fingerprint !== fingerprint) entryRequest.current = { fingerprint, id: crypto.randomUUID() }
    const requestId = entryRequest.current.id
    void run(() => api.entry(userId, { request_id: requestId, expected_revision: detail.revision, kind: entryKind, amount_micros: parsedAmount, reason: reason.trim() }), t('Internal credit allocation recorded.', 'Interne Guthabenbuchung erfasst.'), true)
  }
  const money = (value: number) => formatCurrencyMicros(value, detail?.currency ?? 'EUR', i18n.language)
  const date = (value: string) => new Date(value).toLocaleString(i18n.language)
  if (loading) return <p role="status">{t('Loading member wallet…', 'Mitglieder-Wallet wird geladen…')}</p>
  if (!detail) return <p role="alert" className="enterprise-wallet-error">{error || t('Wallet unavailable.', 'Wallet nicht verfügbar.')}</p>
  const enforced = detail.enabled && detail.enforcement.state === 'enforced'
  return <>
    <div className="enterprise-wallet-badges"><span><ShieldCheck size={13} aria-hidden="true" />{enforced ? t('Wallet limits active', 'Wallet-Limits aktiv') : t('Wallet limits inactive', 'Wallet-Limits inaktiv')}</span><span>{t('Wallet currency', 'Wallet-Währung')}: {detail.currency}</span><span><Clock3 size={13} aria-hidden="true" />{t('Monthly accounting period', 'Monatlicher Abrechnungszeitraum')}</span></div>
    {!enforced && <p className="enterprise-muted">{t('This wallet is disabled. Its credit and monthly limit do not currently block model calls.', 'Dieses Wallet ist deaktiviert. Sein Guthaben und Monatslimit blockieren Modellaufrufe derzeit nicht.')}</p>}
    <dl className="enterprise-wallet-summary"><div className="enterprise-wallet-available"><dt><Coins size={16} aria-hidden="true" />{t('Available credit', 'Verfügbares Guthaben')}</dt><dd>{money(detail.available_micros)}</dd><small>{t('Balance minus reservations', 'Guthaben abzüglich Reservierungen')}</small></div><div><dt><Wallet size={16} aria-hidden="true" />{t('Account balance', 'Kontostand')}</dt><dd>{money(detail.balance_micros)}</dd><small>{t('Recorded internal credit', 'Verbuchtes internes Guthaben')}</small></div><div><dt><LockKeyhole size={16} aria-hidden="true" />{t('Reserved', 'Reserviert')}</dt><dd>{money(detail.reserved_micros)}</dd><small>{t('Held for outstanding requests', 'Für offene Anfragen zurückgehalten')}</small></div><div><dt><ArrowUpRight size={16} aria-hidden="true" />{t('Spent this month', 'Diesen Monat verbraucht')}</dt><dd>{money(detail.monthly_spent_micros)}</dd><small>{detail.monthly_limit_micros === null ? t('No additional monthly cap', 'Kein zusätzliches Monatslimit') : `${t('Limit', 'Limit')}: ${money(detail.monthly_limit_micros)}`}</small></div></dl>
    <p className="enterprise-wallet-period"><Clock3 size={14} aria-hidden="true" />{t('Period starts', 'Zeitraum ab')}: <strong>{date(detail.period_start)}</strong> · {t('Reserved against monthly limit', 'Auf das Monatslimit reserviert')}: <strong>{money(detail.monthly_reserved_micros)}</strong></p>
    {error && <p role="alert" className="enterprise-wallet-error">{error}</p>}{saved && <p role="status" className="enterprise-wallet-saved"><Check size={15} aria-hidden="true" />{saved}</p>}
    {canManage ? <div className="enterprise-wallet-forms"><form onSubmit={saveLimits}><div><h4><ShieldCheck size={17} aria-hidden="true" />{t('Enforced budget', 'Durchgesetztes Budget')}</h4><p>{t('Enable this wallet to require sufficient credit and enforce its monthly cap on covered model calls.', 'Aktiviere dieses Wallet, damit abgedeckte Modellaufrufe ausreichend Guthaben benötigen und das Monatslimit einhalten.')}</p></div><label className="enterprise-check"><input type="checkbox" checked={active} disabled={busy} onChange={event => setActive(event.target.checked)} />{t('Enable wallet enforcement', 'Wallet-Begrenzung aktivieren')}</label><label>{t('Monthly limit', 'Monatslimit')} ({detail.currency})<input inputMode="decimal" placeholder={t('No monthly cap', 'Kein Monatslimit')} value={limit} disabled={busy} aria-invalid={!validLimit} onChange={event => setLimit(event.target.value)} /><small>{t('Leave empty for no monthly cap. Credit is still required. Zero blocks covered calls.', 'Leer lassen für kein Monatslimit. Guthaben bleibt erforderlich. Null blockiert abgedeckte Aufrufe.')}</small></label>{!validLimit && <p role="alert">{t('Enter a nonnegative amount with at most six decimal places. Maximum: ', 'Gib einen nicht negativen Betrag mit höchstens sechs Nachkommastellen ein. Maximum: ')}{money(maximumMicros)}</p>}<button className="enterprise-primary" disabled={busy || !validLimit || !dirty}>{t('Save wallet limits', 'Wallet-Limits speichern')}</button></form>
      <form onSubmit={saveEntry}><div><h4><ArrowDownLeft size={17} aria-hidden="true" />{t('Allocate internal credit', 'Internes Guthaben zuteilen')}</h4><p>{t('Record an internal allocation or withdrawal. This does not charge a bank card or transfer money to a provider.', 'Eine interne Zuteilung oder einen Entzug verbuchen. Dadurch wird keine Karte belastet und kein Geld an einen Anbieter übertragen.')}</p></div><label>{t('Booking type', 'Buchungsart')}<select value={entryKind} disabled={busy} onChange={event => setEntryKind(event.target.value as 'credit' | 'debit')}><option value="credit">{t('Allocate credit', 'Guthaben zuteilen')}</option><option value="debit">{t('Withdraw unused credit', 'Ungenutztes Guthaben entziehen')}</option></select></label><label>{t('Amount', 'Betrag')} ({detail.currency})<input required inputMode="decimal" value={amount} disabled={busy} aria-invalid={!!amount && !validAmount} onChange={event => setAmount(event.target.value)} /></label><label>{t('Booking note', 'Buchungsnotiz')}<input required maxLength={500} value={reason} disabled={busy} onChange={event => setReason(event.target.value)} /></label>{amount && !validAmount && <p role="alert">{t('Enter a positive amount with at most six decimal places. Maximum: ', 'Gib einen positiven Betrag mit höchstens sechs Nachkommastellen ein. Maximum: ')}{money(maximumMicros)}</p>}{exceedsAvailable && <p role="alert">{t('Only unreserved credit can be withdrawn. Available: ', 'Nur unreserviertes Guthaben kann entzogen werden. Verfügbar: ')}{money(detail.available_micros)}</p>}<button disabled={busy || !validAmount || exceedsAvailable || !reason.trim()}>{entryKind === 'credit' ? t('Record credit allocation', 'Guthabenzuteilung buchen') : t('Record credit withdrawal', 'Guthabenentzug buchen')}</button></form></div> : <p className="enterprise-wallet-readonly"><Eye size={15} aria-hidden="true" />{t('Read-only access. An administrator manages credit and limits.', 'Lesezugriff. Ein Administrator verwaltet Guthaben und Limits.')}</p>}
    <section className="enterprise-wallet-ledger"><header className="enterprise-wallet-heading"><div><h3><History size={18} aria-hidden="true" />{t('Wallet ledger', 'Wallet-Buchungen')}</h3><p className="enterprise-muted">{t('Internal allocations, withdrawals and recorded provider usage.', 'Interne Zuteilungen, Entzüge und verbuchte Anbieternutzung.')}</p></div></header>{detail.entries.length ? <div className="enterprise-records" role="region" aria-label={t('Wallet ledger entries', 'Wallet-Buchungsliste')} tabIndex={0}><table><thead><tr>{[t('Time', 'Zeitpunkt'), t('Booking', 'Buchung'), t('Amount', 'Betrag'), t('Balance after booking', 'Kontostand danach'), t('Note', 'Notiz')].map(title => <th key={title} scope="col">{title}</th>)}</tr></thead><tbody>{detail.entries.map(entry => <tr key={entry.id}><td><time dateTime={entry.created_at}>{date(entry.created_at)}</time></td><td>{entry.kind === 'credit' ? t('Credit allocation', 'Guthabenzuteilung') : entry.kind === 'debit' ? t('Credit withdrawal', 'Guthabenentzug') : t('Model usage charge', 'Modellnutzung')}</td><td className="enterprise-wallet-money">{entry.kind === 'credit' ? '+' : '−'}{money(entry.amount_micros)}</td><td className="enterprise-wallet-money">{money(entry.balance_after_micros)}</td><td>{entry.reason}</td></tr>)}</tbody></table></div> : <p className="enterprise-muted">{t('No wallet entries recorded.', 'Noch keine Wallet-Buchungen vorhanden.')}</p>}{detail.has_more && <p className="enterprise-muted">{t('Only the latest entries are shown. Older entries remain in the server ledger.', 'Angezeigt werden die neuesten Buchungen. Ältere Einträge bleiben im Serverjournal gespeichert.')}</p>}</section>
    {detail.reservations.length > 0 && <section className="enterprise-wallet-ledger"><h3><LockKeyhole size={17} aria-hidden="true" />{t('Request reservations', 'Anfragereservierungen')}</h3><p className="enterprise-muted">{t('An unsettled provider request can keep its maximum cost reserved until reconciliation.', 'Eine noch ungeklärte Anbieteranfrage kann ihre maximalen Kosten bis zum Abgleich reserviert halten.')}</p><div className="enterprise-records" role="region" aria-label={t('Request reservations', 'Anfragereservierungen')} tabIndex={0}><table><thead><tr><th scope="col">{t('State', 'Status')}</th><th scope="col">{t('Maximum reserved', 'Maximal reserviert')}</th><th scope="col">{t('Charged', 'Verbucht')}</th><th scope="col">{t('Created', 'Erstellt')}</th></tr></thead><tbody>{detail.reservations.map(reservation => <tr key={reservation.id}><td>{reservation.state === 'reserved' ? t('Reserved', 'Reserviert') : reservation.state === 'held' ? t('Awaiting reconciliation', 'Abgleich ausstehend') : t('Settled', 'Abgerechnet')}</td><td className="enterprise-wallet-money">{money(reservation.maximum_micros)}</td><td className="enterprise-wallet-money">{reservation.charged_micros === null ? '—' : money(reservation.charged_micros)}</td><td>{date(reservation.created_at)}</td></tr>)}</tbody></table></div></section>}
    {canManage && detail.reservations.filter(reservation => reservation.state === 'held').map(reservation => <WalletHoldReview key={reservation.id} reservation={reservation} currency={detail.currency} revision={detail.revision} busy={busy} onResolve={body => void run(() => api.resolve(userId, reservation.id, body), t('Reservation reconciled with the verified provider cost.', 'Reservierung mit den geprüften Anbieterkosten abgeglichen.'))} />)}
    <p className="enterprise-wallet-readonly"><CircleHelp size={15} aria-hidden="true" />{t('Amounts are stored in millionths of the wallet currency. No currency conversion is performed. Entries are bookkeeping records, not invoices.', 'Beträge werden in Millionstel der Wallet-Währung gespeichert. Es findet keine Währungsumrechnung statt. Die Einträge sind Buchungsnachweise und keine Rechnungen.')}</p>
  </>
}

function WalletHoldReview({ reservation, currency, revision, busy, onResolve }: { currency: string; reservation: WalletDetail['reservations'][number]; revision: number; busy: boolean; onResolve: (body: WalletResolve) => void }) {
  const { i18n } = useTranslation(), t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const [actual, setActual] = useState(''), [reason, setReason] = useState(''), [confirmed, setConfirmed] = useState(false)
  const request = useRef<{ fingerprint: string; id: string } | null>(null)
  const micros = parseCurrencyMicros(actual)
  const valid = micros !== null && micros <= reservation.maximum_micros
  function submit(event: FormEvent) {
    event.preventDefault()
    if (busy || !valid || micros === null || !reason.trim() || !confirmed || reservation.state !== 'held') return
    const fingerprint = `${micros}:${reason.trim()}`
    if (request.current?.fingerprint !== fingerprint) request.current = { fingerprint, id: crypto.randomUUID() }
    onResolve({ request_id: request.current.id, expected_revision: revision, actual_micros: micros, reason: reason.trim() })
  }
  return <details className="enterprise-wallet-held-review"><summary><LockKeyhole size={14} aria-hidden="true" />{t('Advanced: reconcile held request', 'Erweitert: ungeklärte Anfrage abgleichen')} · <code>{reservation.id.slice(0, 8)}</code></summary><form onSubmit={submit}>
    <div><h4>{t('Verify the actual provider cost', 'Tatsächliche Anbieterkosten prüfen')}</h4><p>{t('Check the provider’s billing record for this request before entering the final cost. The verified amount is charged and only the unused reservation is released. No automatic refund is applied.', 'Prüfe den Abrechnungsnachweis des Anbieters für diese Anfrage, bevor du die endgültigen Kosten eingibst. Der geprüfte Betrag wird verbucht und nur die ungenutzte Reservierung freigegeben. Es erfolgt keine automatische Erstattung.')}</p><p>{t('Reserved maximum', 'Reserviertes Maximum')}: <strong>{formatCurrencyMicros(reservation.maximum_micros, currency, i18n.language)}</strong> · <code>{reservation.id}</code></p></div>
    <label>{t('Verified actual cost', 'Geprüfte tatsächliche Kosten')} ({currency})<input required inputMode="decimal" value={actual} disabled={busy} aria-invalid={!!actual && !valid} onChange={event => { setActual(event.target.value); setConfirmed(false) }} /><small>{t('Enter zero only if you verified that the provider charged nothing.', 'Gib nur dann null ein, wenn du geprüft hast, dass der Anbieter nichts berechnet hat.')}</small></label>
    <label>{t('Reconciliation note', 'Abgleichnotiz')}<input required maxLength={500} value={reason} disabled={busy} onChange={event => { setReason(event.target.value); setConfirmed(false) }} /></label>
    {actual && !valid && <p role="alert">{t('Enter an exact nonnegative cost no greater than the reservation maximum.', 'Gib exakte, nicht negative Kosten bis zum reservierten Maximum ein.')}</p>}
    <label className="enterprise-check"><input type="checkbox" checked={confirmed} disabled={busy} onChange={event => setConfirmed(event.target.checked)} />{t('I have checked the costs actually billed by the provider.', 'Ich habe tatsächlich abgerechnete Kosten geprüft.')}</label>
    <button disabled={busy || !valid || !reason.trim() || !confirmed}>{t('Record verified cost and reconcile', 'Geprüfte Kosten buchen und abgleichen')}</button>
  </form></details>
}
