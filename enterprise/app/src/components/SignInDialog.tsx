import { useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import './SignInDialog.css'

type Props = { children: ReactNode; onCancel: () => void; labelledBy: string; busy?: boolean }

export default function SignInDialog({ children, onCancel, labelledBy, busy = false }: Props) {
  const dialogRef = useRef<HTMLDialogElement>(null)
  // Capture before React mounts any child with autoFocus.
  const [returnFocus] = useState(() => typeof document !== 'undefined' && document.activeElement instanceof HTMLElement ? document.activeElement : null)
  useLayoutEffect(() => {
    const dialog = dialogRef.current
    if (!dialog) return
    if (!dialog.open) dialog.showModal()
    dialog.querySelector<HTMLElement>('[data-signin-focus]')?.focus({ preventScroll: true })
    return () => {
      if (dialog.open) dialog.close()
      if (returnFocus?.isConnected) returnFocus.focus({ preventScroll: true })
    }
  }, [returnFocus])

  return <dialog ref={dialogRef} className="sign-in-dialog" aria-labelledby={labelledBy} aria-modal="true" aria-busy={busy || undefined} onCancel={event => {
    event.preventDefault()
    if (!busy) onCancel()
  }}>{children}</dialog>
}
