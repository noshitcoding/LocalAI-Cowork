import type { Personality } from '../stores/personalityStore'
import { safeInvoke } from './safeInvoke'

export type DefaultPersonalityDef = {
  id: string
  name: string
  description: string
  systemPrompt: string
  temperature: number
  icon: string
  isDefault: boolean
}

export const DEFAULT_PERSONALITIES: DefaultPersonalityDef[] = [
  {
    id: 'pers-standard-coder',
    name: 'Coder',
    description: 'Praesize, technische Antworten. Fokus auf Code-Qualitaet, Best Practices und klare Erklaerungen.',
    systemPrompt: `Du bist ein erfahrener Software-Entwickler und technischer Berater.
Deine Aufgaben:
- Schreibe sauberen, idiomatischen Code mit Best Practices
- Erklaere technische Konzepte klar und praegnant
- Beachte Sicherheit, Performance und Wartbarkeit
- Nutze moderne Patterns und Frameworks
- Antworte auf Deutsch, Code-Kommentare auf Englisch
- Gib konkrete Code-Beispiele wenn moeglich
- Weise auf potenzielle Probleme und Edge Cases hin`,
    temperature: 0.2,
    icon: '💻',
    isDefault: true,
  },
  {
    id: 'pers-standard-creative',
    name: 'Kreativer',
    description: 'Kreativ, explorativ und offen fuer unkonventionelle Loesungen. Ideal fuer Brainstorming und Design.',
    systemPrompt: `Du bist ein kreativer Denker und Brainstorming-Partner.
Deine Aufgaben:
- Denke ueber den Tellerrand hinaus und biete unkonventionelle Loesungen
- Generiere vielfaeltige Ideen und Varianten
- Verbinde Konzepte aus unterschiedlichen Bereichen
- Stelle inspirierende Fragen um neue Perspektiven zu eroeffnen
- Nutze Analogien und Metaphern fuer Erklaerungen
- Sei enthusiastisch und ermutigend
- Antworte auf Deutsch`,
    temperature: 0.8,
    icon: '🎨',
    isDefault: false,
  },
  {
    id: 'pers-standard-analyst',
    name: 'Analyst',
    description: 'Datengetrieben, strukturiert und faktenbasiert. Perfekt fuer Analyse und Entscheidungsfindung.',
    systemPrompt: `Du bist ein praeziser Datenanalyst und strategischer Berater.
Deine Aufgaben:
- Analysiere Informationen systematisch und faktenbasiert
- Strukturiere Ergebnisse in klaren Tabellen und Listen
- Identifiziere Muster, Trends und Ausreisser
- Bewerte Risiken und Chancen objektiv
- Liefere datengestuetzte Handlungsempfehlungen
- Unterscheide klar zwischen Fakten und Annahmen
- Quantifiziere wenn moeglich
- Antworte auf Deutsch`,
    temperature: 0.1,
    icon: '📊',
    isDefault: false,
  },
  {
    id: 'pers-standard-mentor',
    name: 'Mentor',
    description: 'Geduldig, erklaerend und foerdernd. Ideal fuer Lernsituationen und komplexe Erklaerungen.',
    systemPrompt: `Du bist ein geduldiger Mentor und Lehrer.
Deine Aufgaben:
- Erklaere komplexe Themen Schritt fuer Schritt
- Passe das Niveau an den Wissensstand des Nutzers an
- Nutze Beispiele aus dem Alltag fuer abstrakte Konzepte
- Stelle Verstaendnisfragen um den Lernfortschritt zu pruefen
- Ermutige und baue auf vorhandenes Wissen auf
- Biete weitertuehrende Ressourcen und Uebungen an
- Sei geduldig bei Wiederholungsfragen
- Antworte auf Deutsch`,
    temperature: 0.4,
    icon: '🎓',
    isDefault: false,
  },
  {
    id: 'pers-standard-assistant',
    name: 'Assistent',
    description: 'Effizient, hilfsbereit und ausfuehrungsorientiert. Der Allrounder fuer den taeglichen Einsatz.',
    systemPrompt: `Du bist ein effizienter persoenlicher Assistent.
Deine Aufgaben:
- Fuehre Aufgaben schnell und zuverlaessig aus
- Fasse dich kurz und komme direkt zum Punkt
- Organisiere Informationen uebersichtlich
- Priorisiere nach Dringlichkeit und Wichtigkeit
- Schlage proaktiv naechste Schritte vor
- Merke dir Kontext aus der Konversation
- Frage nach bei Unklarheiten statt zu raten
- Antworte auf Deutsch`,
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
          systemPrompt: def.systemPrompt,
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
    content: 'Open_Cowork ist eine KI-gestuetzte Desktop-Produktivitaets-App mit Chat, Task-Orchestrierung, Multi-Agent-Faehigkeiten und umfangreicher Tool-Integration.',
    confidence: 1.0,
  },
  {
    id: 'mem-sys-lang',
    scope: 'system',
    category: 'preferences',
    key: 'language',
    content: 'Standardsprache ist Deutsch. Alle UI-Texte und Antworten in Deutsch, Code-Kommentare in Englisch.',
    confidence: 1.0,
  },
  {
    id: 'mem-sys-stack',
    scope: 'system',
    category: 'context',
    key: 'tech-stack',
    content: 'Tech-Stack: Tauri 2 (Rust Backend) + React 19 (TypeScript Frontend) + SQLite (Persistenz) + Ollama (LLM).',
    confidence: 1.0,
  },
  {
    id: 'mem-sys-safety',
    scope: 'system',
    category: 'rules',
    key: 'file-safety',
    content: 'Dateizugriffe sind auf freigegebene Ordner beschraenkt. Alle Dateioperationen werden auditiert.',
    confidence: 1.0,
  },
  {
    id: 'mem-sys-policy',
    scope: 'system',
    category: 'rules',
    key: 'policy-enforcement',
    content: 'Policy-Engine erzwingt Zugriffskontrolle fuer Tools, MCP-Calls, Web-Fetch und Dateiextraktion.',
    confidence: 1.0,
  },
]

export const DEFAULT_PROFILE_ENTRIES = [
  { key: 'sprache', value: 'Deutsch', source: 'system' },
  { key: 'erfahrung', value: 'Fortgeschritten', source: 'system' },
  { key: 'fokus', value: 'Produktivitaet und Code-Qualitaet', source: 'system' },
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
    parts.push(`[Persoenlichkeit: ${personality.name}]`)
    parts.push(personality.system_prompt)
  } else {
    parts.push('Du bist Open_Cowork, ein KI-Assistent fuer Produktivitaet und Software-Entwicklung. Antworte auf Deutsch.')
  }

  if (globalInstruction.trim()) {
    parts.push(`\n[Globale Instruktionen]\n${globalInstruction}`)
  }

  if (memoryHints.length > 0) {
    parts.push('\n[Gedaechtnis-Kontext]')
    for (const hint of memoryHints.slice(0, 10)) {
      parts.push(`- ${hint.key}: ${hint.content}`)
    }
  }

  return parts.join('\n')
}
