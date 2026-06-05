import type { Personality } from '../stores/personalityStore'
import type { AgentRole } from '../stores/crewStore'
import { safeInvoke } from './safeInvoke'

export type DefaultPersonalityDef = {
  id: string
  name: string
  description: string
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
    description: 'Precise technical answers. Focus on code quality, best practices, and clear explanations.',
    role: 'executor',
    goal: 'Precise technical answers focused on code quality, best practices, and clear explanations.',
    systemPrompt: `Du bist ein erfahrener Software-Entwickler und technischer Berater.
Your tasks:
- Schreibe sauberen, idiomatischen Code mit best practices
- Erklaere technische Konzepte klar und praegnant
- Beachte security, Performance und Wartbarkeit
- Use moderne Patterns und Frameworks
- Answer in English, code comments in English
- Gib konkrete Code-Beispiele wenn moeglich
- Weise auf potenzielle problems und Edge Cases hin`,
    skillsMarkdown: '',
    temperature: 0.2,
    icon: '💻',
    isDefault: true,
  },
  {
    id: 'pers-standard-creative',
    name: 'Creativeer',
    description: 'Creative, exploratory, and open to unconventional solutions. Ideal for brainstorming and design.',
    role: 'custom',
    goal: 'Work creatively, exploratively, and openly toward unconventional solutions.',
    systemPrompt: `Du bist ein kreativer Denker und Brainstorming-Partner.
Your tasks:
- Think beyond the obvious and offer unconventional solutions
- Generiere vielfaeltige Ideen und Varianten
- Verbinde Konzepte aus unterschiedlichen Bereichen
- Ask inspiring questions to open new perspectives
- Use Analogien und Metaphern for explanations
- Sei enthusiastisch und ermutigend
- Answer in English`,
    skillsMarkdown: '',
    temperature: 0.8,
    icon: '🎨',
    isDefault: false,
  },
  {
    id: 'pers-standard-analyst',
    name: 'Analyst',
    description: 'Data-driven, structured, and fact-based. Perfect for analysis and decision-making.',
    role: 'analyst',
    goal: 'Analyze information in a data-driven, structured, and fact-based way.',
    systemPrompt: `Du bist ein praeziser Datenanalyst und strategischer Berater.
Your tasks:
- Analyze Informationen systematisch und faktenbasiert
- Structure results in clear tables and lists
- Identifiziere Muster, Trends und Ausreisser
- Bewerte Risiken und Chancen objektiv
- Liefere datengestuetzte Handlungsempfehlungen
- Unterscheide klar zwischen Fakten und Annahmen
- Quantifiziere wenn moeglich
- Answer in English`,
    skillsMarkdown: '',
    temperature: 0.1,
    icon: '📊',
    isDefault: false,
  },
  {
    id: 'pers-standard-mentor',
    name: 'Mentor',
    description: 'Patient, explanatory, and supportive. Ideal for learning situations and complex explanations.',
    role: 'custom',
    goal: 'Geduldig erklaeren, Wissen aufbauen und komplexe Inhalte verstaendlich machen.',
    systemPrompt: `Du bist ein geduldiger Mentor und Lehrer.
Your tasks:
- Erklaere komplexe Themen Schritt for Schritt
- Adapt the level to the user's knowledge
- Use Beispiele aus dem Alltag for abstrakte Konzepte
- Ask comprehension questions to check learning progress
- Encourage and build on existing knowledge
- Offer further resources and exercises
- Sei geduldig bei Wiederholungsfragen
- Answer in English`,
    skillsMarkdown: '',
    temperature: 0.4,
    icon: '🎓',
    isDefault: false,
  },
  {
    id: 'pers-standard-assistant',
    name: 'Assistent',
    description: 'Efficient, helpful, and execution-oriented. The all-rounder for daily use.',
    role: 'executor',
    goal: 'Tasks effizient, hilfsready und ausfuehrungsorientiert erledigen.',
    systemPrompt: `Du bist ein effizienter persoenlicher Assistent.
Your tasks:
- Fuehre Tasks schnell und zuverlaessig aus
- Fasse dich kurz und komme direkt zum Punkt
- Organize information clearly
- Priorisiere nach Dringlichkeit und Wichtigkeit
- Proactively suggest next steps
- Remember context from the conversation
- Ask when something is unclear instead of guessing
- Answer in English`,
    skillsMarkdown: '',
    temperature: 0.3,
    icon: '🤖',
    isDefault: false,
  },
]

export async function seedDefaultPersonalities(): Promise<void> {
  try {
    const existing = await safeInvoke<Personality[]>('personality_list', undefined, [])
    const existingIds = new Set(existing.map(p => p.id))

    for (const def of DEFAULT_PERSONALITIES) {
      if (!existingIds.has(def.id)) {
        await safeInvoke('personality_upsert', {
          id: def.id,
          name: def.name,
          description: def.description,
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
