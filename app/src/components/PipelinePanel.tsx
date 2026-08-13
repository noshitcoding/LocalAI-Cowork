import { useEffect, useState } from 'react'
import { Trash2 } from 'lucide-react'
import { usePipelineStore } from '../stores/pipelineStore'
import { useConfigStore } from '../stores/configStore'
import { tr } from '../i18n'

type ActiveTab = 'pipelines' | 'gateway'

export default function PipelinePanel() {
  const {
    pipelines, toolGateway, loading, error, executing, lastResult,
    loadPipelines, upsertPipeline, deletePipeline,
    loadToolGateway, upsertToolGateway, deleteToolGateway, executePipeline,
  } = usePipelineStore()
  const ollama = useConfigStore((s) => s.ollama)

  const [tab, setTab] = useState<ActiveTab>('pipelines')
  const [pName, setPName] = useState('')
  const [pSteps, setPSteps] = useState('')
  const [pDescription, setPDescription] = useState('')
  const [gToolType, setGToolType] = useState('')
  const [gName, setGName] = useState('')
  const [gConfig, setGConfig] = useState('')

  useEffect(() => {
    void loadPipelines()
    void loadToolGateway()
  }, [loadPipelines, loadToolGateway])

  const handleAddPipeline = async () => {
    if (!pName.trim() || !pSteps.trim()) return
    await upsertPipeline({
      id: crypto.randomUUID(),
      name: pName.trim(),
      stepsJson: pSteps.trim(),
      description: pDescription.trim() || undefined,
    })
    setPName('')
    setPSteps('')
    setPDescription('')
  }

  const handleAddGateway = async () => {
    if (!gToolType.trim() || !gName.trim() || !gConfig.trim()) return
    const saved = await upsertToolGateway({
      id: crypto.randomUUID(),
      toolType: gToolType.trim(),
      name: gName.trim(),
      configJson: gConfig.trim(),
      enabled: true,
    })
    if (!saved) return
    setGToolType('')
    setGName('')
    setGConfig('')
  }

  return (
    <div className="panel pipeline-panel">
      <h2>{tr("RPC pipelines and tool gateway")}</h2>

      {error && <p className="pipeline-error">{error}</p>}

      <div className="pipeline-tab-row" role="tablist" aria-label={tr("RPC pipelines and tool gateway")}>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'pipelines'}
          className={`btn-sm${tab === 'pipelines' ? ' active' : ''}`}
          onClick={() => setTab('pipelines')}
        >
          {tr("Pipelines (")}{pipelines.length})
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'gateway'}
          className={`btn-sm${tab === 'gateway' ? ' active' : ''}`}
          onClick={() => setTab('gateway')}
        >
          {tr("Tool-Gateway (")}{toolGateway.length})
        </button>
      </div>

      {tab === 'pipelines' && (
        <>
          <div className="card pipeline-form-card">
            <input
              className="pipeline-input"
              aria-label={tr("Pipeline name")}
              placeholder={tr("Pipeline name")}
              value={pName}
              onChange={(e) => setPName(e.target.value)}
            />
            <textarea
              className="pipeline-input pipeline-textarea"
              aria-label={tr("Steps")}
              placeholder={tr("Steps (JSON array, e.g. [{\"tool\":\"grep\",\"args\":\"...\"},{\"tool\":\"sed\",\"args\":\"...\"}])")}
              value={pSteps}
              onChange={(e) => setPSteps(e.target.value)}
              rows={3}
            />
            <input
              className="pipeline-input"
              aria-label={tr("Description (optional)")}
              placeholder={tr("Description (optional)")}
              value={pDescription}
              onChange={(e) => setPDescription(e.target.value)}
            />
            <button type="button" className="btn-sm" onClick={handleAddPipeline}>{tr("Add pipeline")}</button>
          </div>

          {loading ? (
            <p className="panel-empty">{tr("Loading...")}</p>
          ) : pipelines.length === 0 ? (
            <p className="panel-empty">{tr("No pipelines defined")}</p>
          ) : (
            <div className="pipeline-list">
              {pipelines.map((pipeline) => (
                <div key={pipeline.id} className="card pipeline-card">
                  <div className="pipeline-main">
                    <strong>{pipeline.name}</strong>
                    {pipeline.description && <div className="pipeline-description">{pipeline.description}</div>}
                    <div className="pipeline-code">{pipeline.steps_json}</div>
                  </div>
                  <div className="pipeline-actions">
                    <button type="button" className="btn-sm" onClick={() => void executePipeline(pipeline.id, ollama.baseUrl, ollama.model)} disabled={executing === pipeline.id}>
                      {executing === pipeline.id ? tr('Running...') : tr('Start')}
                    </button>
                    <button type="button" className="pipeline-danger-button" onClick={() => void deletePipeline(pipeline.id)} aria-label={tr("Delete pipeline")} title={tr("Delete pipeline")}>
                      <Trash2 size={14} aria-hidden="true" />
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )}

          {lastResult && (
            <div className="card pipeline-result-card">
              <strong>{tr("Letzte Execution:")}{lastResult.pipelineId}</strong>
              <div className="pipeline-result-meta">{tr("Status:")}{lastResult.status}</div>
              {lastResult.error && <div className="pipeline-result-error">{lastResult.error}</div>}
              <div className="pipeline-step-list">
                {lastResult.stepResults.map((step) => (
                  <div key={`${step.step}-${step.tool}`} className="pipeline-step">
                    <div className="pipeline-step-header">
                      <strong>{tr("Schritt")}{step.step}: {step.tool}</strong>
                      <span className={`pipeline-step-status ${step.success ? 'success' : 'error'}`}>
                        {step.success ? tr("Completed") : tr("Error")}
                      </span>
                    </div>
                    <pre className="pipeline-step-output">{step.result.slice(0, 320)}</pre>
                  </div>
                ))}
              </div>
            </div>
          )}
        </>
      )}

      {tab === 'gateway' && (
        <>
          <div className="card pipeline-form-card">
            <input
              className="pipeline-input"
              aria-label={tr("Tool type (e.g. mcp, rest, grpc)")}
              placeholder={tr("Tool type (e.g. mcp, rest, grpc)")}
              value={gToolType}
              onChange={(e) => setGToolType(e.target.value)}
            />
            <input
              className="pipeline-input"
              aria-label={tr("Name")}
              placeholder={tr("Name")}
              value={gName}
              onChange={(e) => setGName(e.target.value)}
            />
            <textarea
              className="pipeline-input pipeline-textarea"
              aria-label={tr("Protected configuration (JSON)")}
              placeholder={tr("Protected configuration (JSON)")}
              value={gConfig}
              onChange={(e) => setGConfig(e.target.value)}
              rows={3}
            />
            <button type="button" className="btn-sm" onClick={handleAddGateway}>{tr("Add gateway entry")}</button>
          </div>

          {loading ? (
            <p className="panel-empty">{tr("Loading...")}</p>
          ) : toolGateway.length === 0 ? (
            <p className="panel-empty">{tr("No Tool-Gateway configured")}</p>
          ) : (
            <div className="pipeline-list">
              {toolGateway.map((gateway) => (
                <div key={gateway.id} className="card pipeline-card">
                  <div className="pipeline-main">
                    <div className="pipeline-gateway-title">
                      <strong>{gateway.name}</strong>
                      <span className={`pipeline-gateway-status ${gateway.enabled ? 'active' : 'inactive'}`}>
                        {gateway.enabled ? tr("Active") : tr("Inactive")}
                      </span>
                    </div>
                    <div className="pipeline-description">{gateway.tool_type}</div>
                    <div className="pipeline-code">{tr("Protected configuration")}</div>
                  </div>
                  <button type="button" className="pipeline-danger-button" onClick={() => void deleteToolGateway(gateway.id)} aria-label={tr("Delete gateway entry")} title={tr("Delete gateway entry")}>
                    <Trash2 size={14} aria-hidden="true" />
                  </button>
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  )
}
