import { useState } from 'react'
import { Bot, KeyRound } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useBackendDefaultsStore } from '../stores/backendDefaultsStore'
import { useConfigStore } from '../stores/configStore'

export default function BackendSetupDialog() {
  const { t: tr } = useTranslation()
  const { loaded, saving, setupCompleted, error, complete } = useBackendDefaultsStore()
  const profiles = useConfigStore((state) => state.llmProfiles)
  const defaultIds = useConfigStore((state) => state.defaultLlmProfileIds)
  const [apiProfileId, setApiProfileId] = useState(
    defaultIds.api ?? defaultIds.ollama ?? profiles[0]?.id ?? '',
  )

  if (!loaded || setupCompleted) return null

  return (
    <div className="backend-setup-backdrop" role="presentation">
      <section className="backend-setup-dialog" role="dialog" aria-modal="true" aria-labelledby="backend-setup-title">
        <span className="task-detail-kicker">{tr('OpenCowork setup')}</span>
        <h1 id="backend-setup-title">{tr('How would you like to use OpenCowork?')}</h1>
        <p className="hint-text">{tr('This choice becomes the default. You can override it later for individual chats, tasks, and crew members.')}</p>

        <div className="backend-setup-options">
          <button
            type="button"
            className="backend-setup-option"
            disabled={saving}
            onClick={() => void complete('codex', apiProfileId)}
          >
            <Bot size={24} aria-hidden="true" />
            <strong>{tr('Use Codex')}</strong>
            <span>{tr('Use your ChatGPT/Codex quota. You can securely sign in to accounts in Settings afterward.')}</span>
          </button>

          <div className="backend-setup-option backend-setup-option-api">
            <KeyRound size={24} aria-hidden="true" />
            <strong>{tr('OpenAI-compatible API')}</strong>
            <span>{tr('Use a local or hosted API profile.')}</span>
            <select
              className="ui-field"
              aria-label={tr('Default API profile')}
              value={apiProfileId}
              disabled={saving}
              onChange={(event) => setApiProfileId(event.target.value)}
            >
              {profiles.map((profile) => (
                <option key={profile.id} value={profile.id}>{profile.name}</option>
              ))}
            </select>
            <button
              type="button"
              className="ui-button ui-button--primary"
              disabled={saving || !apiProfileId}
              onClick={() => void complete('openai-compatible', apiProfileId)}
            >
              {tr('Use this profile')}
            </button>
          </div>
        </div>

        {error ? <p className="form-error" role="alert">{error}</p> : null}
      </section>
    </div>
  )
}
