import { useCallback, useEffect, useState } from 'react'
import { Activity, Check, Circle, Clock3, ExternalLink, FileOutput, PanelRightClose, ShieldAlert, Square, Wrench, XCircle } from 'lucide-react'
import type { EngineStatus, ContextWarning } from '../stores/engineStore'
import type { LiveToolCall } from '../stores/chatStore'
import type { Task } from '../stores/taskStore'
import { safeInvoke } from '../utils/safeInvoke'
import {
  normalizeEngineRunArtifact,
  normalizeEngineRunEvent,
  type EngineRunArtifactRow,
  type EngineRunEventRow,
} from '../utils/engineRunRecords'
import { tr } from '../i18n'

type CoworkContextRailProps = {
  open: boolean
  engineStatus: EngineStatus
  error: string | null
  sessionId: string | null
  runId: string | null
  providerLabel: string
  model: string
  workingContext: string | null
  contextWarning: ContextWarning
  compactionCount: number
  approvalSteps: string[]
  toolCalls: LiveToolCall[]
  task: Task | null
  onClose: () => void
  onStop: () => void
  onOpenRuns: () => void
  onOpenTasks: () => void
}

const STATUS_LABELS: Record<EngineStatus, string> = {
  idle: 'Ready',
  streaming: 'Responding',
  tool_running: 'Using tools',
  waiting_approval: 'Needs approval',
  error: 'Action needed',
}

const OPENABLE_ARTIFACT_PATTERN = /\.(?:pdf|docx|pptx|xlsx)$/i

function canOpenArtifact(artifact: EngineRunArtifactRow): boolean {
  return Boolean(artifact.path.trim()) && OPENABLE_ARTIFACT_PATTERN.test(artifact.path.trim())
}

function toolStatusIcon(status: LiveToolCall['status']) {
  if (status === 'completed') return <Check size={13} aria-hidden="true" />
  if (status === 'failed') return <XCircle size={13} aria-hidden="true" />
  if (status === 'approval' || status === 'waiting_input') return <ShieldAlert size={13} aria-hidden="true" />
  return <Clock3 size={13} aria-hidden="true" />
}

