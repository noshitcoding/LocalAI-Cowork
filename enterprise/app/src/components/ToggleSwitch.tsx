import { tr } from '../i18n'

type ToggleSwitchProps = {
  checked: boolean
  onChange: (checked: boolean) => void
  ariaLabel: string
  disabled?: boolean
}

export default function ToggleSwitch({ checked, onChange, ariaLabel, disabled = false }: ToggleSwitchProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={`toggle-switch${checked ? ' on' : ''}`}
    >
      <span className="toggle-track" aria-hidden="true">
        <span className="toggle-state-label">{tr(checked ? 'ON' : 'OFF')}</span>
        <span className="toggle-knob" />
      </span>
    </button>
  )
}
