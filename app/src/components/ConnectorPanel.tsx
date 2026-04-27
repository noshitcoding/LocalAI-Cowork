import type { CSSProperties } from 'react'
import { useCoworkStore } from '../stores/coworkStore'

export default function ConnectorPanel() {
  const connectors = useCoworkStore((s) => s.connectors)
  const toggleConnector = useCoworkStore((s) => s.toggleConnector)
  const setConnectorNote = useCoworkStore((s) => s.setConnectorNote)
  const updateConnectorConfig = useCoworkStore((s) => s.updateConnectorConfig)
  const testConnector = useCoworkStore((s) => s.testConnector)

  const inputStyle: CSSProperties = {
    padding: '6px 10px',
    borderRadius: 'var(--radius-sm)',
    border: '1px solid var(--border-color)',
    background: 'var(--bg-secondary)',
    fontSize: 13,
    width: '100%',
  }

  return (
    <div className="panel">
      <h2>Connectors</h2>
      <p className="hint-text">Webhook-URL und API-Key werden lokal gespeichert. Der Verbindungstest laeuft ueber das Rust-Backend, um Webview-CORS zu umgehen.</p>

      <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
        {connectors.map((connector) => {
          const statusColor = connector.lastTestStatus === 'success'
            ? 'var(--success)'
            : connector.lastTestStatus === 'error'
              ? 'var(--danger)'
              : 'var(--text-muted)'

          return (
            <div key={connector.key} className="card" style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
              <div style={{ display: 'flex', justifyContent: 'space-between', gap: 12, alignItems: 'center' }}>
                <div>
                  <strong>{connector.label}</strong>
                  <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 2 }}>
                    Key: {connector.key}
                  </div>
                </div>
                <button
                  type="button"
                  className="btn-sm"
                  onClick={() => toggleConnector(connector.key, !connector.enabled)}
                >
                  {connector.enabled ? 'Deaktivieren' : 'Aktivieren'}
                </button>
              </div>

              <label>
                Webhook / API URL
                <input
                  value={connector.webhookUrl ?? ''}
                  onChange={(event) => updateConnectorConfig(connector.key, { webhookUrl: event.target.value })}
                  placeholder="https://example.com/webhook"
                  style={inputStyle}
                />
              </label>

              <label>
                API Key
                <input
                  value={connector.apiKey ?? ''}
                  onChange={(event) => updateConnectorConfig(connector.key, { apiKey: event.target.value })}
                  placeholder="Optionaler Bearer Token"
                  style={inputStyle}
                />
              </label>

              <label>
                Notiz
                <textarea
                  rows={2}
                  value={connector.note}
                  onChange={(event) => setConnectorNote(connector.key, event.target.value)}
                  placeholder="Interne Hinweise oder Setup-Kommentare"
                  style={{ ...inputStyle, resize: 'vertical' }}
                />
              </label>

              <div style={{ display: 'flex', justifyContent: 'space-between', gap: 12, alignItems: 'center' }}>
                <div style={{ fontSize: 12, color: statusColor }}>
                  {connector.lastTestMessage ?? 'Noch kein Verbindungstest ausgefuehrt.'}
                  {connector.lastTestAt ? ` (${new Date(connector.lastTestAt).toLocaleString('de-DE')})` : ''}
                </div>
                <button
                  type="button"
                  className="btn-sm"
                  onClick={() => void testConnector(connector.key)}
                  disabled={connector.lastTestStatus === 'testing'}
                >
                  {connector.lastTestStatus === 'testing' ? 'Teste...' : 'Verbindung testen'}
                </button>
              </div>
            </div>
          )
        })}
      </div>
    </div>
  )
}
