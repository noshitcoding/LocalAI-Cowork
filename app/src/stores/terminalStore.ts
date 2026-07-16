import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { create } from 'zustand'
import { safeInvoke, hasTauriRuntime } from '../utils/safeInvoke'

export type TerminalBackend = {
  id: string
  name: string
  backend_type: string
  config_json: string
  status: string
  last_connected_at: string | null
  created_at: string
  updated_at: string
}

export type BackendExecResponse = {
  backendId: string
  stdout: string
  stderr: string
  exitCode: number | null
  timedOut: boolean
}

export type TerminalSessionStatus = 'idle' | 'running' | 'exited' | 'error'
export type TerminalSessionKind = 'manual' | 'ai'
export type TerminalPersistenceMode = 'runtime' | 'scrollback' | 'restore-tabs'

export type TerminalSession = {
  id: string
  threadId: string
  title: string
  shell: string
  cwd: string
  output: string
  status: TerminalSessionStatus
  kind: TerminalSessionKind
  hidden: boolean
  createdAt: number
  updatedAt: number
  currentAiCommand?: {
    marker: string
    command: string
    startedAt: number
    intervention: boolean
  }
}

export type TerminalOutputEvent = {
  sessionId: string
  stream: 'stdout' | 'stderr' | 'system'
  data: string
}

export type TerminalExitEvent = {
  sessionId: string
  exitCode: number | null
  reason: string
}

export type AiTerminalCommandResult = {
  sessionId: string
  stdout: string
  stderr: string
  exitCode: number | null
  currentCwd?: string
  interruptedByUser: boolean
  timedOut: boolean
}

type CreateSessionInput = {
  threadId: string
  cwd: string
  title?: string
  kind?: TerminalSessionKind
  hidden?: boolean
}

type RunAiCommandInput = {
  threadId: string
  command: string
  cwd: string
  timeoutMs?: number
}

type PendingAiCommand = {
  sessionId: string
  threadId: string
  marker: string
  cwdMarker: string
  ignoredEchoLines: string[]
  stdout: string[]
  stderr: string[]
  currentCwd?: string
  exitCode: number | null
  intervention: boolean
  resolve: (result: AiTerminalCommandResult) => void
  reject: (error: Error) => void
  timer: ReturnType<typeof setTimeout> | null
}

type TerminalState = {
  backends: TerminalBackend[]
  loading: boolean
  error: string | null

  sessionsByThread: Record<string, TerminalSession[]>
  activeSessionIds: Record<string, string | null>
  dockOpenByThread: Record<string, boolean>
  dockHeightByThread: Record<string, number>
  hiddenActivityByThread: Record<string, boolean>
  activeAiThreadId: string | null

  loadBackends: () => Promise<void>
  upsertBackend: (b: { id: string; name: string; backendType: string; configJson: string }) => Promise<boolean>
  deleteBackend: (id: string) => Promise<void>
  execCommand: (backendId: string, command: string, workingDir?: string, timeoutMs?: number) => Promise<BackendExecResponse>
  ensureLocalBackend: () => Promise<TerminalBackend>

  ensureListeners: () => void
  setActiveAiThread: (threadId: string | null) => void
  getThreadSessions: (threadId: string) => TerminalSession[]
  getActiveSession: (threadId: string) => TerminalSession | null
  setActiveSession: (threadId: string, sessionId: string) => void
  setDockOpen: (threadId: string, open: boolean) => void
  setDockHeight: (threadId: string, height: number) => void
  clearHiddenActivity: (threadId: string) => void
  createSession: (input: CreateSessionInput) => Promise<TerminalSession>
  writeToSession: (sessionId: string, data: string) => Promise<void>
  resizeSession: (sessionId: string, cols: number, rows: number) => Promise<void>
  interruptSession: (sessionId: string) => Promise<void>
  killSession: (sessionId: string) => Promise<void>
  closeSession: (sessionId: string) => Promise<void>
  markAiIntervention: (sessionId: string) => void
  runAiCommand: (input: RunAiCommandInput) => Promise<AiTerminalCommandResult>
}

