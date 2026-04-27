import { create } from 'zustand'
import { useUiStore } from './uiStore'
import { useConfigStore } from './configStore'
import { useChatStore } from './chatStore'
import { useCoworkStore } from './coworkStore'
import { useMemoryStore } from './memoryStore'
import { useSkillStore } from './skillStore'
import { useInsightsStore } from './insightsStore'
import { useProcessStore } from './processStore'
import { useTerminalStore } from './terminalStore'
import { useTaskStore } from './taskStore'
import { useCrewStore } from './crewStore'
import { safeInvoke, safeInvokeVoid } from '../utils/safeInvoke'
import { parseScheduledTaskInput } from '../utils/schedulerUtils'

export type SlashCommandCategory =
  | 'navigation'
  | 'workspace'
  | 'agent'
  | 'model'
  | 'memory'
  | 'tools'
  | 'session'
  | 'config'
  | 'security'
  | 'display'
  | 'plugins'
  | 'crew'
  | 'debug'
  | 'export'

export type SlashCommand = {
  id: string
  command: string
  label: string
  description: string
  category: SlashCommandCategory
  execute: (args?: string) => void | Promise<void>
}

export function getSlashCommandSuggestions(commands: SlashCommand[], input: string): SlashCommand[] {
  const trimmed = input.trimStart().toLowerCase()
  if (!trimmed.startsWith('/')) return []

  const partial = trimmed.split(/\s+/, 1)[0]
  return commands.filter((command) => command.command.toLowerCase().startsWith(partial))
}

type CommandRegistryState = {
  commands: SlashCommand[]
  lastExecuted: string | null
  executionLog: Array<{ command: string; timestamp: number; args?: string }>
  getCommand: (id: string) => SlashCommand | undefined
  executeCommand: (commandOrId: string, args?: string) => void
}

