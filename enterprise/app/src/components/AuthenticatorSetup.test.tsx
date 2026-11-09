import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../i18n'

const mocks = vi.hoisted(() => ({ qr: vi.fn() }))
vi.mock('qrcode', () => ({ toDataURL: mocks.qr }))
import AuthenticatorSetup from './AuthenticatorSetup'

const setup = { uri: 'otpauth://totp/Test?secret=TESTSECRET', secret: 'TESTSECRET', expiresAt: '2099-01-01T00:00:00Z' }

describe('local authenticator setup', () => {
  beforeEach(async () => {
    vi.resetAllMocks()
    mocks.qr.mockResolvedValue('data:image/png;base64,bG9jYWw=')
    await i18n.changeLanguage('en')
  })
  afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers() })

  it('generates the QR locally from the exact setup URI and explains Microsoft Authenticator', async () => {
    const fetch = vi.fn()
    vi.stubGlobal('fetch', fetch)
    render(<AuthenticatorSetup {...setup} />)
    expect(await screen.findByRole('img', { name: 'QR code for your authenticator app' })).toHaveAttribute('src', 'data:image/png;base64,bG9jYWw=')
    expect(mocks.qr).toHaveBeenCalledWith(setup.uri, expect.objectContaining({ margin: 4, errorCorrectionLevel: 'M' }))
    expect(fetch).not.toHaveBeenCalled()
    expect(screen.getByText(/In Microsoft Authenticator/)).toHaveTextContent('“Other account”')
    expect(screen.getByText(/App codes also work on localhost/)).toHaveTextContent('You do not need a domain.')
  })

  it('discards an old QR result as soon as the setup URI changes', async () => {
    let finish!: (value: string) => void
    mocks.qr.mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const view = render(<AuthenticatorSetup {...setup} />)
    view.rerender(<AuthenticatorSetup {...setup} uri="otpauth://totp/New?secret=NEWSECRET" secret="NEWSECRET" />)
    await screen.findByRole('img')
    await act(async () => finish('data:image/png;base64,b2xk'))
    expect(screen.getByRole('img')).toHaveAttribute('src', 'data:image/png;base64,bG9jYWw=')
    expect(screen.getByLabelText('Authenticator secret')).toHaveValue('NEWSECRET')
  })

  it('keeps the manual key available when QR rendering fails', async () => {
    mocks.qr.mockRejectedValue(new Error('Unavailable'))
    render(<AuthenticatorSetup {...setup} />)
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Use the setup key below.'))
    expect(screen.getByText('Or enter the setup key').closest('details')).toHaveAttribute('open')
    expect(screen.getByLabelText('Authenticator secret')).toHaveValue(setup.secret)
  })

  it('hides an expired secret and lets the user explicitly restart setup', () => {
    const restart = vi.fn()
    render(<AuthenticatorSetup {...setup} expiresAt="2000-01-01T00:00:00Z" onRestart={restart} />)
    expect(screen.getByRole('status')).toHaveTextContent('This setup has expired')
    expect(screen.queryByRole('img')).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Authenticator secret')).not.toBeInTheDocument()
    expect(mocks.qr).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Restart setup' }))
    expect(restart).toHaveBeenCalledOnce()
  })

  it('removes the QR when the pending setup expires', async () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-10-17T00:00:00Z'))
    render(<AuthenticatorSetup {...setup} expiresAt="2026-10-17T00:00:01Z" />)
    await act(async () => {})
    expect(screen.getByRole('img')).toBeInTheDocument()
    await act(async () => { vi.advanceTimersByTime(1_001) })
    expect(screen.queryByRole('img')).not.toBeInTheDocument()
    expect(screen.getByRole('status')).toHaveTextContent('This setup has expired')
  })
})
