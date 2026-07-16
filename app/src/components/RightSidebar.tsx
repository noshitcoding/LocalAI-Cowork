import { useEffect, useMemo } from 'react'
import { ExternalLink, FileText, RefreshCw, Save } from 'lucide-react'
import { useUiStore } from '../stores/uiStore'
import { useConfigStore } from '../stores/configStore'
import { useCoworkStore } from '../stores/coworkStore'
import type { Task } from '../stores/taskStore'
import { useDocumentWorkspaceStore } from '../stores/documentWorkspaceStore'
import { getAttachmentPreviewSrc } from '../utils/chatAttachments'
import { tr } from '../i18n'

export function ProgressPanel({ task }: { task: Task | undefined }) {
  if (!task || task.steps.length === 0) {
    return (
      <div className="right-panel">
        <h3 className="right-panel-title">{tr("Progress")}</h3>
        <p className="panel-empty">{tr("No active task")}</p>
      </div>
    )
  }

  const completedCount = task.steps.filter((s) => s.state === 'completed').length
  const progress = Math.round((completedCount / task.steps.length) * 100)

  return (
    <div className="right-panel">
      <h3 className="right-panel-title">{tr("Progress")}</h3>
      <div className="progress-bar-wrapper">
        <progress className="progress-native" max={100} value={progress} aria-label={tr("Progress")} />
        <span className="progress-label">{progress}%</span>
      </div>
      <ul className="step-checklist">
        {task.steps.map((step) => (
          <li key={step.id} className={`step-check-item step-${step.state}`}>
            <span className="step-check-icon">
              {step.state === 'completed'
                ? 'OK'
                : step.state === 'running'
                  ? '...'
                  : step.state === 'failed'
                    ? 'X'
                    : '-'}
            </span>
            <span className="step-check-label">{step.title}</span>
          </li>
        ))}
      </ul>
    </div>
  )
}

export function WorkingFolderPanel() {
  const workingFolder = useUiStore((s) => s.workingFolder)
  const workingPathKind = useUiStore((s) => s.workingPathKind)

  return (
    <div className="right-panel">
      <h3 className="right-panel-title">{tr("Working folder")}</h3>
      {workingFolder ? (
        <div className="folder-display">
          <span className="folder-kind">
            {workingPathKind === 'file' ? tr('File') : tr('Folder')}
          </span>
          <span className="folder-path">{workingFolder}</span>
        </div>
      ) : (
        <p className="panel-empty">{tr("No folder selected")}</p>
      )}
    </div>
  )
}

