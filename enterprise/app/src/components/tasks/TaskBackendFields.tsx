import { useEffect } from 'react'
import { tr } from '../../i18n'
import { useCodexStore } from '../../stores/codexStore'
import { useConfigStore } from '../../stores/configStore'
import type { ChatProviderSelection } from '../../utils/chatProvider'

type TaskBackendFieldsProps = {
  selection?: ChatProviderSelection
  model: string
  defaultModel?: string
  disabled?: boolean
  onChange: (selection: ChatProviderSelection) => void
}

export default function TaskBackendFields({
  selection,
  model,
  defaultModel = '',
  disabled = false,
  onChange,
}: TaskBackendFieldsProps) {
  const profiles = useConfigStore((state) => state.llmProfiles)
  const defaultProfileIds = useConfigStore((state) => state.defaultLlmProfileIds)
  const codexProfiles = useCodexStore((state) => state.profiles)
  const loadCodexProfiles = useCodexStore((state) => state.load)
  const fallbackProfileId = defaultProfileIds.api ?? defaultProfileIds.ollama ?? profiles[0]?.id ?? ''
  const effective = selection ?? {
    backend: 'openai-compatible' as const,
    profileId: fallbackProfileId,
    ...(model.trim() ? { model: model.trim() } : {}),
  }

  useEffect(() => {
    void loadCodexProfiles()
  }, [loadCodexProfiles])

  const changeBackend = (backend: ChatProviderSelection['backend']) => {
    if (backend === 'codex') {
      onChange({ backend: 'codex', ...(model.trim() ? { model: model.trim() } : {}) })
      return
    }
    onChange({
      backend: 'openai-compatible',
      profileId: fallbackProfileId,
      ...(model.trim() ? { model: model.trim() } : {}),
    })
  }

  const changeModel = (nextModel: string) => {
    onChange({
      ...effective,
      ...(nextModel.trim() ? { model: nextModel } : { model: undefined }),
    })
  }

  return (
    <>
      <label>
        {tr('Backend')}
        <select
          className="ui-field"
          value={effective.backend}
          disabled={disabled}
          onChange={(event) => changeBackend(event.target.value as ChatProviderSelection['backend'])}
        >
          <option value="codex">{tr('Use Codex')}</option>
          <option value="openai-compatible">{tr('OpenAI-compatible API')}</option>
        </select>
      </label>

      {effective.backend === 'codex' ? (
        <label>
          {tr('Codex account')}
          <select
            className="ui-field"
            value={effective.authProfileId ?? ''}
            disabled={disabled}
            onChange={(event) => onChange({
              ...effective,
              authProfileId: event.target.value || undefined,
            })}
          >
            <option value="">{tr('Automatic (account order)')}</option>
            {codexProfiles.map((profile) => (
              <option key={profile.id} value={profile.id}>{profile.name}</option>
            ))}
          </select>
          {codexProfiles.length === 0 ? (
            <span className="hint-text">{tr('Add and sign in to a Codex account in Settings.')}</span>
          ) : null}
        </label>
      ) : (
        <label>
          {tr('API profile')}
          <select
            className="ui-field"
            value={effective.profileId || fallbackProfileId}
            disabled={disabled}
            onChange={(event) => onChange({ ...effective, profileId: event.target.value })}
          >
            {profiles.map((profile) => (
              <option key={profile.id} value={profile.id}>{profile.name}</option>
            ))}
          </select>
        </label>
      )}

      <label>
        {tr('Model (optional)')}
        <input
          className="ui-field"
          value={effective.model ?? model}
          disabled={disabled}
          onChange={(event) => changeModel(event.target.value)}
          placeholder={defaultModel ? `${tr('Default')}: ${defaultModel}` : tr('Use backend default')}
        />
      </label>

      {effective.backend === 'codex' ? (
        <label>
          {tr('Reasoning effort')}
          <select
            className="ui-field"
            value={effective.reasoningEffort ?? ''}
            disabled={disabled}
            onChange={(event) => onChange({
              ...effective,
              reasoningEffort: event.target.value || undefined,
            })}
          >
            <option value="">{tr('Automatic')}</option>
            <option value="low">Low</option>
            <option value="medium">Medium</option>
            <option value="high">High</option>
            <option value="xhigh">Extra high</option>
          </select>
        </label>
      ) : null}
    </>
  )
}
