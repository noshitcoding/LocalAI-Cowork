import { useCoworkStore } from '../stores/coworkStore'
import i18n, { tr } from '../i18n'
import SecureCredentialInput from './SecureCredentialInput'

export default function ConnectorPanel() {
  const connectors = useCoworkStore((s) => s.connectors)
  const toggleConnector = useCoworkStore((s) => s.toggleConnector)
  const setConnectorNote = useCoworkStore((s) => s.setConnectorNote)
  const setConnectorApiKey = useCoworkStore((s) => s.setConnectorApiKey)
  const setConnectorWebhookUrl = useCoworkStore((s) => s.setConnectorWebhookUrl)
  const testConnector = useCoworkStore((s) => s.testConnector)

  return (
    <div className="panel connector-panel">
      <h2>{tr("Connectors")}</h2>
      <p className="hint-text">{tr("Connector credentials are protected by the operating-system credential store.")}</p>

      <div className="connector-list">
        {connectors.map((connector) => {
          return (
            <div key={connector.key} className="card connector-card">
              <div className="connector-card-header">
                <div>
                  <strong>{connector.label}</strong>
                  <div className="connector-key">{tr("Key:")}{connector.key}
                  </div>
                </div>
                <button
                  type="button"
                  className="btn-sm"
                  onClick={() => toggleConnector(connector.key, !connector.enabled)}
                >
                  {connector.enabled ? tr('Disable') : tr('Enable')}
                </button>
              </div>

              <label>{tr("Webhook / API URL")}<SecureCredentialInput
                  type="url"
                  className="connector-input"
                  value={connector.webhookUrl ?? ''}
                  onCommit={(value) => setConnectorWebhookUrl(connector.key, value)}
                  placeholder={tr("https://example.com/webhook")}
                />
              </label>

              <label>{tr("API Key")}<SecureCredentialInput
                  className="connector-input"
                  value={connector.apiKey ?? ''}
                  onCommit={(value) => setConnectorApiKey(connector.key, value)}
                  placeholder={tr("Optional bearer token")}
                />
              </label>

              <label>{tr("Note")}<textarea
                  className="connector-input connector-textarea"
                  rows={2}
                  value={connector.note}
                  onChange={(event) => setConnectorNote(connector.key, event.target.value)}
                  placeholder={tr("Internal notes or setup comments")}
                />
              </label>

              <div className="connector-test-row">
                <div className={`connector-test-status ${connector.lastTestStatus ?? 'idle'}`}>
                  {connector.lastTestMessage ?? tr('No connection test has run yet.')}
                  {connector.lastTestAt ? ` (${new Date(connector.lastTestAt).toLocaleString(i18n.resolvedLanguage ?? i18n.language ?? 'en')})` : ''}
                </div>
                <button
                  type="button"
                  className="btn-sm"
                  onClick={() => void testConnector(connector.key)}
                  disabled={connector.lastTestStatus === 'testing'}
                >
                  {connector.lastTestStatus === 'testing' ? tr('Testing...') : tr('Test connection')}
                </button>
              </div>
            </div>
          )
        })}
      </div>
    </div>
  )
}