export default function CoworkContextRail({
  open,
  engineStatus,
  error,
  sessionId,
  runId,
  providerLabel,
  model,
  workingContext,
  contextWarning,
  compactionCount,
  approvalSteps,
  toolCalls,
  task,
  onClose,
  onStop,
  onOpenRuns,
  onOpenTasks,
}: CoworkContextRailProps) {
  const [events, setEvents] = useState<EngineRunEventRow[]>([])
  const [artifacts, setArtifacts] = useState<EngineRunArtifactRow[]>([])
  const [evidenceLoading, setEvidenceLoading] = useState(false)
  const [openingArtifactId, setOpeningArtifactId] = useState<string | null>(null)
  const [artifactOpenError, setArtifactOpenError] = useState<{ id: string; message: string } | null>(null)
  const taskSteps = task?.steps ?? []
  const completedSteps = taskSteps.filter((step) => step.state === 'completed').length
  const outputs = taskSteps.filter((step) => Boolean(step.output))
  const active = engineStatus !== 'idle' && engineStatus !== 'error'
  const refreshEvidence = useCallback(async () => {
    if (!runId) {
      setEvents([])
      setArtifacts([])
      return
    }

    setEvidenceLoading(true)
    const [eventResult, artifactResult] = await Promise.allSettled([
      safeInvoke<unknown[]>('engine_run_event_list', { runId, limit: 12 }, []),
      safeInvoke<unknown[]>('engine_run_artifact_list', { runId, limit: 12 }, []),
    ])
    if (eventResult.status === 'fulfilled') {
      setEvents(eventResult.value.map(normalizeEngineRunEvent).filter((event): event is EngineRunEventRow => event !== null).slice(-6).reverse())
    }
    if (artifactResult.status === 'fulfilled') {
      setArtifacts(artifactResult.value.map(normalizeEngineRunArtifact).filter((artifact): artifact is EngineRunArtifactRow => artifact !== null).slice(-6).reverse())
    }
    setEvidenceLoading(false)
  }, [runId])

  const openArtifact = async (artifact: EngineRunArtifactRow) => {
    if (!runId || !canOpenArtifact(artifact) || openingArtifactId) return
    setOpeningArtifactId(artifact.id)
    setArtifactOpenError(null)
    try {
      await safeInvoke('office_open_document', {
        request: { path: artifact.path },
        runId,
      })
    } catch (openError) {
      setArtifactOpenError({
        id: artifact.id,
        message: openError instanceof Error ? openError.message : tr('Output could not be opened.'),
      })
    } finally {
      setOpeningArtifactId(null)
    }
  }

  useEffect(() => {
    let cancelled = false
    window.queueMicrotask(() => {
      if (!cancelled) void refreshEvidence()
    })
    return () => { cancelled = true }
  }, [engineStatus, refreshEvidence])

  useEffect(() => {
    if (!open || !runId || !active) return
    const intervalId = window.setInterval(() => void refreshEvidence(), 2500)
    return () => window.clearInterval(intervalId)
  }, [active, open, refreshEvidence, runId])

  return (
    <aside id="cowork-context-rail" className={`cowork-context-rail${open ? ' open' : ''}`} aria-label={tr('Run context')} aria-hidden={!open}>
      <header className="context-rail-header">
        <div>
          <span className="context-rail-kicker">{tr('Live workspace')}</span>
          <h2>{tr('Run context')}</h2>
        </div>
        <button type="button" className="context-rail-close" onClick={onClose} aria-label={tr('Close run context')}>
          <PanelRightClose size={18} aria-hidden="true" />
        </button>
      </header>

      <div className={`context-rail-state state-${engineStatus}`} role="status">
        <span className="context-rail-state-dot" aria-hidden="true" />
        <div>
          <strong>{tr(STATUS_LABELS[engineStatus])}</strong>
          <span>{error || (runId ? `${tr('Run')} ${runId.slice(0, 8)}` : tr('No active run'))}</span>
        </div>
        {active && (
          <button type="button" onClick={onStop} className="context-rail-stop">
            <Square size={12} fill="currentColor" aria-hidden="true" />{tr('Stop')}
          </button>
        )}
      </div>

      <div className="context-rail-scroll">
        <section className="context-rail-section" aria-labelledby="context-rail-environment">
          <div className="context-rail-section-title">
            <Activity size={15} aria-hidden="true" />
            <h3 id="context-rail-environment">{tr('Environment')}</h3>
          </div>
          <dl className="context-rail-facts">
            <div><dt>{tr('Provider')}</dt><dd>{providerLabel}</dd></div>
            <div><dt>{tr('Model')}</dt><dd title={model}>{model || tr('not set')}</dd></div>
            <div><dt>{tr('Working context')}</dt><dd title={workingContext ?? undefined}>{workingContext || tr('No folder connected')}</dd></div>
            <div><dt>{tr('Session')}</dt><dd>{sessionId ? sessionId.slice(0, 10) : tr('Not saved')}</dd></div>
          </dl>
          <div className={`context-rail-context-meter level-${contextWarning.level}`}>
            <span>{tr('Context health')}</span>
            <strong>{contextWarning.level === 'none' ? tr('Stable') : tr(contextWarning.level)}</strong>
            <small>{contextWarning.estimatedTokens > 0 ? `${contextWarning.estimatedTokens} ${tr('tokens')}` : `${compactionCount} ${tr('compactions')}`}</small>
          </div>
        </section>

        <section className="context-rail-section" aria-labelledby="context-rail-plan">
          <div className="context-rail-section-title">
            <ShieldAlert size={15} aria-hidden="true" />
            <h3 id="context-rail-plan">{tr('Plan & approvals')}</h3>
            {approvalSteps.length > 0 && <span className="context-rail-count attention">{approvalSteps.length}</span>}
          </div>
          {approvalSteps.length > 0 ? (
            <div className="context-rail-approval">
              <strong>{tr('Your decision is required')}</strong>
              <ol>{approvalSteps.slice(0, 3).map((step) => <li key={step}>{step}</li>)}</ol>
            </div>
          ) : taskSteps.length > 0 ? (
            <div className="context-rail-plan">
              <div className="context-rail-progress-copy">
                <strong>{task?.title}</strong>
                <span>{completedSteps}/{taskSteps.length}</span>
              </div>
              <progress value={completedSteps} max={taskSteps.length} aria-label={tr('Task progress')} />
              <ul>
                {taskSteps.slice(0, 4).map((step) => (
                  <li key={step.id} className={`step-${step.state}`}>
                    {step.state === 'completed' ? <Check size={12} aria-hidden="true" /> : <Circle size={9} aria-hidden="true" />}
                    <span>{step.title}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : (
            <p className="context-rail-empty">{tr('A plan appears here when the task needs multiple steps or approval.')}</p>
          )}
          <button type="button" className="context-rail-link" onClick={onOpenTasks}>{tr('Open tasks')}<ExternalLink size={12} aria-hidden="true" /></button>
        </section>

        <section className="context-rail-section" aria-labelledby="context-rail-activity">
          <div className="context-rail-section-title">
            <Wrench size={15} aria-hidden="true" />
            <h3 id="context-rail-activity">{tr('Tool activity')}</h3>
            {toolCalls.length + events.length > 0 && <span className="context-rail-count">{toolCalls.length + events.length}</span>}
          </div>
          {toolCalls.length === 0 && events.length === 0 ? (
            <p className="context-rail-empty">{tr('Tool calls will appear here with their live status.')}</p>
          ) : toolCalls.length > 0 ? (
            <ul className="context-rail-tools">
              {toolCalls.slice(0, 5).map((call) => (
                <li key={call.id} className={`status-${call.status}`}>
                  <span>{toolStatusIcon(call.status)}</span>
                  <div><strong>{call.toolName}</strong><small>{tr(call.status)}</small></div>
                </li>
              ))}
            </ul>
          ) : null}
          {events.length > 0 && (
            <ul className="context-rail-events">
              {events.slice(0, 5).map((event) => (
                <li key={event.id}>
                  <strong>{event.summary}</strong>
                  <span>#{event.sequence} · {event.eventType}</span>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="context-rail-section" aria-labelledby="context-rail-outputs">
          <div className="context-rail-section-title">
            <FileOutput size={15} aria-hidden="true" />
            <h3 id="context-rail-outputs">{tr('Outputs')}</h3>
            {artifacts.length + outputs.length > 0 && <span className="context-rail-count success">{artifacts.length + outputs.length}</span>}
          </div>
          {evidenceLoading && artifacts.length === 0 && <p className="context-rail-empty">{tr('Loading...')}</p>}
          {artifacts.length > 0 && (
            <ul className="context-rail-events">
              {artifacts.slice(0, 4).map((artifact) => (
                <li key={artifact.id} title={artifact.path} className={canOpenArtifact(artifact) ? 'actionable' : undefined}>
                  {canOpenArtifact(artifact) ? (
                    <button
                      type="button"
                      className="context-rail-artifact-action"
                      onClick={() => void openArtifact(artifact)}
                      disabled={openingArtifactId !== null}
                      aria-label={`${tr('Open output')}: ${artifact.title ?? artifact.kind}`}
                    >
                      <span>
                        <strong>{artifact.title ?? artifact.kind}</strong>
                        <small>{artifact.summary || artifact.path}</small>
                      </span>
                      {openingArtifactId === artifact.id
                        ? <Clock3 size={13} aria-hidden="true" />
                        : <ExternalLink size={13} aria-hidden="true" />}
                    </button>
                  ) : (
                    <>
                      <strong>{artifact.title ?? artifact.kind}</strong>
                      <span>{artifact.summary || artifact.path}</span>
                    </>
                  )}
                  {artifactOpenError?.id === artifact.id && (
                    <span className="context-rail-artifact-error" role="alert">{artifactOpenError.message}</span>
                  )}
                </li>
              ))}
            </ul>
          )}
          {outputs.length > 0 && (
            <ul className="context-rail-outputs">
              {outputs.slice(0, 4).map((step) => (
                <li key={step.id}><strong>{step.title}</strong><span>{step.output}</span></li>
              ))}
            </ul>
          )}
          {!evidenceLoading && artifacts.length === 0 && outputs.length === 0 && (
            <p className="context-rail-empty">{tr('Completed step outputs will collect here.')}</p>
          )}
          <button type="button" className="context-rail-link" onClick={onOpenRuns}>{tr('Open run history')}<ExternalLink size={12} aria-hidden="true" /></button>
        </section>
      </div>
    </aside>
  )
}
