import { useState, type CSSProperties } from 'react'
import { tr } from '../i18n'

type SecureCredentialInputProps = {
  value: string
  onCommit: (value: string) => Promise<void>
  type?: 'password' | 'text' | 'url'
  className?: string
  placeholder?: string
  style?: CSSProperties
  ariaLabel?: string
}

export default function SecureCredentialInput({
  value,
  onCommit,
  type = 'password',
  className,
  placeholder,
  style,
  ariaLabel,
}: SecureCredentialInputProps) {
  const [draft, setDraft] = useState(value)
  const [error, setError] = useState(false)
  const commit = async () => {
    if (draft === value) return
    try {
      await onCommit(draft)
      setError(false)
    } catch {
      setError(true)
    }
  }

  return (
    <>
      <input
        type={type}
        className={className}
        value={draft}
        onChange={(event) => {
          setDraft(event.target.value)
          setError(false)
        }}
        onBlur={() => { void commit() }}
        onKeyDown={(event) => {
          if (event.key === 'Enter') event.currentTarget.blur()
          if (event.key === 'Escape') {
            setDraft(value)
            setError(false)
            event.currentTarget.blur()
          }
        }}
        placeholder={placeholder}
        style={style}
        aria-label={ariaLabel}
        aria-invalid={error || undefined}
      />
      {error && <span className="field-error" role="alert">{tr('Secure value could not be saved.')}</span>}
    </>
  )
}
