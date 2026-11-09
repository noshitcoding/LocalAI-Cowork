import { act, fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import i18n from '../i18n'
import RemoteContentBrowser from './RemoteContentBrowser'
vi.mock('./RemoteThreadMessages', () => ({ default: ({ threadId, replyContext }: { threadId: string; replyContext?: unknown }) => <p>{threadId}{replyContext ? ' can reply' : ' read only'}</p> }))
vi.mock('./InstanceContentSharing', () => ({ default: () => <p>Immutable content transfers</p> }))
describe('authorised content browser', () => {
  beforeEach(async () => { await i18n.changeLanguage('en') })
  it('opens authorised messages without execution or reply controls', async () => {
    const client = { listProjects: vi.fn(async () => [{ id: 'a', name: 'Approved project' }]), listProjectThreads: vi.fn(async () => [{ id: 'chat-a', title: 'Approved chat' }]) } as unknown as RemoteRuntimeClient
    render(<RemoteContentBrowser client={client} />)
    fireEvent.click(screen.getByRole('button', { name: 'Read content' }))
    expect(await screen.findByText('chat-a read only')).toBeInTheDocument()
    expect(client.listProjectThreads).toHaveBeenCalledWith('a')
    expect(screen.queryByText(/can reply/)).not.toBeInTheDocument()
  })
  it('drops a previous project reply after changing the selection', async () => {
    let finish!: (value: unknown) => void
    const client = { listProjects: vi.fn(async () => [{ id: 'a', name: 'First' }, { id: 'b', name: 'Second' }]), listProjectThreads: vi.fn((id: string) => id === 'a' ? new Promise(resolve => { finish = resolve }) : Promise.resolve([{ id: 'chat-b', title: 'Second chat' }])) } as unknown as RemoteRuntimeClient
    render(<RemoteContentBrowser client={client} />)
    fireEvent.click(screen.getByRole('button', { name: 'Read content' }))
    await screen.findByRole('option', { name: 'First' })
    fireEvent.change(screen.getByLabelText('Project'), { target: { value: 'b' } })
    await screen.findByText('chat-b read only')
    await act(async () => { finish([{ id: 'chat-a', title: 'Stale chat' }]) })
    expect(screen.queryByText('chat-a read only')).not.toBeInTheDocument()
    expect(screen.getByText('chat-b read only')).toBeInTheDocument()
  })
})
