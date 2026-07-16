import type { Personality } from '../stores/personalityStore'
import type { AgentRole } from '../stores/crewStore'
import { safeInvoke } from './safeInvoke'

export type DefaultPersonalityDef = {
  id: string
  name: string
  role: AgentRole
  goal: string
  systemPrompt: string
  skillsMarkdown: string
  temperature: number
  icon: string
  isDefault: boolean
}

export const DEFAULT_PERSONALITIES: DefaultPersonalityDef[] = [
  {
    id: 'pers-standard-coder',
    name: 'Coder',
    role: 'executor',
    goal: 'Build maintainable software with verified results.',
    systemPrompt: `You are a senior software engineer and technical advisor.
- Produce clean, idiomatic, maintainable code.
- Explain decisions, trade-offs, and edge cases clearly.
- Prioritize correctness, security, performance, and tests.
- Use modern patterns only when they improve the result.
- Give concrete examples and actionable review notes.
- Respond in the user's language; keep code and code comments in English.`,
    skillsMarkdown: '',
    temperature: 0.2,
    icon: '💻',
    isDefault: true,
  },
  {
    id: 'pers-standard-creative',
    name: 'Creative',
    role: 'custom',
    goal: 'Turn original ideas into practical directions.',
    systemPrompt: `You are a creative strategist and brainstorming partner.
- Explore multiple distinct directions before converging.
- Combine ideas from different domains.
- Turn unconventional concepts into practical experiments.
- Ask questions that reveal new options.
- Use analogies when they improve clarity.
- Respond in the user's language.`,
    skillsMarkdown: '',
    temperature: 0.8,
    icon: '🎨',
    isDefault: false,
  },
  {
    id: 'pers-standard-analyst',
    name: 'Analyst',
    role: 'analyst',
    goal: 'Turn evidence into analysis and actionable recommendations.',
    systemPrompt: `You are a rigorous analyst and strategic advisor.
- Separate facts, assumptions, and unknowns.
- Structure evidence into patterns, risks, and opportunities.
- Quantify claims when possible.
- Explain confidence and limitations.
- End with prioritized, actionable recommendations.
- Respond in the user's language.`,
    skillsMarkdown: '',
    temperature: 0.1,
    icon: '📊',
    isDefault: false,
  },
  {
    id: 'pers-standard-mentor',
    name: 'Mentor',
    role: 'custom',
    goal: 'Teach complex topics step by step.',
    systemPrompt: `You are a patient mentor and teacher.
- Explain complex topics step by step.
- Adapt depth and examples to the learner's context.
- Check understanding without being patronizing.
- Build on existing knowledge and suggest practice.
- Respond in the user's language.`,
    skillsMarkdown: '',
    temperature: 0.4,
    icon: '🎓',
    isDefault: false,
  },
  {
    id: 'pers-standard-assistant',
    name: 'Assistant',
    role: 'executor',
    goal: 'Move everyday work forward with clear next actions.',
    systemPrompt: `You are a concise, dependable execution partner.
- Clarify ambiguity before committing to a path.
- Prioritize by urgency and impact.
- Organize information so the next action is obvious.
- Proactively surface blockers and useful follow-ups.
- Preserve relevant conversation context.
- Respond in the user's language.`,
    skillsMarkdown: '',
    temperature: 0.3,
    icon: '🤖',
    isDefault: false,
  },
]

export async function seedDefaultPersonalities(): Promise<void> {
  try {
    const existing = await safeInvoke<Personality[]>('personality_list', undefined, [])
    const existingById = new Map(existing.map((personality) => [personality.id, personality]))

    for (const def of DEFAULT_PERSONALITIES) {
      const current = existingById.get(def.id)
      if (!current || current.created_at === current.updated_at) {
        await safeInvoke('personality_upsert', {
          id: def.id,
          name: def.name,
          description: def.goal,
          role: def.role,
          goal: def.goal,
          systemPrompt: def.systemPrompt,
          skillsMarkdown: def.skillsMarkdown,
          temperature: def.temperature,
          modelOverride: null,
          icon: def.icon,
          isDefault: def.isDefault,
        }, undefined)
      }
    }
  } catch {
    // DB not available in test environment
  }
}

