import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Personality } from '../stores/personalityStore'
import { safeInvoke } from './safeInvoke'
import { DEFAULT_PERSONALITIES, seedDefaultPersonalities } from './defaultSeeds'

vi.mock('./safeInvoke', () => ({ safeInvoke: vi.fn() }))

const safeInvokeMock = vi.mocked(safeInvoke)

function storedPersonality(id: string, updatedAt = '2026-01-01T00:00:00Z'): Personality {
  const definition = DEFAULT_PERSONALITIES.find((item) => item.id === id)!
  return {
    id,
    name: definition.name,
    description: definition.goal,
    role: definition.role,
    goal: definition.goal,
    system_prompt: definition.systemPrompt,
    skills_markdown: definition.skillsMarkdown,
    temperature: definition.temperature,
    model_override: null,
    icon: definition.icon,
    is_default: definition.isDefault,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: updatedAt,
  }
}

describe('seedDefaultPersonalities', () => {
  beforeEach(() => {
    safeInvokeMock.mockReset()
  })

  it('adds every missing default personality', async () => {
    safeInvokeMock.mockImplementation(async (command, _args, fallback) => (
      command === 'personality_list' ? [] : fallback
    ))

    await seedDefaultPersonalities()

    const upserts = safeInvokeMock.mock.calls.filter(([command]) => command === 'personality_upsert')
    expect(upserts).toHaveLength(DEFAULT_PERSONALITIES.length)
    expect(upserts.map(([, args]) => (args as { id: string }).id)).toEqual(
      DEFAULT_PERSONALITIES.map(({ id }) => id),
    )
  })

  it('refreshes untouched defaults without overwriting an edited profile', async () => {
    const untouched = storedPersonality('pers-standard-coder')
    const edited = storedPersonality('pers-standard-creative', '2026-01-02T00:00:00Z')
    safeInvokeMock.mockImplementation(async (command, _args, fallback) => (
      command === 'personality_list' ? [untouched, edited] : fallback
    ))

    await seedDefaultPersonalities()

    const upsertedIds = safeInvokeMock.mock.calls
      .filter(([command]) => command === 'personality_upsert')
      .map(([, args]) => (args as { id: string }).id)
    expect(upsertedIds).toContain(untouched.id)
    expect(upsertedIds).not.toContain(edited.id)
  })
})