export function OutputsPanel({ task }: { task: Task | undefined }) {
  const outputs = task?.steps.filter((s) => s.output) ?? []

  return (
    <div className="right-panel">
      <h3 className="right-panel-title">{tr("Outputs")}</h3>
      {outputs.length === 0 ? (
        <p className="panel-empty">{tr("No outputs yet")}</p>
      ) : (
        <ul className="output-list">
          {outputs.map((step) => (
            <li key={step.id} className="output-item">
              <span className="output-name">{step.title}</span>
              <pre className="output-preview">{step.output}</pre>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

export function ContextPanel() {
  const mcpServer = useConfigStore((s) => s.mcpServer)
  const ollama = useConfigStore((s) => s.ollama)
  const connectors = useCoworkStore((s) => s.connectors)
  const plugins = useCoworkStore((s) => s.plugins)
  const scheduledTasks = useCoworkStore((s) => s.scheduledTasks)

  const enabledConnectors = connectors.filter((entry) => entry.enabled).length
  const enabledPlugins = plugins.filter((entry) => entry.enabled).length
  const activeSchedules = scheduledTasks.filter((entry) => entry.active).length

  return (
    <div className="right-panel">
      <h3 className="right-panel-title">{tr("Context")}</h3>
      <div className="context-items">
        <div className="context-item">
          <span className="context-label">{tr("Model")}</span>
          <span className="context-value">{ollama.model}</span>
        </div>
        <div className="context-item">
          <span className="context-label">{tr("MCP Server")}</span>
          <span className="context-value">{mcpServer.name}</span>
        </div>
        <div className="context-item">
          <span className="context-label">{tr("Endpoint")}</span>
          <span className="context-value">{ollama.baseUrl}</span>
        </div>
        <div className="context-item">
          <span className="context-label">{tr("Connectors")}</span>
          <span className="context-value">{enabledConnectors} {tr("active")}</span>
        </div>
        <div className="context-item">
          <span className="context-label">{tr("Plugins")}</span>
          <span className="context-value">{enabledPlugins} {tr("active")}</span>
        </div>
        <div className="context-item">
          <span className="context-label">{tr("Scheduled tasks")}</span>
          <span className="context-value">{activeSchedules} {tr("active")}</span>
        </div>
      </div>
    </div>
  )
}

export function DocumentWorkspacePanel() {
  const documents = useDocumentWorkspaceStore((s) => s.documents)
  const activePath = useDocumentWorkspaceStore((s) => s.activePath)
  const officeApps = useDocumentWorkspaceStore((s) => s.officeApps)
  const officeWarnings = useDocumentWorkspaceStore((s) => s.officeWarnings)
  const busy = useDocumentWorkspaceStore((s) => s.busy)
  const detectOfficeApps = useDocumentWorkspaceStore((s) => s.detectOfficeApps)
  const setActiveDocument = useDocumentWorkspaceStore((s) => s.setActiveDocument)
  const renderPreview = useDocumentWorkspaceStore((s) => s.renderPreview)
  const openDocument = useDocumentWorkspaceStore((s) => s.openDocument)
  const saveVersion = useDocumentWorkspaceStore((s) => s.saveVersion)

  useEffect(() => {
    void detectOfficeApps()
  }, [detectOfficeApps])

  const activeDocument = useMemo(
    () => documents.find((document) => document.path === activePath) ?? documents[0] ?? null,
    [activePath, documents],
  )
  const availableOfficeApps = officeApps.filter((app) => app.available)
  const officeStatus = availableOfficeApps.length > 0
    ? `Office: ${availableOfficeApps.map((app) => app.displayName || app.kind).join(', ')}`
    : officeWarnings[0] ?? 'Office not detected yet'

  const runPreview = () => {
    if (!activeDocument) return
    void renderPreview(activeDocument.path).catch(() => undefined)
  }

  const runOpen = () => {
    if (!activeDocument) return
    void openDocument(activeDocument.path)
  }

  const runSaveVersion = () => {
    if (!activeDocument) return
    void saveVersion(activeDocument.path)
  }

  const locked = busy || (activeDocument?.status ?? 'idle') !== 'idle'

  return (
    <div className="right-panel document-workspace">
      <h3 className="right-panel-title">{tr("Documents")}</h3>
      {documents.length === 0 ? (
        <>
          <p className="panel-empty">{tr("No active document yet")}</p>
          <p className="document-office-status">{officeStatus}</p>
        </>
      ) : (
        <>
          <div className="document-tabs" role="tablist" aria-label={tr("Active documents")}>
            {documents.map((document) => (
              <button
                type="button"
                key={document.path}
                role="tab"
                aria-selected={document.path === activeDocument?.path}
                className={`document-tab${document.path === activeDocument?.path ? ' active' : ''}`}
                onClick={() => setActiveDocument(document.path)}
                title={document.path}
              >
                <FileText size={14} aria-hidden="true" />
                <span>{document.label}</span>
              </button>
            ))}
          </div>

          {activeDocument && (
            <div className="document-active">
              <div className="document-meta">
                <span className="document-format">{activeDocument.format.toUpperCase()}</span>
                <span className="document-name" title={activeDocument.path}>{activeDocument.path}</span>
              </div>
              <div className="document-actions">
                <button type="button" onClick={runOpen} disabled={locked} title={tr("Open in Office")} aria-label={tr("Open document in Office")}>
                  <ExternalLink size={14} aria-hidden="true" />
                </button>
                <button type="button" onClick={runPreview} disabled={locked} title={tr("Refresh preview")} aria-label={tr("Refresh document preview")}>
                  <RefreshCw size={14} aria-hidden="true" />
                </button>
                <button type="button" onClick={runSaveVersion} disabled={locked} title={tr("Save version")} aria-label={tr("Save document version")}>
                  <Save size={14} aria-hidden="true" />
                </button>
              </div>
              <p className="document-status">
                {activeDocument.status !== 'idle' ? activeDocument.status : activeDocument.lastAction ?? tr('Ready')}
              </p>
              <p className="document-office-status">{officeStatus}</p>
              {activeDocument.error && <p className="document-error">{activeDocument.error}</p>}
              {activeDocument.preview?.warnings.map((warning) => (
                <p key={warning} className="document-warning">{warning}</p>
              ))}
              {activeDocument.preview?.pages.length ? (
                <div className="document-preview-strip">
                  {activeDocument.preview.pages.map((page) => (
                    <figure key={`${activeDocument.path}-${page.pageNumber}`} className="document-preview-page">
                      <img src={getAttachmentPreviewSrc(page.imagePath)} alt={`${activeDocument.label} Seite ${page.pageNumber}`} />
                      <figcaption>{tr("Seite")}{page.pageNumber}</figcaption>
                    </figure>
                  ))}
                </div>
              ) : (
                <button type="button" className="document-preview-empty" onClick={runPreview} disabled={locked}>{tr("Create preview")}</button>
              )}
            </div>
          )}
        </>
      )}
    </div>
  )
}
