import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../i18n'
import { useCodexStore } from '../stores/codexStore'
import CodexAccountsPanel from './CodexAccountsPanel'

const mocks = vi.hoisted(() => ({
  openUrl: vi.fn(),
  writeText: vi.fn(),
  load: vi.fn(),
}))

vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: (...args: unknown[]) => mocks.openUrl(...args),
}))

describe('CodexAccountsPanel device login', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en')
    mocks.openUrl.mockReset()
    mocks.writeText.mockReset()
    mocks.writeText.mockResolvedValue(undefined)
    mocks.load.mockReset()
    mocks.load.mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: mocks.writeText },
    })
    useCodexStore.setState({
      runtime: { available: true, version: '0.147.0', protocolSchema: 'app-server-0.147.0', error: null },
      profiles: [],
      deviceLogin: {
        profileId: 'codex-test',
        verificationUrl: 'https://auth.openai.com/codex/device',
        userCode: 'ABCD-1234',
      },
      loading: false,
      error: null,
      load: mocks.load,
    })
  })

  it('shows a selectable link and copies it without opening the browser', async () => {
    render(<CodexAccountsPanel />)

    expect(screen.getByLabelText('Sign-in link')).toHaveValue('https://auth.openai.com/codex/device')
    fireEvent.click(screen.getByRole('button', { name: 'Copy sign-in link' }))

    await waitFor(() => expect(mocks.writeText).toHaveBeenCalledWith('https://auth.openai.com/codex/device'))
    expect(mocks.openUrl).not.toHaveBeenCalled()
    expect(screen.getByRole('button', { name: 'Link copied' })).toBeInTheDocument()
  })

  it('opens the device page only after the user requests it', () => {
    render(<CodexAccountsPanel />)

    fireEvent.click(screen.getByRole('button', { name: 'Open sign-in page' }))

    expect(mocks.openUrl).toHaveBeenCalledWith('https://auth.openai.com/codex/device')
  })
})
