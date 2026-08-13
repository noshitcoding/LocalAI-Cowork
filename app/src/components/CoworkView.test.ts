import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  appendStoppedAssistantContent,
  buildChatWorkspacePermissionConfig,
  buildProjectInstructionsPromptContext,
  buildProjectLinkPromptContext,
  findPreviousUserMessage,
  formatAssistantFailureContent,
  getAssistantFailureSettingsPath,
  getChatWorkspaceAttachments,
  isAssistantFailureContent,
  reconcileChatComposerAttachments,
  updateAttachmentAccess,
} from './CoworkView'
import type { ProjectResource } from '../stores/projectStore'
import i18n from '../i18n'

const safeInvokeMock = vi.hoisted(() => vi.fn())

vi.mock('../utils/safeInvoke', () => ({
  hasTauriRuntime: vi.fn(() => false),
  safeInvoke: safeInvokeMock,
  safeInvokeVoid: vi.fn(),
}))

describe('CoworkView project context helpers', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en')
    safeInvokeMock.mockReset()
  })

  it('builds supplemental project instructions without global override text', () => {
    expect(buildProjectInstructionsPromptContext({
      title: 'Alpha',
      instructions: 'Use the project sources first.  ',
    })).toBe('Project instructions for "Alpha":\nUse the project sources first.')

    expect(buildProjectInstructionsPromptContext({
      title: 'Alpha',
      instructions: '   ',
    })).toBe('')
  })

  it('recognizes actionable assistant failures without flagging ordinary answers', () => {
    expect(isAssistantFailureContent('LLM request failed: timeout')).toBe(true)
    expect(isAssistantFailureContent('ConnectionError: provider unreachable')).toBe(true)
    expect(isAssistantFailureContent('Here is the completed launch checklist.')).toBe(false)
  })

  it('presents provider failures as localized, actionable chat copy', async () => {
    await i18n.changeLanguage('de')

    expect(formatAssistantFailureContent(
      'LLM request failed: OpenRouter API-Key fehlt.\n\nCheck the OpenRouter profile, endpoint, API key, and model in Settings.',
    )).toBe(
      'Anfrage fehlgeschlagen: Für OpenRouter fehlt der API-Schlüssel.\n\nÜberprüfe in den Einstellungen das OpenRouter-Profil, den Endpunkt, den API-Schlüssel und das Modell.',
    )
  })

  it('links provider failures to the matching settings profile', () => {
    expect(getAssistantFailureSettingsPath('LLM request failed: OpenRouter API-Key fehlt.')).toBe('/settings?provider=openrouter')
    expect(getAssistantFailureSettingsPath('Ollama request failed: timeout')).toBe('/settings?provider=ollama')
    expect(getAssistantFailureSettingsPath('Unknown provider failure')).toBe('/settings')
  })

  it('restores the request that directly precedes a failed assistant response', () => {
    const messages = [
      { id: 'user-1', role: 'user' as const, content: 'First request', timestamp: 1 },
      { id: 'assistant-1', role: 'assistant' as const, content: 'First answer', timestamp: 2 },
      { id: 'user-2', role: 'user' as const, content: 'Retry this request', timestamp: 3 },
      { id: 'assistant-2', role: 'assistant' as const, content: 'LLM request failed: timeout', timestamp: 4 },
    ]

    expect(findPreviousUserMessage(messages, 'assistant-2')?.id).toBe('user-2')
    expect(findPreviousUserMessage(messages, 'missing')).toBeNull()
  })

  it('marks stopped responses in the active interface language', async () => {
    expect(appendStoppedAssistantContent('Partial answer')).toBe('Partial answer\n\nStopped')

    await i18n.changeLanguage('de')
    expect(appendStoppedAssistantContent('')).toBe('Gestoppt')
  })

  it('fetches project links manually and reports non-blocking failures', async () => {
    safeInvokeMock.mockImplementation(async (_cmd: string, args: { request: { url: string } }) => {
      if (args.request.url.includes('broken')) {
        throw new Error('Network error')
      }
      return {
        url: args.request.url,
        status: 200,
        ok: true,
        title: 'Specification',
        content: 'Link content',
        truncated: false,
      }
    })

    const links: ProjectResource[] = [
      {
        id: 'link-1',
        kind: 'link',
        path: 'https://example.com/spec',
        label: 'Spec',
        enabled: true,
        access: 'read_only',
        isPrimary: false,
        addedAt: 100,
      },
      {
        id: 'link-2',
        kind: 'link',
        path: 'https://example.com/broken',
        label: 'Broken',
        enabled: true,
        access: 'read_only',
        isPrimary: false,
        addedAt: 100,
      },
    ]

    const result = await buildProjectLinkPromptContext(links)

    expect(result.context).toContain('Manually fetched project links:')
    expect(result.context).toContain('Source: Spec')
    expect(result.context).toContain('Link content')
    expect(result.notice).toContain('Not all project links could be fetched')
    expect(result.notice).toContain('Broken: Network error')
  })

  it('updates attachment access without retaining the change event', () => {
    const attachments = [
      { path: 'C:\\workspace', kind: 'folder' as const, access: 'read_only' as const },
      { path: 'C:\\notes.txt', kind: 'file' as const, access: 'read_only' as const },
    ]

    expect(updateAttachmentAccess(attachments, attachments[0], 'read_write')).toEqual([
      { path: 'C:\\workspace', kind: 'folder', access: 'read_write' },
      attachments[1],
    ])
  })

  it('stores selected folders as chat-scoped workspace directories', () => {
    const nextConfig = buildChatWorkspacePermissionConfig({
      mode: 'strict',
      allowedDirectories: ['C:\\shared', 'C:\\old-workspace'],
      workspaceAttachments: [{ path: 'C:\\old-workspace', kind: 'folder', access: 'read_only' }],
    }, [
      { path: 'C:\\new-workspace', kind: 'folder', access: 'read_write' },
      { path: 'C:\\one-shot.txt', kind: 'file', access: 'read_only' },
    ], 'default')

    expect(nextConfig).toEqual({
      mode: 'strict',
      allowedDirectories: ['C:\\shared', 'C:\\new-workspace'],
      workspaceAttachments: [
        { path: 'C:\\new-workspace', kind: 'folder', access: 'read_write' },
        { path: 'C:\\one-shot.txt', kind: 'file', access: 'read_only' },
      ],
    })
    expect(getChatWorkspaceAttachments(nextConfig)).toEqual([
      {
        path: 'C:\\new-workspace',
        kind: 'folder',
        access: 'read_write',
        isPrimary: true,
      },
      {
        path: 'C:\\one-shot.txt',
        kind: 'file',
        access: 'read_only',
        isPrimary: false,
      },
    ])
  })

  it('keeps local folders and files after submit while clearing inline-only attachments', () => {
    const permissionConfig = {
      mode: 'default' as const,
      allowedDirectories: ['C:\\workspace'],
      workspaceAttachments: [
        { path: 'C:\\workspace', kind: 'folder' as const, access: 'read_only' as const },
        { path: 'C:\\notes.txt', kind: 'file' as const, access: 'read_only' as const },
      ],
    }
    const current = [
      { path: 'C:\\workspace', kind: 'folder' as const, access: 'read_only' as const },
      { path: 'C:\\notes.txt', kind: 'file' as const, access: 'read_only' as const },
      { path: 'clipboard.png', kind: 'file' as const, source: 'inline' as const, dataUrl: 'data:image/png;base64,abc' },
    ]

    expect(reconcileChatComposerAttachments(permissionConfig, current, false)).toEqual([
      {
        path: 'C:\\workspace',
        kind: 'folder',
        access: 'read_only',
        isPrimary: true,
      },
      {
        path: 'C:\\notes.txt',
        kind: 'file',
        access: 'read_only',
        isPrimary: false,
      },
    ])
  })
})
