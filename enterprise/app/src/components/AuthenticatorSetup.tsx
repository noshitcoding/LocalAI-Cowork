import { Check, Copy, KeyRound, Smartphone } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toDataURL } from 'qrcode'
import './AuthenticatorSetup.css'

type Props = { uri: string; secret: string; expiresAt: string; onRestart?: () => void; busy?: boolean }

/** QR generation stays in this client. The setup URI is never sent to a service. */
export default function AuthenticatorSetup({ uri, secret, expiresAt, onRestart, busy = false }: Props) {
  const { i18n } = useTranslation()
  const t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const secretId = useId()
  const [qr, setQr] = useState<{ uri: string; value: string } | null>(null)
  const [qrError, setQrError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const [copyError, setCopyError] = useState(false)
  const [expiredFor, setExpiredFor] = useState<string | null>(null)
  const current = useRef<object | null>(null)
  const expiry = Date.parse(expiresAt)
  const expired = expiredFor === uri || !Number.isFinite(expiry) || expiry <= Date.now()

  useEffect(() => {
    setExpiredFor(null)
    if (!Number.isFinite(expiry) || expiry <= Date.now()) return
    const timer = window.setTimeout(() => { if (Date.now() >= expiry) setExpiredFor(uri) }, Math.min(expiry - Date.now(), 2_147_483_647))
    return () => window.clearTimeout(timer)
  }, [uri, expiry])

  useEffect(() => {
    const request = {}
    current.current = request
    setQrError(null); setCopied(false); setCopyError(false)
    if (!Number.isFinite(expiry) || expiry <= Date.now()) return () => { current.current = null }
    void toDataURL(uri, { width: 224, margin: 4, errorCorrectionLevel: 'M', color: { dark: '#191919', light: '#ffffff' } })
      .then(value => { if (current.current === request) setQr({ uri, value }) })
      .catch(() => { if (current.current === request) setQrError(uri) })
    return () => { current.current = null }
  }, [uri, expiry])

  const copy = async () => {
    const request = current.current
    try {
      await navigator.clipboard.writeText(secret)
      if (request && current.current === request) { setCopied(true); setCopyError(false) }
    } catch { if (request && current.current === request) setCopyError(true) }
  }

  if (expired) return <div className="authenticator-setup"><p role="status">{t('This setup has expired. Start again to get a new QR code.', 'Diese Einrichtung ist abgelaufen. Starte erneut, um einen neuen QR-Code zu erhalten.')}</p>{onRestart && <button className="ui-button ui-button--secondary ui-button--lg" type="button" disabled={busy} onClick={onRestart}>{t('Restart setup', 'Einrichtung neu starten')}</button>}</div>

  return <div className="authenticator-setup">
    <ol className="authenticator-setup-steps">
      <li>
        <div className="authenticator-setup-step-title"><Smartphone size={16} aria-hidden="true" /><strong>{t('Open your authenticator app', 'Authenticator-App öffnen')}</strong></div>
        <p>{t('In Microsoft Authenticator, tap + and choose “Other account”. Other authenticator apps work too.', 'Tippe in Microsoft Authenticator auf + und wähle „Anderes Konto“. Andere Authenticator-Apps funktionieren ebenfalls.')}</p>
      </li>
      <li>
        <strong>{t('Scan this QR code', 'Diesen QR-Code scannen')}</strong>
        <div className="authenticator-setup-qr">
          {qr?.uri === uri ? <img src={qr.value} width="224" height="224" alt={t('QR code for your authenticator app', 'QR-Code für deine Authenticator-App')} />
            : <p role="status">{qrError === uri ? t('Use the setup key below.', 'Verwende den Einrichtungsschlüssel unten.') : t('Preparing QR code…', 'QR-Code wird erstellt…')}</p>}
        </div>
        <details className="authenticator-setup-manual" open={qrError === uri || undefined}>
          <summary><KeyRound size={14} aria-hidden="true" />{t('Or enter the setup key', 'Oder den Einrichtungsschlüssel eingeben')}</summary>
          <label htmlFor={secretId}>{t('Authenticator secret', 'Authenticator-Schlüssel')}</label>
          <div className="authenticator-setup-key">
            <input id={secretId} readOnly value={secret} spellCheck={false} autoComplete="off" onFocus={event => event.target.select()} />
            <button type="button" className="ui-button ui-button--secondary ui-button--sm" onClick={() => { void copy() }} aria-label={t('Copy setup key', 'Einrichtungsschlüssel kopieren')}>
              {copied ? <Check size={15} aria-hidden="true" /> : <Copy size={15} aria-hidden="true" />}
              {copied ? t('Copied', 'Kopiert') : t('Copy', 'Kopieren')}
            </button>
          </div>
          {copyError && <p role="alert">{t('Copying failed. Select the key and copy it manually.', 'Kopieren fehlgeschlagen. Markiere den Schlüssel und kopiere ihn manuell.')}</p>}
        </details>
      </li>
      <li><strong>{t('Confirm the six-digit code', 'Sechsstelligen Code bestätigen')}</strong><p>{t('Enter the current code from your app below to finish setup.', 'Gib unten den aktuellen Code aus deiner App ein, um die Einrichtung abzuschließen.')}</p></li>
    </ol>
    <p className="authenticator-setup-note">{t('App codes also work on localhost. You do not need a domain.', 'App-Codes funktionieren auch auf localhost. Du brauchst dafür keine Domain.')}</p>
    <p className="authenticator-setup-expiry">{t('Complete setup by', 'Einrichtung abschließen bis')} <time dateTime={expiresAt}>{new Date(expiresAt).toLocaleString(i18n.language)}</time>.</p>
  </div>
}
