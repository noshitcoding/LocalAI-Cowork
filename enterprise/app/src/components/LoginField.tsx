import { cloneElement, useId, useState, type InputHTMLAttributes, type ReactElement } from 'react'
import './LoginField.css'

export default function LoginField({ label, help, helpButtonLabel, children }: {
  label: string
  help: string
  helpButtonLabel: string
  children: ReactElement<InputHTMLAttributes<HTMLInputElement>>
}) {
  const id = useId()
  const [expanded, setExpanded] = useState(false)
  const helpId = `${id}-help`
  const describedBy = [children.props['aria-describedby'], expanded ? helpId : null].filter(Boolean).join(' ') || undefined

  return <div className="remote-login-field">
    <div className="remote-login-field-heading">
      <label htmlFor={id}>{label}</label>
      <button type="button" className="remote-login-field-info" aria-label={helpButtonLabel}
        title={help} aria-expanded={expanded} aria-controls={helpId}
        onClick={() => setExpanded(value => !value)}
        onKeyDown={event => { if (event.key === 'Escape') { setExpanded(false); event.stopPropagation() } }}>
        <svg aria-hidden="true" width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="1.4">
          <circle cx="7" cy="7" r="5.8" />
          <path d="M7 6.3v3.5M7 4h.01" />
        </svg>
      </button>
    </div>
    {expanded && <p id={helpId} className="remote-login-field-help">{help}</p>}
    {cloneElement(children, { id, 'aria-describedby': describedBy })}
  </div>
}