export type DefaultMemoryEntry = {
  id: string
  scope: string
  category: string
  key: string
  content: string
  confidence: number
}

export const DEFAULT_MEMORY_ENTRIES: DefaultMemoryEntry[] = [
  {
    id: 'mem-sys-intro',
    scope: 'system',
    category: 'context',
    key: 'app-identity',
    content: 'Open_Cowork is an AI-powered desktop productivity app with chat, task orchestration, multi-agent capabilities, and extensive tool integration.',
    confidence: 1.0,
  },
  {
    id: 'mem-sys-lang',
    scope: 'system',
    category: 'preferences',
    key: 'language',
    content: 'Default language is English. All UI text and answers in English. Code comments are in English.',
    confidence: 1.0,
  },
  {
    id: 'mem-sys-stack',
    scope: 'system',
    category: 'context',
    key: 'tech-stack',
    content: 'Tech stack: Tauri 2 (Rust backend) + React 19 (TypeScript frontend) + SQLite (persistence) + Ollama (LLM).',
    confidence: 1.0,
  },
  {
    id: 'mem-sys-safety',
    scope: 'system',
    category: 'rules',
    key: 'file-safety',
    content: 'File access is limited to approved folders. All file operations are audited.',
    confidence: 1.0,
  },
  {
    id: 'mem-sys-policy',
    scope: 'system',
    category: 'rules',
    key: 'policy-enforcement',
    content: 'Policy engine enforces access control for tools, MCP calls, web fetch, and file extraction.',
    confidence: 1.0,
  },
]

export const DEFAULT_PROFILE_ENTRIES = [
  { key: 'sprache', value: 'English', source: 'system' },
  { key: 'erfahrung', value: 'Fortgeschritten', source: 'system' },
  { key: 'focus', value: 'Productivity and code quality', source: 'system' },
]

export async function seedDefaultMemory(): Promise<void> {
  try {
    const existing = await safeInvoke<Array<{ id: string }>>('memory_search', {
      scope: 'system', category: null, keyword: null, limit: 100,
    }, [])
    const existingIds = new Set(existing.map(e => e.id))

    for (const entry of DEFAULT_MEMORY_ENTRIES) {
      if (!existingIds.has(entry.id)) {
        await safeInvoke('memory_upsert', {
          id: entry.id,
          scope: entry.scope,
          category: entry.category,
          key: entry.key,
          content: entry.content,
          confidence: entry.confidence,
        }, undefined)
      }
    }

    const profileEntries = await safeInvoke<Array<{ key: string }>>('user_profile_list', undefined, [])
    const existingKeys = new Set(profileEntries.map(p => p.key))

    for (const profile of DEFAULT_PROFILE_ENTRIES) {
      if (!existingKeys.has(profile.key)) {
        const id = `prof-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
        await safeInvoke('user_profile_upsert', {
          id,
          key: profile.key,
          value: profile.value,
          source: profile.source,
          confidence: 1.0,
        }, undefined)
      }
    }
  } catch {
    // DB not available in test environment
  }
}

export function buildSystemPromptFromPersonality(
  personality: { system_prompt: string; name: string } | null,
  globalInstruction: string,
  memoryHints: Array<{ key: string; content: string }>,
): string {
  const parts: string[] = []

  if (personality) {
    parts.push(`[Personality: ${personality.name}]`)
    parts.push(personality.system_prompt)
  } else {
    parts.push('Du bist Open_Cowork, ein KI-Assistent for Produktivitaet und Software-Entwicklung. Answer in English.')
  }

  if (globalInstruction.trim()) {
    parts.push(`\n[Global instructions]\n${globalInstruction}`)
  }

  if (memoryHints.length > 0) {
    parts.push('\n[Memory context]')
    for (const hint of memoryHints.slice(0, 10)) {
      parts.push(`- ${hint.key}: ${hint.content}`)
    }
  }

  return parts.join('\n')
}
