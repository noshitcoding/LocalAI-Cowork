import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { DesktopSession } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import RemoteDesktopViewer from './RemoteDesktopViewer'

const noVnc = vi.hoisted(() => ({
  instances: [] as Array<{
    viewOnly: boolean
    disconnect: ReturnType<typeof vi.fn>
    clipboardPasteFrom: ReturnType<typeof vi.fn>
    sendCtrlAltDel: ReturnType<typeof vi.fn>
  }>,
}))

vi.mock('@novnc/novnc', () => {
  class MockRFB extends EventTarget {
    viewOnly = false
    clipViewport = false
    scaleViewport = false
    resizeSession = false
    showDotCursor = false
    background = ''
    qualityLevel = 0
    compressionLevel = 0
    capabilities = {}
    disconnect = vi.fn(() => this.dispatchEvent(new CustomEvent('disconnect', { detail: { clean: true } })))
    clipboardPasteFrom = vi.fn()
    sendCtrlAltDel = vi.fn()
    focus = vi.fn()

    constructor() {
      super()
      noVnc.instances.push(this)
      queueMicrotask(() => this.dispatchEvent(new CustomEvent('connect', { detail: {} })))
    }
  }
  return { default: MockRFB }
})

const session: DesktopSession = {
  schema_version: 1,
  id: '3424205c-6281-4ce0-b7a5-ac30e2d7f95e',
  run_id: '8bda7de0-e4d5-4d8d-bd07-76d8a3ac03a8',
  executor_id: '7c440e5b-2081-494d-8215-86595272ddb5',
  state: 'agent_controlled',
  stream_protocol: 'rfb.binary.v1',
  dimensions: { width: 1440, height: 900, scale_factor: 1 },
  controller_user_id: null,
  created_at: new Date().toISOString(),
  ended_at: null,
}

describe('RemoteDesktopViewer', () => {
  beforeEach(() => {
    noVnc.instances.length = 0
  })

  it('starts view-only and requires password reauthentication before control', async () => {
    const client = {
      createDesktopStreamTicket: vi.fn()
        .mockResolvedValueOnce({ token: 'view-ticket' })
        .mockResolvedValueOnce({ token: 'control-ticket' })
        .mockResolvedValueOnce({ token: 'view-ticket-2' }),
      desktopStreamUrl: vi.fn((_sessionId: string, ticket: string) => `wss://server.test/desktop?ticket=${ticket}`),
      reauthenticateDesktopControl: vi.fn().mockResolvedValue({ token: 'reauth-token' }),
    } as unknown as RemoteRuntimeClient

    render(<RemoteDesktopViewer client={client} runId={session.run_id} session={session} />)

    await screen.findByText('Live desktop (view only)')
    await waitFor(() => expect(noVnc.instances).toHaveLength(1))
    expect(noVnc.instances[0]?.viewOnly).toBe(true)
    expect(client.createDesktopStreamTicket).toHaveBeenNthCalledWith(1, session.run_id, session.id, false, undefined)

    fireEvent.click(screen.getByRole('button', { name: /take control/i }))
    fireEvent.change(screen.getByLabelText(/confirm your account password/i), { target: { value: 'secret-password' } })
    fireEvent.click(screen.getByRole('button', { name: /confirm and take control/i }))

    await screen.findByText('You are controlling this desktop')
    expect(client.reauthenticateDesktopControl).toHaveBeenCalledWith('secret-password')
    expect(client.createDesktopStreamTicket).toHaveBeenNthCalledWith(2, session.run_id, session.id, true, 'reauth-token')
    expect(noVnc.instances[1]?.viewOnly).toBe(false)

    fireEvent.click(screen.getByRole('button', { name: /release control/i }))
    await screen.findByText('Live desktop (view only)')
    await waitFor(() => expect(noVnc.instances).toHaveLength(3))
    expect(noVnc.instances[2]?.viewOnly).toBe(true)
  })
})
