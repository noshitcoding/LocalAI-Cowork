import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import DeveloperBrowserView from './DeveloperBrowserView'
import { defaultBrowserUrl } from '../utils/developerBrowserDefaults'
import i18n from '../i18n'
import { safeInvoke } from '../utils/safeInvoke'

vi.mock('../utils/safeInvoke', () => ({
  safeInvoke: vi.fn(async (_command: string, _args: unknown, fallback: unknown) => fallback ?? {}),
}))

describe('DeveloperBrowserView', () => {
  beforeEach(async () => { vi.clearAllMocks(); localStorage.clear(); await i18n.changeLanguage('en') })

  it('uses a public production start URL but does not navigate on mount', async () => {
    expect(defaultBrowserUrl(false)).toBe('https://example.com')
    expect(defaultBrowserUrl(true)).toBe('http://127.0.0.1:5173')
    render(<DeveloperBrowserView />)
    await waitFor(() => expect(safeInvoke).toHaveBeenCalledWith('developer_browser_status', undefined, expect.any(Object)))
    expect(vi.mocked(safeInvoke).mock.calls.some(([command]) => command === 'developer_browser_navigate')).toBe(false)
  })

  it('renders browser controls and inspector copy in German', async () => {
    await i18n.changeLanguage('de')
    render(<DeveloperBrowserView />)
    expect(screen.getByRole('region', { name: 'Browsersteuerung' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Browser starten' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Kommentieren' })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('tab', { name: 'CDP' }))
    expect(screen.getByRole('button', { name: 'CDP-Befehl ausführen' })).toBeInTheDocument()
    expect(screen.getByRole('textbox', { name: 'Parameter (JSON)' })).toBeInTheDocument()
    expect(screen.queryByText('Start browser')).not.toBeInTheDocument()
  })

  it('labels active-page text entry separately from the network resource type', async () => {
    await i18n.changeLanguage('de')
    vi.mocked(safeInvoke).mockImplementation(async (command) => {
      if (command === 'developer_browser_status') return { active: true, browserName: 'Chromium', debuggerPort: 0, profilePath: '' }
      if (command === 'developer_browser_snapshot') return {
        active: true, url: 'https://example.com', title: 'Example', viewportWidth: 1280, viewportHeight: 720,
        deviceScaleFactor: 1, screenshotDataUrl: 'data:image/png;base64,AA==', dom: '', text: '', activeElement: '',
        consoleEntries: [], networkEntries: [],
      }
      return null
    })
    render(<DeveloperBrowserView />)
    fireEvent.change(await screen.findByRole('textbox', { name: 'Text für die Seite' }), { target: { value: 'fixture text' } })
    fireEvent.click(screen.getByRole('button', { name: 'Text eingeben' }))
    await waitFor(() => expect(safeInvoke).toHaveBeenCalledWith('developer_browser_type_text', { request: { text: 'fixture text' } }))
    fireEvent.click(screen.getByRole('tab', { name: /Netzwerk/ }))
    expect(screen.getByText('Typ')).toBeInTheDocument()
  })
})
