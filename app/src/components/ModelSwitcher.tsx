import { useEffect } from 'react'
import { useConfigStore } from '../stores/configStore'
import { usePersonalityStore } from '../stores/personalityStore'
import { tr } from '../i18n'

export default function ModelSwitcher() {
  const { ollama, availableModels, setOllama } = useConfigStore()
  const { personalities, activeId, loadPersonalities, setActive } = usePersonalityStore()

  useEffect(() => {
    loadPersonalities()
  }, [loadPersonalities])

  const activePersonality = personalities.find((p) => p.id === activeId) ?? null
  const effectiveModel = activePersonality?.model_override || ollama.model

  return (
    <div className="panel model-switcher-panel">
      <h2>{tr("Model & Ollama")}</h2>

      <div className="model-switcher-stack">
        <div className="card model-switcher-active-card">
          <div className="model-switcher-label">{tr("Active model")}</div>
          <div className="model-switcher-active-model">{effectiveModel}</div>
          {activePersonality?.model_override && (
            <div className="model-switcher-note">{tr("overridden by personality &quot;")}{activePersonality.name}{tr("&quot;")}</div>
          )}
        </div>

        <div className="model-switcher-field">
          <label className="model-switcher-label">{tr("Default model")}</label>
          <select
            className="model-switcher-control"
            value={ollama.model}
            onChange={(e) => setOllama({ model: e.target.value })}
          >
            {availableModels.length === 0 && <option value={ollama.model}>{ollama.model}</option>}
            {availableModels.map((m) => (
              <option key={m} value={m}>{m}</option>
            ))}
          </select>
        </div>

        <div className="model-switcher-field">
          <label className="model-switcher-label">{tr("Ollama-URL")}</label>
          <input
            className="model-switcher-control"
            type="text"
            value={ollama.baseUrl}
            onChange={(e) => setOllama({ baseUrl: e.target.value })}
          />
        </div>

        <div className="model-switcher-field">
          <label className="model-switcher-label">{tr("Temperature:")}{ollama.temperature.toFixed(2)}
          </label>
          <input
            className="model-switcher-range"
            type="range"
            min={0}
            max={2}
            step={0.05}
            value={ollama.temperature}
            onChange={(e) => setOllama({ temperature: parseFloat(e.target.value) })}
          />
        </div>

        <div className="model-switcher-field">
          <label className="model-switcher-label">{tr("Context-Window:")}{ollama.contextWindow.toLocaleString()}
          </label>
          <input
            className="model-switcher-range"
            type="range"
            min={2048}
            max={131072}
            step={1024}
            value={ollama.contextWindow}
            onChange={(e) => setOllama({ contextWindow: parseInt(e.target.value) })}
          />
        </div>

        {personalities.length > 0 && (
          <div className="model-switcher-field">
            <label className="model-switcher-label">{tr("Quick personality switch")}</label>
            <div className="model-switcher-help">{tr("Select the active personality. Full management is available below in the &quot;Personalities&quot; section.")}</div>
            <div className="model-switcher-personality-list">
              <button
                type="button"
                className={`btn-sm${activeId == null ? ' active' : ''}`}
                onClick={() => setActive(null)}
              >{tr("Standard")}</button>
              {personalities.map((p) => (
                <button
                  key={p.id}
                  type="button"
                  className={`btn-sm${activeId === p.id ? ' active' : ''}`}
                  onClick={() => setActive(p.id)}
                  title={p.model_override ? `${tr("Model:")} ${p.model_override}` : undefined}
                >
                  {p.icon || 'AG'} {p.name}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
