import { tr } from '../i18n'

type CoworkQuickPromptsProps = {
  prompts: string[]
  onSelect: (prompt: string) => void
}

export default function CoworkQuickPrompts({ prompts, onSelect }: CoworkQuickPromptsProps) {
  return (
    <div className="quick-prompts crew-starter-grid" role="group" aria-label={tr('Use starter task')}>
      {prompts.map((prompt, index) => (
        <button
          key={prompt}
          type="button"
          className="quick-prompt-btn crew-starter-card"
          onClick={() => onSelect(prompt)}
        >
          <span className="crew-starter-number" aria-hidden="true">{String(index + 1).padStart(2, '0')}</span>
          <strong>{prompt}</strong>
        </button>
      ))}
    </div>
  )
}