const LOCAL_DEFAULT_BACKEND: TerminalBackend = {
  id: 'local',
  name: 'Local',
  backend_type: 'local',
  config_json: '{}',
  status: 'connected',
  last_connected_at: new Date().toISOString(),
  created_at: new Date().toISOString(),
  updated_at: new Date().toISOString(),
}

const MAX_OUTPUT_CHARS = 250_000
const AI_DONE_PREFIX = '__OPEN_COWORK_AI_DONE__'
const AI_CWD_PREFIX = '__OPEN_COWORK_CURRENT_CWD__'

let listenersReady = false
const pendingAiCommands = new Map<string, PendingAiCommand>()

function createSessionId(kind: TerminalSessionKind) {
  return `term-${kind}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function appendOutput(previous: string, next: string): string {
  const combined = `${previous}${next}`
  if (combined.length <= MAX_OUTPUT_CHARS) return combined
  return combined.slice(combined.length - MAX_OUTPUT_CHARS)
}

function getAllSessions(state: TerminalState): TerminalSession[] {
  return Object.values(state.sessionsByThread).flat()
}

function findSession(state: TerminalState, sessionId: string): TerminalSession | undefined {
  return getAllSessions(state).find((session) => session.id === sessionId)
}

function updateSessionById(
  state: TerminalState,
  sessionId: string,
  updater: (session: TerminalSession) => TerminalSession,
): Pick<TerminalState, 'sessionsByThread'> {
  const sessionsByThread: Record<string, TerminalSession[]> = {}
  for (const [threadId, sessions] of Object.entries(state.sessionsByThread)) {
    sessionsByThread[threadId] = sessions.map((session) =>
      session.id === sessionId ? updater(session) : session,
    )
  }
  return { sessionsByThread }
}

function removeSessionById(
  state: TerminalState,
  sessionId: string,
): Pick<TerminalState, 'sessionsByThread' | 'activeSessionIds'> {
  const sessionsByThread: Record<string, TerminalSession[]> = {}
  const activeSessionIds = { ...state.activeSessionIds }
  for (const [threadId, sessions] of Object.entries(state.sessionsByThread)) {
    const nextSessions = sessions.filter((session) => session.id !== sessionId)
    sessionsByThread[threadId] = nextSessions
    if (activeSessionIds[threadId] === sessionId) {
      activeSessionIds[threadId] = nextSessions[0]?.id ?? null
    }
  }
  return { sessionsByThread, activeSessionIds }
}

function isIgnoredEchoLine(line: string, pending: PendingAiCommand): boolean {
  const trimmed = line.trim()
  if (!trimmed) return false
  return pending.ignoredEchoLines.some((echoLine) => (
    trimmed === echoLine
    || trimmed.endsWith(`> ${echoLine}`)
    || trimmed.endsWith(`] ${echoLine}`)
  ))
}

function splitInternalLines(data: string, pending: PendingAiCommand | undefined) {
  const display: string[] = []
  const capture: string[] = []
  if (!pending) return { displayData: data, captureData: data, done: false }

  let done = false
  let skipNextLineBreak = false
  const lines = data.split(/(\r?\n)/)
  for (let index = 0; index < lines.length; index += 1) {
    const part = lines[index]
    if (part === '\n' || part === '\r\n') {
      if (skipNextLineBreak) {
        skipNextLineBreak = false
        continue
      }
      display.push(part)
      if (!done) capture.push(part)
      continue
    }

    const trimmed = part.trim()
    if (trimmed.startsWith(`${pending.marker}=`)) {
      const parsed = Number(trimmed.slice(`${pending.marker}=`.length).trim())
      pending.exitCode = Number.isFinite(parsed) ? parsed : null
      done = true
      skipNextLineBreak = true
      continue
    }
    if (trimmed.startsWith(`${pending.cwdMarker}=`)) {
      pending.currentCwd = trimmed.slice(`${pending.cwdMarker}=`.length).trim()
      skipNextLineBreak = true
      continue
    }
    if (!done && isIgnoredEchoLine(part, pending)) {
      skipNextLineBreak = true
      continue
    }
    display.push(part)
    if (!done) capture.push(part)
  }

  return { displayData: display.join(''), captureData: capture.join(''), done }
}

function completePendingAiCommand(pending: PendingAiCommand, timedOut: boolean) {
  if (pending.timer !== null) {
    clearTimeout(pending.timer)
  }
  pendingAiCommands.delete(pending.marker)
  pending.resolve({
    sessionId: pending.sessionId,
    stdout: pending.stdout.join(''),
    stderr: pending.stderr.join(''),
    exitCode: pending.exitCode,
    currentCwd: pending.currentCwd,
    interruptedByUser: pending.intervention,
    timedOut,
  })
  useTerminalStore.setState((state) =>
    updateSessionById(state, pending.sessionId, (session) => ({
      ...session,
      status: 'idle',
      updatedAt: Date.now(),
      currentAiCommand: undefined,
    })),
  )
}

function handleTerminalOutput(payload: TerminalOutputEvent) {
  const pending = [...pendingAiCommands.values()].find((entry) => entry.sessionId === payload.sessionId)
  const { displayData, captureData, done } = splitInternalLines(payload.data, pending)

  if (pending && captureData && payload.stream !== 'system') {
    if (payload.stream === 'stderr') {
      pending.stderr.push(captureData)
    } else {
      pending.stdout.push(captureData)
    }
  }

  useTerminalStore.setState((state) => {
    const session = findSession(state, payload.sessionId)
    if (!session) return state
    const threadId = session.threadId
    const hiddenActivityByThread = !state.dockOpenByThread[threadId]
      ? { ...state.hiddenActivityByThread, [threadId]: true }
      : state.hiddenActivityByThread
    return {
      ...updateSessionById(state, payload.sessionId, (item) => ({
        ...item,
        output: appendOutput(item.output, displayData),
        updatedAt: Date.now(),
      })),
      hiddenActivityByThread,
    }
  })

  if (pending && done) {
    completePendingAiCommand(pending, false)
  }
}

function handleTerminalExit(payload: TerminalExitEvent) {
  useTerminalStore.setState((state) =>
    updateSessionById(state, payload.sessionId, (session) => ({
      ...session,
      status: 'exited',
      output: appendOutput(session.output, `\n[terminal ${payload.reason}${payload.exitCode !== null ? `: ${payload.exitCode}` : ''}]\n`),
      updatedAt: Date.now(),
      currentAiCommand: undefined,
    })),
  )
}

function looksRiskyCommand(command: string): boolean {
  const normalized = command.toLowerCase()
  return [
    'rm -rf',
    'remove-item',
    '-recurse',
    'del /f',
    'del /s',
    'format ',
    'reg delete',
    'taskkill /f',
  ].some((pattern) => normalized.includes(pattern))
}

function buildAiCommand(command: string, marker: string, cwdMarker: string): { data: string; echoLines: string[] } {
  const normalizedCommand = command.replace(/\r?\n/g, '\r\n')
  const lines = [
    normalizedCommand,
    '$openCoworkExit = if ($null -ne $LASTEXITCODE) { $LASTEXITCODE } elseif ($?) { 0 } else { 1 }',
    `Write-Output ('${marker}=' + $openCoworkExit)`,
    `Write-Output ('${cwdMarker}=' + (Get-Location).Path)`,
  ]
  return {
    data: [...lines, ''].join('\r\n'),
    echoLines: lines.flatMap((line) => line.split(/\r\n/)).map((line) => line.trim()).filter(Boolean),
  }
}

export function isRiskyTerminalCommand(command: string): boolean {
  return looksRiskyCommand(command)
}

export const useTerminalStore = create<TerminalState>()((set, get) => ({
  backends: [],
  loading: false,
  error: null,

  sessionsByThread: {},
  activeSessionIds: {},
  dockOpenByThread: {},
  dockHeightByThread: {},
  hiddenActivityByThread: {},
  activeAiThreadId: null,

  loadBackends: async () => {
    set({ loading: true, error: null })
    try {
      const backends = await safeInvoke<TerminalBackend[]>('backend_list', undefined, [])
      set({ backends, loading: false })
    } catch (e) {
      set({ error: String(e), loading: false })
    }
  },

  upsertBackend: async (b) => {
    try {
      await safeInvoke('backend_upsert', {
        id: b.id,
        name: b.name,
        backendType: b.backendType,
        configJson: b.configJson,
      }, undefined)
      return true
    } catch (e) {
      set({ error: String(e) })
      return false
    }
  },

  deleteBackend: async (id) => {
    try {
      await safeInvoke('backend_delete', { id }, undefined)
      set((s) => ({ backends: s.backends.filter((b) => b.id !== id) }))
    } catch (e) {
      set({ error: String(e) })
    }
  },

  execCommand: async (backendId, command, workingDir, timeoutMs) => {
    try {
      return await safeInvoke<BackendExecResponse>('backend_exec', {
        backendId,
        command,
        workingDir: workingDir ?? null,
        timeoutMs: timeoutMs ?? null,
      }, {
        backendId,
        stdout: 'Terminal execution is only available in the desktop app.',
        stderr: '',
        exitCode: 1,
        timedOut: false,
      })
    } catch (e) {
      return {
        backendId,
        stdout: '',
        stderr: String(e),
        exitCode: 1,
        timedOut: false,
      }
    }
  },

  ensureLocalBackend: async () => {
    try {
      const backend = await safeInvoke<TerminalBackend>(
        'backend_ensure_local', undefined, LOCAL_DEFAULT_BACKEND,
      )
      set((s) => {
        const exists = s.backends.some((b) => b.id === backend.id)
        return { backends: exists ? s.backends : [backend, ...s.backends] }
      })
      return backend
    } catch {
      set((s) => {
        const exists = s.backends.some((b) => b.id === LOCAL_DEFAULT_BACKEND.id)
        return { backends: exists ? s.backends : [LOCAL_DEFAULT_BACKEND, ...s.backends] }
      })
      return LOCAL_DEFAULT_BACKEND
    }
  },

  ensureListeners: () => {
    if (listenersReady || !hasTauriRuntime()) return
    listenersReady = true
    void listen<TerminalOutputEvent>('terminal-output', (event) => handleTerminalOutput(event.payload))
    void listen<TerminalExitEvent>('terminal-exit', (event) => handleTerminalExit(event.payload))
  },

  setActiveAiThread: (threadId) => set({ activeAiThreadId: threadId }),

  getThreadSessions: (threadId) => get().sessionsByThread[threadId] ?? [],

  getActiveSession: (threadId) => {
    const state = get()
    const sessions = state.sessionsByThread[threadId] ?? []
    const activeId = state.activeSessionIds[threadId]
    return sessions.find((session) => session.id === activeId) ?? sessions[0] ?? null
  },

  setActiveSession: (threadId, sessionId) =>
    set((state) => ({
      activeSessionIds: { ...state.activeSessionIds, [threadId]: sessionId },
      sessionsByThread: {
        ...state.sessionsByThread,
        [threadId]: (state.sessionsByThread[threadId] ?? []).map((session) => ({
          ...session,
          hidden: session.id === sessionId ? false : session.hidden,
        })),
      },
      hiddenActivityByThread: { ...state.hiddenActivityByThread, [threadId]: false },
    })),

  setDockOpen: (threadId, open) =>
    set((state) => ({
      dockOpenByThread: { ...state.dockOpenByThread, [threadId]: open },
      hiddenActivityByThread: open
        ? { ...state.hiddenActivityByThread, [threadId]: false }
        : state.hiddenActivityByThread,
    })),

  setDockHeight: (threadId, height) =>
    set((state) => ({
      dockHeightByThread: {
        ...state.dockHeightByThread,
        [threadId]: Math.max(180, Math.min(560, Math.round(height))),
      },
    })),

  clearHiddenActivity: (threadId) =>
    set((state) => ({
      hiddenActivityByThread: { ...state.hiddenActivityByThread, [threadId]: false },
    })),

  createSession: async ({ threadId, cwd, title, kind = 'manual', hidden = false }) => {
    get().ensureListeners()
    const id = createSessionId(kind)
    const session: TerminalSession = {
      id,
      threadId,
      title: title ?? (kind === 'ai' ? 'AI Shell' : 'PowerShell'),
      shell: 'powershell',
      cwd,
      output: '',
      status: 'idle',
      kind,
      hidden,
      createdAt: Date.now(),
      updatedAt: Date.now(),
    }

    set((state) => ({
      sessionsByThread: {
        ...state.sessionsByThread,
        [threadId]: [...(state.sessionsByThread[threadId] ?? []), session],
      },
      activeSessionIds: {
        ...state.activeSessionIds,
        [threadId]: hidden ? state.activeSessionIds[threadId] ?? id : id,
      },
      hiddenActivityByThread: hidden
        ? { ...state.hiddenActivityByThread, [threadId]: true }
        : state.hiddenActivityByThread,
    }))

    if (hasTauriRuntime()) {
      await invoke('terminal_create', {
        request: {
          sessionId: id,
          shell: 'powershell',
          cwd,
          cols: 100,
          rows: 24,
        },
      })
    } else {
      handleTerminalOutput({
        sessionId: id,
        stream: 'system',
        data: 'Terminal sessions are only available in the desktop app.\n',
      })
    }

    return session
  },

  writeToSession: async (sessionId, data) => {
    if (!hasTauriRuntime()) {
      handleTerminalOutput({ sessionId, stream: 'stdout', data })
      return
    }
    await invoke('terminal_write', { request: { sessionId, data } })
  },

  resizeSession: async (sessionId, cols, rows) => {
    if (!hasTauriRuntime()) return
    await invoke('terminal_resize', { request: { sessionId, cols, rows } })
  },

  interruptSession: async (sessionId) => {
    if (!hasTauriRuntime()) return
    await invoke('terminal_interrupt', { request: { sessionId } })
  },

  killSession: async (sessionId) => {
    if (hasTauriRuntime()) {
      await invoke('terminal_kill', { request: { sessionId } })
    }
    set((state) =>
      updateSessionById(state, sessionId, (session) => ({
        ...session,
        status: 'exited',
        currentAiCommand: undefined,
        updatedAt: Date.now(),
      })),
    )
  },

  closeSession: async (sessionId) => {
    if (hasTauriRuntime()) {
      await invoke('terminal_close', { request: { sessionId } })
    }
    set((state) => removeSessionById(state, sessionId))
  },

  markAiIntervention: (sessionId) => {
    for (const pending of pendingAiCommands.values()) {
      if (pending.sessionId === sessionId) {
        pending.intervention = true
      }
    }
    set((state) =>
      updateSessionById(state, sessionId, (session) => ({
        ...session,
        currentAiCommand: session.currentAiCommand
          ? { ...session.currentAiCommand, intervention: true }
          : session.currentAiCommand,
      })),
    )
  },

  runAiCommand: async ({ threadId, command, cwd, timeoutMs = 30_000 }) => {
    const state = get()
    if (!hasTauriRuntime()) {
      const dockOpen = Boolean(state.dockOpenByThread[threadId])
      const visibleActive = state.getActiveSession(threadId)
      const reusableHiddenAi = !dockOpen
        ? (state.sessionsByThread[threadId] ?? []).find((item) => item.kind === 'ai' && item.hidden && item.status === 'idle') ?? null
        : null
      const selected = dockOpen && visibleActive && visibleActive.status === 'idle'
        ? visibleActive
        : reusableHiddenAi
      const session = selected ?? await state.createSession({
        threadId,
        cwd,
        title: 'AI Shell',
        kind: 'ai',
        hidden: !dockOpen,
      })
      handleTerminalOutput({
        sessionId: session.id,
        stream: 'system',
        data: `AI command requested outside the desktop runtime:\n${command}\n`,
      })
      return {
        sessionId: session.id,
        stdout: '',
        stderr: 'Terminal command execution is only available in the desktop app.',
        exitCode: 1,
        interruptedByUser: false,
        timedOut: false,
      }
    }
    const dockOpen = Boolean(state.dockOpenByThread[threadId])
    const visibleActive = state.getActiveSession(threadId)
    const reusableHiddenAi = !dockOpen
      ? (state.sessionsByThread[threadId] ?? []).find((item) => item.kind === 'ai' && item.hidden && item.status === 'idle') ?? null
      : null
    const selected = dockOpen && visibleActive && visibleActive.status === 'idle'
      ? visibleActive
      : reusableHiddenAi
    const session = selected ?? await state.createSession({
      threadId,
      cwd,
      title: 'AI Shell',
      kind: 'ai',
      hidden: !dockOpen,
    })

    const markerId = `${Date.now()}_${Math.random().toString(36).slice(2, 8)}`
    const marker = `${AI_DONE_PREFIX}_${markerId}`
    const cwdMarker = `${AI_CWD_PREFIX}_${markerId}`
    const aiCommand = buildAiCommand(command, marker, cwdMarker)
    const pending = await new Promise<AiTerminalCommandResult>((resolve, reject) => {
      const pendingCommand: PendingAiCommand = {
        sessionId: session.id,
        threadId,
        marker,
        cwdMarker,
        ignoredEchoLines: aiCommand.echoLines,
        stdout: [],
        stderr: [],
        exitCode: null,
        intervention: false,
        resolve,
        reject,
        timer: null,
      }
      pendingCommand.timer = setTimeout(() => {
        pendingAiCommands.delete(marker)
        void get().killSession(session.id).catch((error) => {
          set({ error: error instanceof Error ? error.message : String(error) })
        })
        set((current) =>
          updateSessionById(current, session.id, (item) => ({
            ...item,
            status: 'exited',
            output: appendOutput(item.output, `\n[terminal command timed out after ${timeoutMs}ms; session terminated]\n`),
            currentAiCommand: undefined,
            updatedAt: Date.now(),
          })),
        )
        reject(new Error(`Terminal command timed out after ${timeoutMs}ms`))
      }, Math.max(1000, timeoutMs))
      pendingAiCommands.set(marker, pendingCommand)

      set((current) => ({
        ...updateSessionById(current, session.id, (item) => ({
          ...item,
          status: 'running',
          kind: 'ai',
          hidden: !dockOpen && item.hidden,
          updatedAt: Date.now(),
          currentAiCommand: {
            marker,
            command,
            startedAt: Date.now(),
            intervention: false,
          },
        })),
        activeSessionIds: {
          ...current.activeSessionIds,
          [threadId]: dockOpen ? session.id : current.activeSessionIds[threadId] ?? session.id,
        },
        hiddenActivityByThread: !dockOpen
          ? { ...current.hiddenActivityByThread, [threadId]: true }
          : current.hiddenActivityByThread,
      }))

      void get().writeToSession(session.id, aiCommand.data).catch((error) => {
        if (pendingCommand.timer !== null) {
          clearTimeout(pendingCommand.timer)
        }
        pendingAiCommands.delete(marker)
        void get().killSession(session.id).catch(() => undefined)
        set((current) =>
          updateSessionById(current, session.id, (item) => ({
            ...item,
            status: 'error',
            currentAiCommand: undefined,
            updatedAt: Date.now(),
          })),
        )
        reject(error instanceof Error ? error : new Error(String(error)))
      })
    })

    return pending
  },
}))
