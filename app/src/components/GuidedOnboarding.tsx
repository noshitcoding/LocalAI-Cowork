import { useState } from 'react'
import { ArrowLeft, ArrowRight, Check, FolderOpen, PanelsTopLeft, Settings2, ShieldCheck, X } from 'lucide-react'
import { tr } from '../i18n'

const STORAGE_KEY = 'open-cowork.onboarding.v1'
const STARTER_PROMPT = 'Inspect the current working folder and create a concise project brief with architecture, risks, and the three highest-priority next steps. Do not modify files.'

type GuidedOnboardingProps = {
  providerLabel: string
  model: string
  providerConfigured: boolean
  workingFolder: string | null
  permissionLabel: string
  onChooseFolder: () => void
  onOpenSettings: () => void
  onUseStarterTask: (prompt: string) => void
}

function readStoredState(): 'open' | 'collapsed' {
  if (typeof window === 'undefined') return 'open'
  try {
    return window.localStorage.getItem(STORAGE_KEY) ? 'collapsed' : 'open'
  } catch {
    return 'open'
  }
}

function persistState(value: 'dismissed' | 'completed') {
  try {
    window.localStorage.setItem(STORAGE_KEY, value)
  } catch {
    // The flow still works when storage is unavailable.
  }
}

export default function GuidedOnboarding({
  providerLabel,
  model,
  providerConfigured,
  workingFolder,
  permissionLabel,
  onChooseFolder,
  onOpenSettings,
  onUseStarterTask,
}: GuidedOnboardingProps) {
  const [visibility, setVisibility] = useState(readStoredState)
  const [step, setStep] = useState(0)
  const modelReady = providerConfigured && Boolean(model.trim())
  const folderReady = Boolean(workingFolder?.trim())
  const steps = [
    tr('Orient'),
    tr('Model'),
    tr('Context'),
    tr('Control'),
  ]

  const collapse = (value: 'dismissed' | 'completed') => {
    persistState(value)
    setVisibility('collapsed')
  }

  if (visibility === 'collapsed') {
    return (
      <section className="cowork-empty-start" aria-labelledby="cowork-empty-start-title">
        <div className="cowork-empty-start-mark" aria-hidden="true"><img src="/favicon.svg" alt="" /></div>
        <span className="onboarding-kicker">{modelReady ? tr('Ready') : tr('Needs setup')}</span>
        <h1 id="cowork-empty-start-title">{tr('What do you want to accomplish?')}</h1>
        <p>{tr('Describe the outcome below. LocalAI Cowork will keep the plan, tool activity, approvals, and outputs together.')}</p>
        <div className="cowork-empty-start-status" aria-label={tr('Current workspace setup')}>
          <span><strong>{providerLabel}</strong>{model || tr('No model selected')}</span>
          <span><strong>{tr('Context')}</strong>{folderReady ? tr('Folder connected') : tr('No folder connected')}</span>
          <span><strong>{tr('Safety')}</strong>{permissionLabel}</span>
        </div>
        <div className="cowork-empty-start-actions">
          {!modelReady ? (
            <button type="button" className="onboarding-reopen" onClick={onOpenSettings}>
              <Settings2 size={15} aria-hidden="true" />{tr('Open model settings')}
            </button>
          ) : !folderReady ? (
            <button type="button" className="onboarding-secondary-action" onClick={onChooseFolder}>
              <FolderOpen size={15} aria-hidden="true" />{tr('Choose a working folder')}
            </button>
          ) : null}
          <button
            type="button"
            className={modelReady ? 'onboarding-reopen' : 'onboarding-secondary-action'}
            onClick={() => {
              setStep(0)
              setVisibility('open')
            }}
          >
            <PanelsTopLeft size={16} aria-hidden="true" />
            {tr('Open getting started')}
          </button>
        </div>
      </section>
    )
  }

  return (
    <section className="guided-onboarding" aria-labelledby="guided-onboarding-title">
      <div className="onboarding-accent" aria-hidden="true" />
      <header className="onboarding-header">
        <div className="onboarding-brand-mark" aria-hidden="true"><img src="/favicon.svg" alt="" /></div>
        <div className="onboarding-heading">
          <span className="onboarding-kicker">{tr('Getting started')}</span>
          <h1 id="guided-onboarding-title">{tr('Set up LocalAI Cowork')}</h1>
          <p>{tr('Start confidently in four short steps.')}</p>
        </div>
        <button type="button" className="onboarding-dismiss" onClick={() => collapse('dismissed')} aria-label={tr('Dismiss onboarding')}>
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      <div className="onboarding-layout">
        <nav className="onboarding-steps" aria-label={tr('Onboarding steps')}>
          {steps.map((label, index) => (
            <button
              key={label}
              type="button"
              className={index === step ? 'active' : index < step ? 'complete' : ''}
              onClick={() => setStep(index)}
              aria-label={label}
              aria-current={index === step ? 'step' : undefined}
            >
              <span>{index < step ? <Check size={13} aria-hidden="true" /> : index + 1}</span>
              {label}
            </button>
          ))}
        </nav>

        <div className="onboarding-content">
          <div className="onboarding-progress-label">{tr('Step {{current}} of {{total}}', { current: step + 1, total: steps.length })}</div>

          {step === 0 && (
            <div className="onboarding-stage">
              <h2>{tr('Meet your workspace')}</h2>
              <p>{tr('Describe the outcome, review the plan, and approve sensitive actions only when you are ready.')}</p>
              <div className="onboarding-status-grid">
                <div className={modelReady ? 'ready' : 'attention'}>
                  <span>{tr('Provider & model')}</span>
                  <strong>{modelReady ? `${providerLabel} · ${model}` : tr('Needs setup')}</strong>
                </div>
                <div className={folderReady ? 'ready' : 'neutral'}>
                  <span>{tr('Working context')}</span>
                  <strong>{folderReady ? tr('Folder connected') : tr('No folder connected')}</strong>
                </div>
                <div className="ready">
                  <span>{tr('Safety')}</span>
                  <strong>{permissionLabel}</strong>
                </div>
              </div>
            </div>
          )}

          {step === 1 && (
            <div className="onboarding-stage onboarding-action-stage">
              <div className="onboarding-stage-icon" aria-hidden="true"><Settings2 size={23} /></div>
              <div>
                <h2>{tr('Choose how the work is powered')}</h2>
                <p>{tr('Select a local or cloud provider and verify the model before the first run.')}</p>
                <div className="onboarding-current-value">
                  <span>{providerLabel}</span>
                  <strong>{model || tr('No model selected')}</strong>
                  <em className={modelReady ? 'ready' : 'attention'}>{modelReady ? tr('Ready') : tr('Needs setup')}</em>
                </div>
                <button type="button" className="onboarding-secondary-action" onClick={onOpenSettings}>
                  <Settings2 size={15} aria-hidden="true" />{tr('Open model settings')}
                </button>
              </div>
            </div>
          )}

          {step === 2 && (
            <div className="onboarding-stage onboarding-action-stage">
              <div className="onboarding-stage-icon" aria-hidden="true"><FolderOpen size={23} /></div>
              <div>
                <h2>{tr('Give the task a clear working context')}</h2>
                <p>{tr('Add a folder so file work stays scoped, understandable, and auditable.')}</p>
                <div className="onboarding-folder-value" title={workingFolder ?? undefined}>
                  {workingFolder || tr('No folder connected')}
                </div>
                <button type="button" className="onboarding-secondary-action" onClick={onChooseFolder}>
                  <FolderOpen size={15} aria-hidden="true" />{folderReady ? tr('Choose another folder') : tr('Choose a working folder')}
                </button>
              </div>
            </div>
          )}

          {step === 3 && (
            <div className="onboarding-stage onboarding-action-stage">
              <div className="onboarding-stage-icon" aria-hidden="true"><ShieldCheck size={23} /></div>
              <div>
                <h2>{tr('Stay in control of every action')}</h2>
                <p>{tr('LocalAI Cowork can pause before sensitive actions. You can approve, reject, or stop a run at any time.')}</p>
                <div className="onboarding-permission-value">
                  <span>{tr('Current permission mode')}</span>
                  <strong>{permissionLabel}</strong>
                </div>
                <button
                  type="button"
                  className="onboarding-primary-action"
                  onClick={() => {
                    if (!modelReady) {
                      setStep(1)
                      return
                    }
                    if (!folderReady) {
                      setStep(2)
                      return
                    }
                    collapse('completed')
                    onUseStarterTask(tr(STARTER_PROMPT))
                  }}
                >
                  <PanelsTopLeft size={16} aria-hidden="true" />{modelReady && folderReady ? tr('Use starter task') : tr('Continue')}
                </button>
              </div>
            </div>
          )}

          <footer className="onboarding-footer">
            <button type="button" className="onboarding-back" onClick={() => setStep((current) => Math.max(0, current - 1))} disabled={step === 0}>
              <ArrowLeft size={15} aria-hidden="true" />{tr('Back')}
            </button>
            {step < steps.length - 1 && (
              <button type="button" className="onboarding-next" onClick={() => setStep((current) => Math.min(steps.length - 1, current + 1))}>
                {tr('Continue')}<ArrowRight size={15} aria-hidden="true" />
              </button>
            )}
          </footer>
        </div>
      </div>
    </section>
  )
}

export { STARTER_PROMPT, STORAGE_KEY as GUIDED_ONBOARDING_STORAGE_KEY }
