import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  buildProjectInstructionsPromptContext,
  buildProjectLinkPromptContext,
  isAssistantFailureContent,
} from './CoworkView'
import type { ProjectResource } from '../stores/projectStore'

const safeInvokeMock = vi.hoisted(() => vi.fn())

vi.mock('../utils/safeInvoke', () => ({
  hasTauriRuntime: vi.fn(() => false),
  safeInvoke: safeInvokeMock,
  safeInvokeVoid: vi.fn(),
}))

describe('CoworkView project context helpers', () => {
  beforeEach(() => {
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
        addedAt: 100,
      },
      {
        id: 'link-2',
        kind: 'link',
        path: 'https://example.com/broken',
        label: 'Broken',
        enabled: true,
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
})