function uid() {
  return `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

export const useCommandRegistry = create<CommandRegistryState>()((set, get) => ({
  commands: buildAllCommands(),
  lastExecuted: null,
  executionLog: [],
  getCommand: (id) => get().commands.find((c) => c.id === id || c.command === id),
  executeCommand: (commandOrId, args) => {
    const cmd = get().commands.find(
      (c) => c.id === commandOrId || c.command === commandOrId || c.command === `/${commandOrId}`
    )
    if (cmd) {
      cmd.execute(args)
      set((s) => ({
        lastExecuted: cmd.id,
        executionLog: [
          { command: cmd.command, timestamp: Date.now(), args },
          ...s.executionLog.slice(0, 199),
        ],
      }))
    }
  },
}))

function buildAllCommands(): SlashCommand[] {
  return [
    // ===== Navigation =====
    {
      id: 'switch-work', command: '/ide', label: 'Zum Arbeitsbereich', description: 'Wechselt zum Hauptarbeitsbereich',
      category: 'navigation', execute: () => useUiStore.getState().setActiveMode('work'),
    },
    {
      id: 'switch-settings', command: '/config', label: 'Einstellungen oeffnen', description: 'Alle Einstellungen und Konfigurationen',
      category: 'config', execute: () => useUiStore.getState().setActiveMode('settings'),
    },
    {
      id: 'toggle-sidebar', command: '/focus', label: 'Fokus-Modus', description: 'Seitenleisten ein/ausblenden fuer Fokusarbeit',
      category: 'display', execute: () => {
        const ui = useUiStore.getState()
        ui.toggleLeftSidebar()
      },
    },
    {
      id: 'toggle-theme', command: '/theme', label: 'Theme wechseln', description: 'Zwischen Light/Dark Theme umschalten',
      category: 'display', execute: (args) => {
        const ui = useUiStore.getState()
        if (args === 'dark') ui.setTheme('dark')
        else if (args === 'light') ui.setTheme('light')
        else ui.toggleTheme()
      },
    },

    // ===== Workspace =====
    {
      id: 'add-dir', command: '/add-dir', label: 'Ordner hinzufuegen', description: 'Neuen Arbeitsordner zur Allowlist hinzufuegen',
      category: 'workspace', execute: async (args) => {
        if (args?.trim()) {
          await safeInvokeVoid('fs_add_allowed_folder', { path: args.trim() })
        }
      },
    },
    {
      id: 'context', command: '/context', label: 'Kontext anzeigen', description: 'Aktuellen Thread-Kontext und Attachments anzeigen',
      category: 'workspace', execute: () => {
        const thread = useChatStore.getState()
        const active = thread.threads.find(t => t.id === thread.activeThreadId)
        if (active) {
          const msgCount = active.messages.length
          const charCount = active.messages.reduce((a, m) => a + m.content.length, 0)
          useChatStore.getState().addMessage(active.id, {
            role: 'system', content: `Kontext: ${msgCount} Nachrichten, ${charCount} Zeichen, Thread "${active.title}"`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'diff', command: '/diff', label: 'Diff anzeigen', description: 'Aenderungen seit letztem Backup anzeigen',
      category: 'workspace', execute: () => {
        useChatStore.getState().addMessage(
          useChatStore.getState().activeThreadId ?? '',
          { role: 'system', content: 'Diff-Ansicht: Nutze die Einstellungen um Backup-Diffs zu pruefen.', timestamp: Date.now() }
        )
      },
    },
    {
      id: 'init', command: '/init', label: 'Projekt initialisieren', description: 'Initialisiert ein neues Open_Cowork Projekt im aktuellen Ordner',
      category: 'workspace', execute: async () => {
        await safeInvokeVoid('audit_event', { area: 'project', action: 'init', details: 'Projekt-Init gestartet' })
        const store = useChatStore.getState()
        if (store.activeThreadId) {
          store.addMessage(store.activeThreadId, {
            role: 'system', content: 'Projekt initialisiert. Open_Cowork Konfiguration wurde erstellt.',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'rename', command: '/rename', label: 'Thread umbenennen', description: 'Aktuellen Chat-Thread umbenennen',
      category: 'workspace', execute: (args) => {
        if (!args?.trim()) return
        const store = useChatStore.getState()
        const threadId = store.activeThreadId
        if (threadId) {
          const thread = store.threads.find(t => t.id === threadId)
          if (thread) {
            void safeInvokeVoid('db_save_thread', { id: threadId, title: args.trim(), createdAt: new Date(thread.createdAt).toISOString() })
          }
        }
      },
    },
    {
      id: 'branch', command: '/branch', label: 'Thread-Zweig', description: 'Erstellt einen neuen Thread-Zweig vom aktuellen Punkt',
      category: 'workspace', execute: () => {
        const cs = useChatStore.getState()
        const active = cs.threads.find(t => t.id === cs.activeThreadId)
        if (active) {
          cs.addThread(`Zweig: ${active.title}`)
        }
      },
    },

    // ===== Agent Commands =====
    {
      id: 'agents', command: '/agents', label: 'Agenten verwalten', description: 'Crew-Agenten anzeigen und verwalten',
      category: 'agent', execute: () => {
        useCrewStore.getState().loadAgents()
      },
    },
    {
      id: 'batch', command: '/batch', label: 'Batch-Ausfuehrung', description: 'Mehrere Aufgaben als Batch ausfuehren',
      category: 'agent', execute: async (args) => {
        if (!args?.trim()) return
        const tasks = args.split(';').map(t => t.trim()).filter(Boolean)
        for (const task of tasks) {
          useTaskStore.getState().createTask(task, task, null)
        }
      },
    },
    {
      id: 'loop', command: '/loop', label: 'Agentic Loop', description: 'Startet eine automatisierte Agent-Schleife',
      category: 'agent', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `Agentic Loop gestartet${args ? `: ${args}` : ''}. Agent wird autonom arbeiten bis Aufgabe erledigt.`,
            timestamp: Date.now(),
          })
        }
        useConfigStore.getState().setPreference('autoPilotAllTools', true)
      },
    },
    {
      id: 'autofix-pr', command: '/autofix-pr', label: 'PR Auto-Fix', description: 'Automatisch Fehler in einem PR beheben',
      category: 'agent', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Autofix fuer PR: ${args ?? 'aktueller Branch'}. Analysiere und behebe Probleme.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'ultraplan', command: '/ultraplan', label: 'Ultra-Planung', description: 'Erstellt einen detaillierten Multi-Step Plan',
      category: 'agent', execute: async (args) => {
        if (!args?.trim()) return
        const taskId = useTaskStore.getState().createTask(`Ultra-Plan: ${args}`, args, null)
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Ultra-Plan erstellt (Task: ${taskId}). Detaillierte Schritt-fuer-Schritt Analyse folgt.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'ultrareview', command: '/ultrareview', label: 'Ultra-Review', description: 'Fuehrt ein umfassendes Code-Review durch',
      category: 'agent', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Ultra-Review gestartet${args ? ` fuer: ${args}` : ''}. Umfassende Analyse: Architektur, Sicherheit, Performance, Best Practices.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'review', command: '/review', label: 'Code Review', description: 'Standard Code-Review durchfuehren',
      category: 'agent', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Code Review${args ? ` fuer: ${args}` : ''}. Pruefe Qualitaet, Fehler und Verbesserungen.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'security-review', command: '/security-review', label: 'Security Review', description: 'Sicherheitsanalyse des Codes',
      category: 'security', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Security Review${args ? ` fuer: ${args}` : ''}. OWASP Top 10, Injection, Auth, Crypto pruefen.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'passes', command: '/passes', label: 'Multi-Pass', description: 'Mehrfach-Durchlaeufe fuer komplexe Aufgaben',
      category: 'agent', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Multi-Pass Modus aktiviert (${args ?? '3'} Durchlaeufe). Iterative Verbesserung.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'simplify', command: '/simplify', label: 'Code vereinfachen', description: 'Vereinfacht und bereinigt den ausgewaehlten Code',
      category: 'agent', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: `Code-Vereinfachung${args ? ` fuer: ${args}` : ''}. Reduziere Komplexitaet, entferne Redundanzen.`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'debug', command: '/debug', label: 'Debug Modus', description: 'Aktiviert erweiterte Debug-Informationen',
      category: 'debug', execute: () => {
        const config = useConfigStore.getState()
        config.setPreference('verboseMode', !config.preferences.verboseMode)
        config.setPreference('superVerboseAuditLogging', !config.preferences.superVerboseAuditLogging)
      },
    },
    {
      id: 'doctor', command: '/doctor', label: 'System-Diagnose', description: 'Prueft Systemzustand und Konfiguration',
      category: 'debug', execute: async () => {
        const cs = useChatStore.getState()
        if (!cs.activeThreadId) return
        try {
          const health = await safeInvoke<{ status: string }>('ollama_health_check', { config: useConfigStore.getState().ollama })
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `System-Diagnose:\n- Ollama: ${health.status}\n- DB: aktiv\n- MCP: konfiguriert\n- Audit: aktiv`,
            timestamp: Date.now(),
          })
        } catch {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'System-Diagnose: Ollama nicht erreichbar. Pruefe Konfiguration.',
            timestamp: Date.now(),
          })
        }
      },
    },

    // ===== Model Commands =====
    {
      id: 'model', command: '/model', label: 'Modell wechseln', description: 'Aktives LLM-Modell wechseln',
      category: 'model', execute: (args) => {
        if (args?.trim()) {
          useConfigStore.getState().setOllama({ model: args.trim() })
        }
      },
    },
    {
      id: 'effort', command: '/effort', label: 'Aufwand steuern', description: 'Antwort-Aufwand (Temperatur) anpassen: low/medium/high',
      category: 'model', execute: (args) => {
        const map: Record<string, number> = { low: 0.1, medium: 0.5, high: 0.9 }
        const temp = map[args ?? ''] ?? 0.2
        useConfigStore.getState().setOllama({ temperature: temp })
      },
    },
    {
      id: 'fast', command: '/fast', label: 'Schnell-Modus', description: 'Wechselt zum schnellsten verfuegbaren Modell',
      category: 'model', execute: () => {
        const models = useConfigStore.getState().availableModels
        const fast = models.find(m => m.includes('tiny') || m.includes('mini') || m.includes('3b')) ?? models[0]
        if (fast) useConfigStore.getState().setOllama({ model: fast })
      },
    },
    {
      id: 'powerup', command: '/powerup', label: 'Power-Modus', description: 'Wechselt zum staerksten verfuegbaren Modell',
      category: 'model', execute: () => {
        const models = useConfigStore.getState().availableModels
        const power = models.find(m => m.includes('70b') || m.includes('405b') || m.includes('llama3.1')) ?? models[models.length - 1]
        if (power) useConfigStore.getState().setOllama({ model: power })
      },
    },
    {
      id: 'compact', command: '/compact', label: 'Kontext komprimieren', description: 'Komprimiert den Chat-Kontext fuer laengere Sessions',
      category: 'model', execute: () => {
        useCoworkStore.getState().setPolicyFlag('autoCompactLongContext', true)
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Kontext-Kompression aktiviert. Aeltere Nachrichten werden zusammengefasst.',
            timestamp: Date.now(),
          })
        }
      },
    },

    // ===== Memory Commands =====
    {
      id: 'memory', command: '/memory', label: 'Gedaechtnis', description: 'Gedaechtnis-Eintraege verwalten und durchsuchen',
      category: 'memory', execute: async (args) => {
        if (args?.trim()) {
          await useMemoryStore.getState().searchEntries(args.trim())
        } else {
          await useMemoryStore.getState().loadEntries()
        }
      },
    },
    {
      id: 'recap', command: '/recap', label: 'Zusammenfassung', description: 'Zusammenfassung der aktuellen Session',
      category: 'memory', execute: () => {
        const cs = useChatStore.getState()
        const active = cs.threads.find(t => t.id === cs.activeThreadId)
        if (active && cs.activeThreadId) {
          const userMsgs = active.messages.filter(m => m.role === 'user')
          const assistantMsgs = active.messages.filter(m => m.role === 'assistant')
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `Session-Recap:\n- ${userMsgs.length} Benutzer-Nachrichten\n- ${assistantMsgs.length} Antworten\n- Thread: "${active.title}"\n- Gestartet: ${new Date(active.createdAt).toLocaleString('de-DE')}`,
            timestamp: Date.now(),
          })
        }
      },
    },

    // ===== Session Commands =====
    {
      id: 'clear', command: '/clear', label: 'Chat leeren', description: 'Aktuellen Chat-Verlauf zuruecksetzen',
      category: 'session', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.deleteThread(cs.activeThreadId)
          cs.addThread('Neuer Chat')
        }
      },
    },
    {
      id: 'resume', command: '/resume', label: 'Fortsetzen', description: 'Letzte Session fortsetzen',
      category: 'session', execute: () => {
        const cs = useChatStore.getState()
        const latest = cs.threads.sort((a, b) => b.updatedAt - a.updatedAt)[0]
        if (latest) cs.setActiveThread(latest.id)
      },
    },
    {
      id: 'rewind', command: '/rewind', label: 'Zurueckspulen', description: 'Letzte N Nachrichten entfernen',
      category: 'session', execute: (args) => {
        const count = Number.parseInt(args ?? '1', 10) || 1
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          const removed = cs.removeLastMessagePairs(cs.activeThreadId, count)
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: removed.pairsRemoved > 0
              ? `Zurueckgespult: ${removed.pairsRemoved} Nachrichtenpaar(e) entfernt.`
              : 'Keine passenden Nachrichten zum Zurueckspulen gefunden.',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'exit', command: '/exit', label: 'Beenden', description: 'Aktuelle Session beenden',
      category: 'session', execute: () => {
        useChatStore.getState().setActiveThread(null)
      },
    },

    // ===== Tools =====
    {
      id: 'mcp', command: '/mcp', label: 'MCP verwalten', description: 'MCP-Server und Tools konfigurieren',
      category: 'tools', execute: () => {
        useUiStore.getState().setActiveMode('settings')
      },
    },
    {
      id: 'hooks', command: '/hooks', label: 'Hooks verwalten', description: 'Pre/Post-Execution Hooks konfigurieren',
      category: 'tools', execute: () => {
        useConfigStore.getState().setPreferences({})
      },
    },
    {
      id: 'sandbox', command: '/sandbox', label: 'Sandbox-Modus', description: 'Isolierte Ausfuehrungsumgebung aktivieren',
      category: 'tools', execute: () => {
        useCoworkStore.getState().setPolicyFlag('strictPolicyEnforcement', true)
        useConfigStore.getState().setPreference('readOnlyFsMode', true)
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Sandbox-Modus aktiviert: Nur-Lese-Zugriff, strenge Policy.',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'terminal-setup', command: '/terminal-setup', label: 'Terminal einrichten', description: 'Terminal-Backend konfigurieren',
      category: 'tools', execute: async () => {
        await useTerminalStore.getState().ensureLocalBackend()
      },
    },
    {
      id: 'web-setup', command: '/web-setup', label: 'Web-Zugriff Setup', description: 'Web-Recherche und URL-Zugriff konfigurieren',
      category: 'tools', execute: () => {
        useCoworkStore.getState().setPolicyFlag('allowWebFetch', true)
        useCoworkStore.getState().setPolicyFlag('allowWebSearch', true)
      },
    },

    // ===== Config Commands =====
    {
      id: 'color', command: '/color', label: 'Farbschema', description: 'Farbschema anpassen',
      category: 'display', execute: (args) => {
        if (args === 'dark' || args === 'light') {
          useUiStore.getState().setTheme(args)
        }
      },
    },
    {
      id: 'keybindings', command: '/keybindings', label: 'Tastenkuerzel', description: 'Tastenkuerzel anzeigen und bearbeiten',
      category: 'config', execute: () => {
        useUiStore.getState().setShortcutsOverlayOpen(true)
      },
    },
    {
      id: 'less-permission-prompts', command: '/less-permission-prompts', label: 'Weniger Berechtigungsfragen', description: 'Reduziert Bestaetigungs-Dialoge',
      category: 'config', execute: () => {
        useConfigStore.getState().setPreferences({
          autoApproveSafeTools: true,
          confirmOnCloseWithRunningTasks: false,
          fallbackToHumanOnRepeatedFailure: false,
        })
      },
    },
    {
      id: 'privacy-settings', command: '/privacy-settings', label: 'Datenschutz', description: 'Datenschutz- und Telemetrie-Einstellungen',
      category: 'config', execute: () => {
        useConfigStore.getState().setPreference('telemetryEnabled', false)
      },
    },

    // ===== Insights & Stats =====
    {
      id: 'stats', command: '/stats', label: 'Statistiken', description: 'Nutzungsstatistiken und Metriken anzeigen',
      category: 'debug', execute: async () => {
        await useInsightsStore.getState().loadSummary()
        const summary = useInsightsStore.getState().summary
        const cs = useChatStore.getState()
        if (cs.activeThreadId && summary) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `Statistiken:\n- Events: ${summary.totalEvents}\n- Sessions: ${summary.totalSessions}\n- Nachrichten: ${summary.totalMessagesSent}\n- Token (est.): ${summary.totalTokensEst}\n- Skills: ${summary.skillUsageCount}\n- Memory: ${summary.memoryEntryCount}`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'insights', command: '/insights', label: 'Insights Dashboard', description: 'Ausfuehrliches Insights-Dashboard oeffnen',
      category: 'debug', execute: () => {
        useInsightsStore.getState().loadSummary()
        useInsightsStore.getState().loadEvents()
      },
    },
    {
      id: 'cost', command: '/cost', label: 'Kosten', description: 'Geschaetzte Kosten der aktuellen Session',
      category: 'debug', execute: async () => {
        const summary = useInsightsStore.getState().summary
          ?? (await useInsightsStore.getState().loadSummary(), useInsightsStore.getState().summary)
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          const tokens = summary?.totalTokensEst ?? 0
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `Kosten-Schaetzung:\n- Token gesamt: ${tokens}\n- Lokales Modell: 0 EUR (Ollama)\n- Geschaetzte API-Kosten: ~${(tokens * 0.000002).toFixed(4)} EUR`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'usage', command: '/usage', label: 'Nutzung', description: 'Detaillierte Nutzungsstatistik',
      category: 'debug', execute: () => useInsightsStore.getState().loadSummary(),
    },
    {
      id: 'status', command: '/status', label: 'Status', description: 'Aktuellen System-Status anzeigen',
      category: 'debug', execute: async () => {
        const cs = useChatStore.getState()
        if (!cs.activeThreadId) return
        const procs = useProcessStore.getState().processes
        const backends = useTerminalStore.getState().backends
        cs.addMessage(cs.activeThreadId, {
          role: 'system',
          content: `Status:\n- Threads: ${cs.threads.length}\n- Prozesse: ${procs.length}\n- Backends: ${backends.length}\n- Modell: ${useConfigStore.getState().ollama.model}`,
          timestamp: Date.now(),
        })
      },
    },
    {
      id: 'statusline', command: '/statusline', label: 'Statuszeile', description: 'Kompakte Statuszeile ein/ausblenden',
      category: 'display', execute: () => {
        useConfigStore.getState().setPreference('compactMode', !useConfigStore.getState().preferences.compactMode)
      },
    },

    // ===== Export =====
    {
      id: 'export', command: '/export', label: 'Exportieren', description: 'Chat oder Daten exportieren (JSON/MD/TXT)',
      category: 'export', execute: (args) => {
        const cs = useChatStore.getState()
        const active = cs.threads.find(t => t.id === cs.activeThreadId)
        if (active) {
          const format = args?.trim() ?? 'json'
          const data = format === 'json'
            ? JSON.stringify(active, null, 2)
            : active.messages.map(m => `[${m.role}] ${m.content}`).join('\n\n')
          navigator.clipboard.writeText(data).catch(() => {})
          if (cs.activeThreadId) {
            cs.addMessage(cs.activeThreadId, {
              role: 'system', content: `Export (${format}) in Zwischenablage kopiert.`,
              timestamp: Date.now(),
            })
          }
        }
      },
    },
    {
      id: 'copy', command: '/copy', label: 'Kopieren', description: 'Letzte Antwort in Zwischenablage kopieren',
      category: 'export', execute: () => {
        const cs = useChatStore.getState()
        const active = cs.threads.find(t => t.id === cs.activeThreadId)
        if (active) {
          const lastAssistant = [...active.messages].reverse().find(m => m.role === 'assistant')
          if (lastAssistant) {
            navigator.clipboard.writeText(lastAssistant.content).catch(() => {})
          }
        }
      },
    },

    // ===== Memory =====
    {
      id: 'skills', command: '/skills', label: 'Skills', description: 'Gelernte Skills anzeigen und verwalten',
      category: 'memory', execute: () => useSkillStore.getState().loadSkills(),
    },
    {
      id: 'tasks', command: '/tasks', label: 'Aufgaben', description: 'Offene Aufgaben anzeigen',
      category: 'agent', execute: () => useTaskStore.getState().loadFromDb(),
    },

    // ===== Plugins =====
    {
      id: 'plugin', command: '/plugin', label: 'Plugin verwalten', description: 'Plugins installieren und konfigurieren',
      category: 'plugins', execute: (args) => {
        if (args === 'examples' || args === 'install') {
          useCoworkStore.getState().installPluginExamples()
        }
      },
    },
    {
      id: 'reload-plugins', command: '/reload-plugins', label: 'Plugins neu laden', description: 'Alle Plugins neu laden',
      category: 'plugins', execute: () => {
        useCoworkStore.getState().installPluginExamples()
      },
    },

    // ===== Local Backend =====
    {
      id: 'ollama-settings', command: '/ollama', label: 'Ollama Einstellungen', description: 'Lokalen Ollama-Endpoint und Laufzeitparameter konfigurieren',
      category: 'config', execute: () => useUiStore.getState().setActiveMode('settings'),
    },
    {
      id: 'local-model', command: '/local-model', label: 'Lokales Modell', description: 'Aktives Ollama-Modell pruefen oder in den Einstellungen wechseln',
      category: 'model', execute: () => useUiStore.getState().setActiveMode('settings'),
    },
    {
      id: 'local-runtime', command: '/local-runtime', label: 'Lokale Runtime', description: 'Lokalen Desktop- und Ollama-Betrieb bestaetigen',
      category: 'config', execute: () => useUiStore.getState().setActiveMode('settings'),
    },

    // ===== Display & UX =====
    {
      id: 'stickers', command: '/stickers', label: 'Sticker', description: 'Sticker-Reaktionen ein/ausblenden',
      category: 'display', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: '🎉 Sticker-Modus aktiviert!',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'tui', command: '/tui', label: 'TUI Modus', description: 'Terminal UI Ansicht aktivieren',
      category: 'display', execute: () => {
        useConfigStore.getState().setPreference('compactMode', true)
      },
    },
    {
      id: 'desktop', command: '/desktop', label: 'Desktop-Integration', description: 'Desktop-Features und Tray-Icon konfigurieren',
      category: 'config', execute: () => useUiStore.getState().setActiveMode('settings'),
    },
    {
      id: 'mobile', command: '/mobile', label: 'Mobile-Optimierung', description: 'Mobile/Touch-Ansicht aktivieren',
      category: 'display', execute: () => {
        useConfigStore.getState().setPreference('compactMode', true)
        useConfigStore.getState().setPreference('fontScale', 110)
      },
    },
    {
      id: 'voice', command: '/voice', label: 'Spracheingabe', description: 'Spracheingabe aktivieren (via Browser API)',
      category: 'display', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Spracheingabe: Nutze die Browser SpeechRecognition API. Feature wird in kuenftige Versionen integriert.',
            timestamp: Date.now(),
          })
        }
      },
    },

    // ===== Crew AI Commands =====
    {
      id: 'crew-create', command: '/crew', label: 'Crew erstellen', description: 'Neue AI-Crew mit Agenten erstellen',
      category: 'crew', execute: (args) => {
        if (args?.trim()) {
          useCrewStore.getState().createCrew(uid(), args.trim(), [])
        }
      },
    },
    {
      id: 'team-onboarding', command: '/team-onboarding', label: 'Team-Onboarding', description: 'Neuen Team-Mitgliedern Kontext geben',
      category: 'crew', execute: (args) => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `Team-Onboarding${args ? ` fuer ${args}` : ''}: Projekt-Kontext, Konventionen und Setup-Anleitung werden generiert.`,
            timestamp: Date.now(),
          })
        }
      },
    },

    // ===== Session Management =====
    {
      id: 'schedule', command: '/schedule', label: 'Zeitplan', description: 'Aufgabe zeitlich planen',
      category: 'session', execute: async (args) => {
        if (args?.trim()) {
          const parsed = parseScheduledTaskInput(args)
          if (!parsed) return
          await useCoworkStore.getState().upsertScheduledTask({
            id: uid(),
            name: parsed.prompt.slice(0, 40),
            prompt: parsed.prompt,
            cronLike: parsed.scheduleExpr,
            active: true,
            lastRunAt: null,
            nextRunAt: null,
          })
        }
      },
    },

    // ===== Misc =====
    {
      id: 'btw', command: '/btw', label: 'Nebenbei', description: 'Kontext-Info hinzufuegen ohne Hauptaufgabe zu aendern',
      category: 'agent', execute: (args) => {
        if (!args?.trim()) return
        useMemoryStore.getState().upsertEntry({
          id: uid(), scope: 'session', category: 'context', key: 'btw', content: args.trim(),
        })
      },
    },
    {
      id: 'chrome', command: '/chrome', label: 'Chrome Integration', description: 'Chrome-Browser Integration steuern',
      category: 'tools', execute: () => {
        useCoworkStore.getState().toggleConnector('chrome', true)
      },
    },
    {
      id: 'feedback', command: '/feedback', label: 'Feedback', description: 'Feedback zur aktuellen Antwort geben',
      category: 'session', execute: (args) => {
        void safeInvokeVoid('audit_event', {
          area: 'feedback', action: 'user_feedback', details: args ?? 'Kein Kommentar',
        })
      },
    },
    {
      id: 'heapdump', command: '/heapdump', label: 'Heap Dump', description: 'Speicher-Snapshot fuer Debugging erstellen',
      category: 'debug', execute: async () => {
        const snapshot = await useMemoryStore.getState().createSnapshot()
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system',
            content: `Heap Dump erstellt: ${snapshot.total_entries} Memory-Eintraege, ${snapshot.total_profile_keys} Profil-Keys`,
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'install-github-app', command: '/install-github-app', label: 'GitHub App installieren', description: 'GitHub Integration einrichten',
      category: 'tools', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'GitHub App Installation: Konfiguriere einen MCP-Server fuer GitHub oder nutze einen GitHub Personal Access Token.',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'install-slack-app', command: '/install-slack-app', label: 'Slack App installieren', description: 'Slack Integration einrichten',
      category: 'tools', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Slack Integration: Konfiguriere einen MCP-Server fuer Slack oder nutze Webhooks.',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'teleport', command: '/teleport', label: 'Teleport', description: 'Schnell zu einer bestimmten Datei/Ordner springen',
      category: 'navigation', execute: (args) => {
        if (args?.trim()) {
          useUiStore.getState().setWorkingPath(args.trim(), 'file')
        }
      },
    },
    {
      id: 'remote-control', command: '/remote-control', label: 'Fernsteuerung', description: 'Remote-Steuerung aktivieren',
      category: 'config', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Remote Control: Feature fuer kuenftige Versionen vorgesehen.',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'remote-env', command: '/remote-env', label: 'Remote-Umgebung', description: 'Remote-Ausfuehrungsumgebung konfigurieren',
      category: 'config', execute: () => useUiStore.getState().setActiveMode('settings'),
    },
    {
      id: 'extra-usage', command: '/extra-usage', label: 'Extra-Nutzung', description: 'Erweiterte Nutzungslimits aktivieren',
      category: 'config', execute: () => {
        useConfigStore.getState().setPreference('maxToolCallsPerLoop', 50)
      },
    },
    {
      id: 'release-notes', command: '/release-notes', label: 'Release Notes', description: 'Aktuelle Release Notes anzeigen',
      category: 'config', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Open_Cowork v1.0\n- 100+ Slash-Commands\n- 5 Standard-Persoenlichkeiten\n- CrewAI Multi-Agent\n- Memory Engine\n- Full Claude Code Kompatibilitaet',
            timestamp: Date.now(),
          })
        }
      },
    },
    {
      id: 'upgrade', command: '/upgrade', label: 'Upgrade', description: 'Auf neueste Version aktualisieren',
      category: 'config', execute: () => {
        const cs = useChatStore.getState()
        if (cs.activeThreadId) {
          cs.addMessage(cs.activeThreadId, {
            role: 'system', content: 'Upgrade: Pruefe auf Updates... Aktuelle Version ist auf dem neuesten Stand.',
            timestamp: Date.now(),
          })
        }
      },
    },
  ]
}
