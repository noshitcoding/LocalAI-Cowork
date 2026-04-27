import { useEffect, useState } from 'react'
import { usePipelineStore } from '../stores/pipelineStore'
import { useConfigStore } from '../stores/configStore'

type ActiveTab = 'pipelines' | 'gateway'

export default function PipelinePanel() {
  const {
    pipelines, toolGateway, loading, error, executing, lastResult,
    loadPipelines, upsertPipeline, deletePipeline,
    loadToolGateway, upsertToolGateway, deleteToolGateway, executePipeline,
  } = usePipelineStore()
  const ollama = useConfigStore((s) => s.ollama)

  const [tab, setTab] = useState<ActiveTab>('pipelines')

  // Pipeline form
  const [pName, setPName] = useState('')
  const [pSteps, setPSteps] = useState('')
  const [pDescription, setPDescription] = useState('')

  // Gateway form
  const [gToolType, setGToolType] = useState('')
  const [gName, setGName] = useState('')
  const [gConfig, setGConfig] = useState('')

  useEffect(() => {
    loadPipelines()
    loadToolGateway()
  }, [loadPipelines, loadToolGateway])

  const handleAddPipeline = async () => {
    if (!pName.trim() || !pSteps.trim()) return
    await upsertPipeline({
      id: crypto.randomUUID(),
      name: pName.trim(),
      stepsJson: pSteps.trim(),
      description: pDescription.trim() || undefined,
    })
    setPName(''); setPSteps(''); setPDescription('')
  }

  const handleAddGateway = async () => {
    if (!gToolType.trim() || !gName.trim() || !gConfig.trim()) return
    await upsertToolGateway({
      id: crypto.randomUUID(),
      toolType: gToolType.trim(),
      name: gName.trim(),
      configJson: gConfig.trim(),
      enabled: true,
    })
    setGToolType(''); setGName(''); setGConfig('')
  }

  const inputStyle: React.CSSProperties = {
    padding: '6px 10px', borderRadius: 'var(--radius-sm)',
    border: '1px solid var(--border-color)', background: 'var(--bg-secondary)', fontSize: 13, width: '100%',
  }

  return (
    <div className="panel">
      <h2>🔗 RPC-Pipelines &amp; Tool-Gateway</h2>

      {error && <p style={{ color: 'var(--danger)', fontSize: 12 }}>{error}</p>}

      <div style={{ display: 'flex', gap: 4, marginBottom: 12 }}>
        <button type="button" className={`btn-sm${tab === 'pipelines' ? ' active' : ''}`} onClick={() => setTab('pipelines')}>
          Pipelines ({pipelines.length})
        </button>
        <button type="button" className={`btn-sm${tab === 'gateway' ? ' active' : ''}`} onClick={() => setTab('gateway')}>
          Tool-Gateway ({toolGateway.length})
        </button>
      </div>

      {tab === 'pipelines' && (
        <>
          <div className="card" style={{ marginBottom: 12, display: 'flex', flexDirection: 'column', gap: 6 }}>
            <input placeholder="Pipeline-Name" value={pName} onChange={(e) => setPName(e.target.value)} style={inputStyle} />
            <textarea
              placeholder='Steps (JSON-Array, z.B. [{"tool":"grep","args":"..."},{"tool":"sed","args":"..."}])'
              value={pSteps} onChange={(e) => setPSteps(e.target.value)}
              rows={3}
              style={{ ...inputStyle, resize: 'vertical' }}
            />
            <input placeholder="Beschreibung (optional)" value={pDescription} onChange={(e) => setPDescription(e.target.value)} style={inputStyle} />
            <button type="button" className="btn-sm" onClick={handleAddPipeline}>Pipeline hinzufuegen</button>
          </div>

          {loading ? (
            <p className="panel-empty">Laden...</p>
          ) : pipelines.length === 0 ? (
            <p className="panel-empty">Keine Pipelines definiert</p>
          ) : (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              {pipelines.map((p) => (
                <div key={p.id} className="card" style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                  <div style={{ flex: 1 }}>
                    <strong>{p.name}</strong>
                    {p.description && <div style={{ fontSize: 12, color: 'var(--text-muted)' }}>{p.description}</div>}
                    <div style={{ fontSize: 11, color: 'var(--text-secondary)', marginTop: 2, fontFamily: 'monospace', maxWidth: 400, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                      {p.steps_json}
                    </div>
                  </div>
                  <div style={{ display: 'flex', gap: 6 }}>
                    <button type="button" className="btn-sm" onClick={() => void executePipeline(p.id, ollama.baseUrl, ollama.model)} disabled={executing === p.id}>
                      {executing === p.id ? 'Laeuft...' : 'Start'}
                    </button>
                    <button type="button" className="btn-sm" onClick={() => deletePipeline(p.id)} style={{ color: 'var(--danger)' }}>✕</button>
                  </div>
                </div>
              ))}
            </div>
          )}

          {lastResult && (
            <div className="card" style={{ marginTop: 12 }}>
              <strong>Letzte Ausfuehrung: {lastResult.pipelineId}</strong>
              <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 4 }}>Status: {lastResult.status}</div>
              {lastResult.error && <div style={{ fontSize: 12, color: 'var(--danger)', marginTop: 4 }}>{lastResult.error}</div>}
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginTop: 10 }}>
                {lastResult.stepResults.map((step) => (
                  <div key={`${step.step}-${step.tool}`} style={{ borderBottom: '1px solid var(--border-color)', paddingBottom: 8 }}>
                    <div style={{ display: 'flex', justifyContent: 'space-between', gap: 8 }}>
                      <strong>Schritt {step.step}: {step.tool}</strong>
                      <span>{step.success ? 'ok' : 'fehler'}</span>
                    </div>
                    <pre style={{ whiteSpace: 'pre-wrap', marginTop: 6, fontSize: 11 }}>{step.result.slice(0, 320)}</pre>
                  </div>
                ))}
              </div>
            </div>
          )}
        </>
      )}

      {tab === 'gateway' && (
        <>
          <div className="card" style={{ marginBottom: 12, display: 'flex', flexDirection: 'column', gap: 6 }}>
            <input placeholder="Tool-Typ (z.B. mcp, rest, grpc)" value={gToolType} onChange={(e) => setGToolType(e.target.value)} style={inputStyle} />
            <input placeholder="Name" value={gName} onChange={(e) => setGName(e.target.value)} style={inputStyle} />
            <textarea
              placeholder='Konfiguration (JSON, z.B. {"endpoint":"http://...","auth":"..."})'
              value={gConfig} onChange={(e) => setGConfig(e.target.value)}
              rows={3}
              style={{ ...inputStyle, resize: 'vertical' }}
            />
            <button type="button" className="btn-sm" onClick={handleAddGateway}>Gateway-Eintrag hinzufuegen</button>
          </div>

          {loading ? (
            <p className="panel-empty">Laden...</p>
          ) : toolGateway.length === 0 ? (
            <p className="panel-empty">Kein Tool-Gateway konfiguriert</p>
          ) : (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              {toolGateway.map((g) => (
                <div key={g.id} className="card" style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                  <div style={{ flex: 1 }}>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                      <strong>{g.name}</strong>
                      <span style={{ fontSize: 11, padding: '1px 6px', borderRadius: 8, background: g.enabled ? 'var(--success)' : 'var(--danger)', color: '#fff' }}>
                        {g.enabled ? 'aktiv' : 'inaktiv'}
                      </span>
                    </div>
                    <div style={{ fontSize: 12, color: 'var(--text-muted)' }}>{g.tool_type}</div>
                    <div style={{ fontSize: 11, fontFamily: 'monospace', color: 'var(--text-secondary)', maxWidth: 400, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{g.config_json}</div>
                  </div>
                  <button type="button" className="btn-sm" onClick={() => deleteToolGateway(g.id)} style={{ color: 'var(--danger)' }}>✕</button>
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  )
}
