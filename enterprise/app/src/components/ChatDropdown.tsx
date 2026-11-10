import { useEffect, useId, useRef, useState } from 'react'
import { Check, ChevronDown } from 'lucide-react'

export type ChatDropdownOption = {
  value: string
  label: string
  disabled?: boolean
}

type ChatDropdownProps = {
  value: string
  options: ChatDropdownOption[]
  onChange: (value: string) => void
  ariaLabel: string
  className?: string
  disabled?: boolean
  title?: string
}

function nextEnabledIndex(options: ChatDropdownOption[], start: number, direction: 1 | -1): number {
  if (options.length === 0) return -1

  for (let offset = 1; offset <= options.length; offset += 1) {
    const index = (start + (offset * direction) + options.length) % options.length
    if (!options[index].disabled) return index
  }

  return -1
}

export default function ChatDropdown({
  value,
  options,
  onChange,
  ariaLabel,
  className = '',
  disabled = false,
  title,
}: ChatDropdownProps) {
  const listboxId = useId()
  const rootRef = useRef<HTMLDivElement>(null)
  const toggleRef = useRef<HTMLButtonElement>(null)
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([])
  const selectedIndex = options.findIndex((option) => option.value === value)
  const fallbackIndex = options.findIndex((option) => !option.disabled)
  const initialIndex = selectedIndex >= 0 ? selectedIndex : fallbackIndex
  const [open, setOpen] = useState(false)
  const [activeIndex, setActiveIndex] = useState(initialIndex)
  const selectedOption = selectedIndex >= 0 ? options[selectedIndex] : options[fallbackIndex]

  useEffect(() => {
    if (!open) return

    const handlePointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false)
    }

    document.addEventListener('pointerdown', handlePointerDown)
    return () => document.removeEventListener('pointerdown', handlePointerDown)
  }, [open])

  useEffect(() => {
    if (!open || activeIndex < 0) return
    const frame = window.requestAnimationFrame(() => optionRefs.current[activeIndex]?.focus())
    return () => window.cancelAnimationFrame(frame)
  }, [activeIndex, open])

  const openMenu = () => {
    if (disabled || options.length === 0) return
    setActiveIndex(initialIndex)
    setOpen(true)
  }

  const closeMenu = (restoreFocus = false) => {
    setOpen(false)
    if (restoreFocus) window.requestAnimationFrame(() => toggleRef.current?.focus())
  }

  const chooseOption = (option: ChatDropdownOption) => {
    if (option.disabled) return
    if (option.value !== value) onChange(option.value)
    closeMenu(true)
  }

  const moveActive = (direction: 1 | -1) => {
    const next = nextEnabledIndex(options, activeIndex >= 0 ? activeIndex : initialIndex, direction)
    if (next >= 0) setActiveIndex(next)
  }

  return (
    <div ref={rootRef} className={`chat-dropdown ${className}`.trim()}>
      <button
        ref={toggleRef}
        type="button"
        className="chat-dropdown-toggle"
        role="combobox"
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-controls={listboxId}
        aria-expanded={open}
        disabled={disabled}
        title={title}
        onClick={() => (open ? closeMenu() : openMenu())}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault()
            openMenu()
          } else if (event.key === 'Escape' && open) {
            event.preventDefault()
            closeMenu(true)
          }
        }}
      >
        <span>{selectedOption?.label ?? value}</span>
        <ChevronDown size={13} aria-hidden="true" />
      </button>

      {open && (
        <div id={listboxId} className="chat-dropdown-menu" role="listbox" aria-label={ariaLabel}>
          {options.map((option, index) => (
            <button
              key={option.value}
              ref={(element) => { optionRefs.current[index] = element }}
              type="button"
              role="option"
              aria-selected={option.value === value}
              className={`chat-dropdown-option${index === activeIndex ? ' active' : ''}${option.value === value ? ' selected' : ''}`}
              disabled={option.disabled}
              onClick={() => chooseOption(option)}
              onKeyDown={(event) => {
                if (event.key === 'ArrowDown') {
                  event.preventDefault()
                  moveActive(1)
                } else if (event.key === 'ArrowUp') {
                  event.preventDefault()
                  moveActive(-1)
                } else if (event.key === 'Home') {
                  event.preventDefault()
                  setActiveIndex(fallbackIndex)
                } else if (event.key === 'End') {
                  event.preventDefault()
                  const lastEnabled = [...options].reverse().findIndex((entry) => !entry.disabled)
                  if (lastEnabled >= 0) setActiveIndex(options.length - 1 - lastEnabled)
                } else if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault()
                  chooseOption(option)
                } else if (event.key === 'Escape') {
                  event.preventDefault()
                  closeMenu(true)
                } else if (event.key === 'Tab') {
                  setOpen(false)
                }
              }}
            >
              <span>{option.label}</span>
              {option.value === value ? <Check size={13} aria-hidden="true" /> : null}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
