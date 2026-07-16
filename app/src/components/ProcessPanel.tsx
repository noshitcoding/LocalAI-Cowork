import { useEffect, useState } from 'react'
import { useProcessStore, type ProcessStatusResult } from '../stores/processStore'
import { tr } from '../i18n'

export default function ProcessPanel() {
  const { processes, loading, error, loadProcesses, startProcess, stopProcess, approveProcess } = useProcessStore()
  const [label, setLabel] = useState('')
  const [command, setCommand] = useState('')
  const [showForm, setShowForm] = useState(false)

  useEffect(() => {
    void loadProcesses()
    const interval = window.setInterval(() => void loadProcesses(), 5000)
    return () => window.clearInterval(interval)
  }, [loadProcesses])

  const handleStart = async () => {
    if (!label.trim() || !command.trim()) return
    await startProcess(label.trim(), command.trim())
    setLabel('')
    setCommand('')
    setShowForm(false)
    void loadProcesses()
  }

  const statusLabel = (status: string) => {
    switch (status) {
      case 'running': return tr('Running')
      case 'stopped': return tr('Stopped')
      case 'failed': return tr('Failed')
      case 'pending_approval': return tr('Waiting for approval')
      default: return status
    }
  }

  const handleApproval = async (processId: string, approved: boolean) => {
    await approveProcess(processId, approved)
    void loadProcesses()
  }

  const handleStop = async (processId: string) => {
    await stopProcess(processId)
    void loadProcesses()
  }

  return (
    <div className="panel process-panel">
      <div className="panel-heading-row">
        <h2>{tr("Processes")}</h2>
        <button type="button" className="btn-sm" onClick={() => setShowForm(!showForm)}>{tr("Start")}</button>
      </div>

      {error && <p className="process-panel-error">{error}</p>}

      {showForm && (
        <div className="card process-form-card">
          <div className="grid process-form-grid">
            <label>{tr("Label")}<input type="text" value={label} onChange={(e) => setLabel(e.target.value)} placeholder={tr("e.g. Dev Server")} />
            </label>
            <label>{tr("Command")}<input type="text" value={command} onChange={(e) => setCommand(e.target.value)} placeholder={tr("e.g. npm run dev")} />
            </label>
          </div>
          <button type="button" className="btn-sm" onClick={handleStart}>{tr("Start process")}</button>
        </div>
      )}

      {loading ? (
        <p className="panel-empty">{tr("Loading...")}</p>
      ) : processes.length === 0 ? (
        <p className="panel-empty">{tr("No active Processes")}</p>
      ) : (
        <div className="process-list">
          {processes.map((process: ProcessStatusResult) => (
            <div key={process.processId} className="card process-card">
              <div className="process-main">
                <div className="process-label">{process.label}</div>
                <div className="process-command">{process.command}</div>
                <div className="process-meta">
                  <span className={`process-status status-${process.status}`}>{statusLabel(process.status)}</span>
                  {process.pid != null && <span className="process-muted-meta">{tr("PID:")}{process.pid}</span>}
                  {process.requiresAdmin && <span className="process-admin-meta">{tr("Admin")}</span>}
                </div>
              </div>
              <div className="process-actions">
                {process.status === 'pending_approval' && (
                  <>
                    <button type="button" className="btn-sm process-approve" onClick={() => void handleApproval(process.processId, true)} aria-label={tr("Approve")}>{tr("Approve")}</button>
                    <button type="button" className="btn-sm process-reject" onClick={() => void handleApproval(process.processId, false)} aria-label={tr("Reject")}>{tr("Reject")}</button>
                  </>
                )}
                {process.status === 'running' && (
                  <button type="button" className="btn-sm process-reject" onClick={() => void handleStop(process.processId)}>{tr("Stop")}</button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
