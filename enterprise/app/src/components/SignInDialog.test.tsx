import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { StrictMode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import SignInDialog from './SignInDialog'

// JSDOM has no native dialog implementation. Production always uses showModal.
const showModal = vi.fn(function (this: HTMLDialogElement) { this.open = true; this.querySelector<HTMLElement>('input,button')?.focus() })
const close = vi.fn(function (this: HTMLDialogElement) { this.open = false })
const originalShowModal = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, 'showModal')
const originalClose = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, 'close')
const contents = <><h2 id="sign-in-heading">Confirm sign-in</h2><input aria-label="Authenticator code" autoFocus /><button type="button">Verify</button></>

describe('native sign-in dialog', () => {
  beforeEach(() => {
    showModal.mockClear(); close.mockClear()
    Object.defineProperty(HTMLDialogElement.prototype, 'showModal', { configurable: true, writable: true, value: showModal })
    Object.defineProperty(HTMLDialogElement.prototype, 'close', { configurable: true, writable: true, value: close })
  })
  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    if (originalShowModal) Object.defineProperty(HTMLDialogElement.prototype, 'showModal', originalShowModal)
    else Reflect.deleteProperty(HTMLDialogElement.prototype, 'showModal')
    if (originalClose) Object.defineProperty(HTMLDialogElement.prototype, 'close', originalClose)
    else Reflect.deleteProperty(HTMLDialogElement.prototype, 'close')
  })

  it('opens a labelled native modal once and keeps focus when busy changes', () => {
    const onCancel = vi.fn()
    const view = render(<SignInDialog labelledBy="sign-in-heading" onCancel={onCancel}>{contents}</SignInDialog>)
    const dialog = screen.getByRole('dialog', { name: 'Confirm sign-in' })
    expect(dialog).toHaveAttribute('open')
    expect(dialog).toHaveAttribute('aria-modal', 'true')
    expect(showModal).toHaveBeenCalledTimes(1)
    expect(screen.getByRole('textbox', { name: 'Authenticator code' })).toHaveFocus()
    view.rerender(<SignInDialog labelledBy="sign-in-heading" onCancel={onCancel} busy>{contents}</SignInDialog>)
    expect(dialog).toHaveAttribute('aria-busy', 'true')
    expect(showModal).toHaveBeenCalledTimes(1)
    expect(close).not.toHaveBeenCalled()
  })

  it('routes Escape cancellation to the current callback while preventing native self-close', () => {
    const initial = vi.fn(), current = vi.fn()
    const view = render(<SignInDialog labelledBy="sign-in-heading" onCancel={initial}>{contents}</SignInDialog>)
    view.rerender(<SignInDialog labelledBy="sign-in-heading" onCancel={current}>{contents}</SignInDialog>)
    const event = new Event('cancel', { bubbles: false, cancelable: true })
    act(() => screen.getByRole('dialog').dispatchEvent(event))
    expect(event.defaultPrevented).toBe(true)
    expect(current).toHaveBeenCalledTimes(1)
    expect(initial).not.toHaveBeenCalled()
    expect(screen.getByRole('dialog')).toHaveAttribute('open')
    expect(close).not.toHaveBeenCalled()
  })

  it('focuses the code field even when its help button precedes it', () => {
    render(<SignInDialog labelledBy="sign-in-heading" onCancel={vi.fn()}><h2 id="sign-in-heading">Confirm sign-in</h2><button type="button">Help for code</button><input aria-label="Authenticator code" autoFocus data-signin-focus /></SignInDialog>)
    expect(screen.getByRole('textbox', { name: 'Authenticator code' })).toHaveFocus()
  })

  it('blocks Escape during a pending mutation and ignores backdrop clicks', () => {
    const onCancel = vi.fn()
    const view = render(<SignInDialog labelledBy="sign-in-heading" onCancel={onCancel} busy>{contents}</SignInDialog>)
    const dialog = screen.getByRole('dialog')
    const event = new Event('cancel', { cancelable: true })
    act(() => dialog.dispatchEvent(event))
    fireEvent.mouseDown(dialog); fireEvent.mouseUp(dialog); fireEvent.click(dialog)
    expect(event.defaultPrevented).toBe(true)
    expect(onCancel).not.toHaveBeenCalled()
    expect(close).not.toHaveBeenCalled()
    view.rerender(<SignInDialog labelledBy="sign-in-heading" onCancel={onCancel}>{contents}</SignInDialog>)
    fireEvent.click(dialog)
    expect(onCancel).not.toHaveBeenCalled()
  })

  it('closes on cleanup and restores the opener despite child autoFocus', () => {
    const opener = document.createElement('button'); opener.textContent = 'Sign in'; document.body.append(opener); opener.focus()
    const view = render(<SignInDialog labelledBy="sign-in-heading" onCancel={vi.fn()}>{contents}</SignInDialog>)
    expect(screen.getByRole('textbox', { name: 'Authenticator code' })).toHaveFocus()
    view.unmount()
    expect(close).toHaveBeenCalledTimes(1)
    expect(opener).toHaveFocus()
    opener.remove()
  })

  it('does not focus an opener that has left the document', () => {
    const opener = document.createElement('button'); document.body.append(opener); opener.focus()
    const view = render(<SignInDialog labelledBy="sign-in-heading" onCancel={vi.fn()}>{contents}</SignInDialog>)
    const focus = vi.spyOn(opener, 'focus'); opener.remove(); view.unmount()
    expect(focus).not.toHaveBeenCalled()
    expect(close).toHaveBeenCalledTimes(1)
  })

  it('keeps the native modal lifecycle balanced in StrictMode', () => {
    const view = render(<StrictMode><SignInDialog labelledBy="sign-in-heading" onCancel={vi.fn()}>{contents}</SignInDialog></StrictMode>)
    expect(screen.getByRole('dialog')).toHaveAttribute('open')
    expect(showModal).toHaveBeenCalledTimes(2)
    expect(close).toHaveBeenCalledTimes(1)
    view.unmount()
    expect(close).toHaveBeenCalledTimes(2)
  })
})
