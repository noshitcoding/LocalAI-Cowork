// ── Tool Registry & Built-in Tools (ported from Claude Code) ────────────────
// Mirrors: claude-code-main/src/tools.ts + tools/*
// All file/shell operations delegate to Tauri IPC commands

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useConfigStore } from '../../stores/configStore'
import type { McpServerConfig } from '../../stores/configStore'
import type { AttachmentMessage, Tool, Tools, ToolInputSchema } from '../types'

// ── Tool Registration ──────────────────────────────────────────────────────

const toolRegistry: Tool[] = []

type ExecCommandChunkPayload = {
  streamId: string
  channel: 'stdout' | 'stderr' | 'done'
  content: string
}

type ExecCommandResult = {
  stdout: string
  stderr: string
  exitCode: number
  currentCwd?: string
}

type DesktopDisplayInfo = {
  primary: boolean
  x: number
  y: number
  width: number
  height: number
  deviceName: string
  scaleFactor?: number
}

type DesktopScreenshotResponse = DesktopDisplayInfo & {
  dataUrl: string
  imageWidth?: number
  imageHeight?: number
  coordinateOverlay?: boolean
}

type DesktopWindowInfo = {
  title: string
  processId: number
  processName: string
  handle: string
  x: number
  y: number
  width: number
  height: number
  isForeground: boolean
}

type DesktopActionResponse = {
  ok: boolean
  action: string
}

type DesktopCoordinateSpace = 'display' | 'screen'

type DesktopLaunchResponse = {
  pid: number
  path: string
  args: string[]
}

function normalizeShellPath(pathValue: string): string {
  const hasDrivePrefix = /^[a-zA-Z]:[\\/]/.test(pathValue)
  const hasLeadingSlash = !hasDrivePrefix && pathValue.startsWith('/')
  const normalizedSeparators = pathValue.replace(/\\/g, '/')
  const prefix = hasDrivePrefix
    ? normalizedSeparators.slice(0, 2)
    : hasLeadingSlash
      ? '/'
      : ''
  const remainder = normalizedSeparators.slice(prefix.length)
  const parts: string[] = []

  for (const rawPart of remainder.split('/')) {
    const part = rawPart.trim()
    if (!part || part === '.') continue
    if (part === '..') {
      if (parts.length > 0 && parts[parts.length - 1] !== '..') {
        parts.pop()
      } else if (!prefix) {
        parts.push('..')
      }
      continue
    }
    parts.push(part)
  }

  if (hasDrivePrefix) {
    const suffix = parts.join('\\')
    return suffix ? `${prefix}\\${suffix}` : `${prefix}\\`
  }

  if (hasLeadingSlash) {
    return `/${parts.join('/')}`.replace(/\/$/, '') || '/'
  }

  return parts.join('/')
}

function resolveShellNavigationTarget(target: string, cwd: string): string {
  const trimmedTarget = target.trim()
  if (!trimmedTarget) return cwd
  if (/^[a-zA-Z]:[\\/]/.test(trimmedTarget) || trimmedTarget.startsWith('/')) {
    return normalizeShellPath(trimmedTarget)
  }
  return normalizeShellPath(`${cwd.replace(/[\\/]+$/, '')}/${trimmedTarget}`)
}

function inferShellCwdFromCommand(command: string, cwd: string): string | undefined {
  const navigationPattern = /(?:^|[;\n]\s*)(?:cd|chdir|Set-Location)\s+(?:"([^"]+)"|'([^']+)'|([^;\n]+))/gi
  let inferredCwd: string | undefined
  let match: RegExpExecArray | null

  while ((match = navigationPattern.exec(command)) !== null) {
    const rawTarget = match[1] ?? match[2] ?? match[3] ?? ''
    const target = rawTarget.trim()
    if (!target || target === '-' || target.startsWith('$')) continue
    inferredCwd = resolveShellNavigationTarget(target, inferredCwd ?? cwd)
  }

  return inferredCwd
}

type FsAttachmentMetadataResponse = {
  rootPath: string
  rootKind: string
  totalFiles: number
  returnedFiles: number
  truncated: boolean
  files: Array<{
    path: string
    fileName: string
    extension?: string | null
    language?: string | null
    sizeBytes: number
  }>
}

type FsCreateDirectoryResponse = {
  path: string
  created: boolean
}

type FsPathMutationResponse = {
  sourcePath: string
  destinationPath: string
  itemKind: string
  createdParent: boolean
  replacedExisting: boolean
}

type McpCallResponse = {
  serverName: string
  toolName: string
  success: boolean
  result: string
  error: string | null
}

type McpScreenshotPayload = {
  success?: boolean
  reused?: boolean
  path?: string
  displayIndex?: number
  displayInfo?: {
    width?: number
    height?: number
    x?: number
    y?: number
    primary?: boolean
    deviceName?: string
    scaleFactor?: number
    imageWidth?: number
    imageHeight?: number
    coordinateOverlay?: boolean
    coordinateGrid?: {
      minorStepPx?: number
      majorStepPx?: number
      origin?: string
      coordinateSpace?: string
    }
  }
  timestamp?: string
  reason?: string
  region?: {
    x: number
    y: number
    width: number
    height: number
  }
  duplicateCallCount?: number
  nextStepHint?: string
  forceRefresh?: boolean
  mimeType?: string
  coordinateOverlay?: boolean
  coordinateGrid?: {
    minorStepPx?: number
    majorStepPx?: number
    origin?: string
    coordinateSpace?: string
  }
  imageDataUrl?: string
}

type OfficeWorkflowArtifact = {
  path: string
  generator: string
  format: string
}

type OfficeWorkflowResponse = {
  format: string
  mode: string
  placeholdersApplied: number
  generated: OfficeWorkflowArtifact[]
  warnings: string[]
}

type LegacyMcpCallResponse = {
  content: string
  isError?: boolean
}

type MemoryEntry = {
  id: string
  scope: string
  category: string
  key: string
  content: string
  sourceSessionId?: string | null
  confidence: number
  accessCount: number
  lastAccessedAt?: string | null
  createdAt: string
  updatedAt: string
}

function createToolStreamId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID()
  }
  return `${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
}

function splitCommandArgs(value: string): string[] {
  const matches = value.match(/"[^"]*"|'[^']*'|\S+/g) ?? []
  return matches.map((part) => part.replace(/^["']|["']$/g, ''))
}

function parseBase64DataUrl(dataUrl: string): { mediaType: string; data: string } | null {
  const match = dataUrl.trim().match(/^data:([^;]+);base64,(.+)$/s)
  if (!match) return null
  return {
    mediaType: match[1],
    data: match[2],
  }
}

function formatDesktopScreenshotSummary(screenshot: DesktopScreenshotResponse): string {
  const imageWidth = screenshot.imageWidth ?? screenshot.width
  const imageHeight = screenshot.imageHeight ?? screenshot.height
  const scaleFactor = typeof screenshot.scaleFactor === 'number' ? ` DPI-Skalierung ca. ${screenshot.scaleFactor.toFixed(2)}x.` : ''
  const overlay = screenshot.coordinateOverlay
    ? ' Das Bild enthaelt ein Koordinatenraster: feine Linien alle 50 px, beschriftete Hauptlinien alle 100 px.'
    : ''
  return `Desktop screenshot aufgenommen: Bild ${imageWidth}x${imageHeight}, Display ${screenshot.width}x${screenshot.height} auf ${screenshot.deviceName}${screenshot.primary ? ' [primary]' : ''}.${scaleFactor} Verwende fuer Klicks lokale Display-Koordinaten aus exakt diesem Bild mit Ursprung (0, 0) links oben; der virtuelle Bildschirm-Ursprung dieses Displays liegt bei (${screenshot.x}, ${screenshot.y}).${overlay}`
}

function normalizeDesktopCoordinateSpace(value: string | undefined): DesktopCoordinateSpace {
  return value === 'screen' ? 'screen' : 'display'
}

type ResolvedDesktopPoint = {
  requestedX: number
  requestedY: number
  absoluteX: number
  absoluteY: number
  coordinateSpace: DesktopCoordinateSpace
  display?: DesktopDisplayInfo
}

async function resolveDesktopPoint(
  x: number,
  y: number,
  coordinateSpace?: string,
): Promise<ResolvedDesktopPoint> {
  const requestedX = Math.round(x)
  const requestedY = Math.round(y)
  const normalizedCoordinateSpace = normalizeDesktopCoordinateSpace(coordinateSpace)

  if (normalizedCoordinateSpace === 'screen') {
    return {
      requestedX,
      requestedY,
      absoluteX: requestedX,
      absoluteY: requestedY,
      coordinateSpace: normalizedCoordinateSpace,
    }
  }

  const display = await invoke<DesktopDisplayInfo>('desktop_primary_display')

  return {
    requestedX,
    requestedY,
    absoluteX: requestedX + display.x,
    absoluteY: requestedY + display.y,
    coordinateSpace: normalizedCoordinateSpace,
    display,
  }
}

function describeResolvedDesktopPoint(point: ResolvedDesktopPoint): string {
  if (point.coordinateSpace === 'screen' || !point.display) {
    return `Bildschirmkoordinaten (${point.absoluteX}, ${point.absoluteY})`
  }

  return `Display-Koordinaten (${point.requestedX}, ${point.requestedY}) wurden mit Display-Ursprung (${point.display.x}, ${point.display.y}) zu Bildschirmkoordinaten (${point.absoluteX}, ${point.absoluteY}) umgerechnet`
}

function createDesktopScreenshotAttachment(
  screenshot: DesktopScreenshotResponse,
  options?: { title?: string; preface?: string },
): AttachmentMessage | null {
  const parsed = parseBase64DataUrl(screenshot.dataUrl)
  if (!parsed) return null

  const summary = formatDesktopScreenshotSummary(screenshot)
  const text = options?.preface ? `${options.preface}\n${summary}` : summary

  return {
    type: 'attachment',
    uuid: createToolStreamId(),
    title: options?.title ?? `Desktop Screenshot ${screenshot.width}x${screenshot.height}`,
    attachmentType: 'tool_result',
    timestamp: Date.now(),
    content: [
      {
        type: 'text',
        text,
      },
      {
        type: 'image',
        source: {
          type: 'base64',
          media_type: parsed.mediaType,
          data: parsed.data,
        },
      },
    ],
  }
}

function parseMcpScreenshotPayload(message: string): McpScreenshotPayload | null {
  try {
    const parsed = JSON.parse(message)
    if (!parsed || typeof parsed !== 'object') return null
    return parsed as McpScreenshotPayload
  } catch {
    return null
  }
}

function createMcpScreenshotAttachment(
  toolName: string,
  message: string,
): { attachment: AttachmentMessage; textSummary: string } | null {
  if (toolName !== 'screenshot_for_display') {
    return null
  }

  const payload = parseMcpScreenshotPayload(message)
  if (!payload?.imageDataUrl) {
    return null
  }

  const parsedDataUrl = parseBase64DataUrl(payload.imageDataUrl)
  if (!parsedDataUrl) {
    return null
  }

  const safePayload: Record<string, unknown> = {
    ...payload,
    imageDataUrl: undefined,
  }
  delete safePayload.imageDataUrl

  const width = payload.displayInfo?.width
  const height = payload.displayInfo?.height
  const reused = payload.reused === true
  const title = width && height
    ? `MCP Screenshot ${width}x${height}${reused ? ' (reused)' : ''}`
    : `MCP Screenshot${reused ? ' (reused)' : ''}`

  const overlayEnabled = payload.coordinateOverlay === true || payload.displayInfo?.coordinateOverlay === true
  const grid = payload.coordinateGrid ?? payload.displayInfo?.coordinateGrid
  const coordinateGuide = overlayEnabled
    ? `Koordinatenhinweis: Das angehaengte Screenshot-Bild enthaelt ein Raster in lokalen Display-Koordinaten mit Ursprung (0, 0) links oben. Feine Linien: ${grid?.minorStepPx ?? 50}px, beschriftete Hauptlinien: ${grid?.majorStepPx ?? 100}px. Nutze diese Bildkoordinaten direkt fuer DesktopClick/DesktopMoveMouse mit coordinate_space="display".`
    : 'Koordinatenhinweis: Nutze lokale Display-Koordinaten aus dem angehaengten Screenshot-Bild direkt fuer DesktopClick/DesktopMoveMouse mit coordinate_space="display".'
  const textSummary = `${coordinateGuide}\n${JSON.stringify(safePayload, null, 2)}`

  return {
    attachment: {
      type: 'attachment',
      uuid: createToolStreamId(),
      title,
      attachmentType: 'tool_result',
      timestamp: Date.now(),
      content: [
        {
          type: 'text',
          text: textSummary,
        },
        {
          type: 'image',
          source: {
            type: 'base64',
            media_type: parsedDataUrl.mediaType,
            data: parsedDataUrl.data,
          },
        },
      ],
    },
    textSummary,
  }
}

async function captureDesktopVerificationAttachment(actionLabel: string): Promise<AttachmentMessage | null> {
  try {
    const screenshot = await invoke<DesktopScreenshotResponse>('desktop_capture_primary_annotated_screenshot')
    return createDesktopScreenshotAttachment(screenshot, {
      title: `Desktop Verification: ${actionLabel}`,
      preface: `Verifikation nach Aktion: ${actionLabel}`,
    })
  } catch {
    return null
  }
}

function findMcpServerConfig(serverName: string): McpServerConfig | null {
  const { mcpServer, mcpServers } = useConfigStore.getState()
  const servers = mcpServers.length > 0 ? mcpServers : [mcpServer]
  return servers.find((server) => server.name === serverName) ?? null
}

function normalizeMemoryScope(scope?: string): string | undefined {
  if (!scope) return undefined

  switch (scope) {
    case 'project':
      return 'agent'
    case 'global':
      return 'shared'
    case 'agent':
    case 'user':
    case 'session':
    case 'shared':
      return scope
    default:
      return scope
  }
}

export function registerTool(tool: Tool): void {
  toolRegistry.push(tool)
}

export function getAllTools(): Tools {
  return toolRegistry
}

export function getToolsByCategory(category: string): Tools {
  return toolRegistry.filter(t => t.category === category)
}

export function getEnabledTools(): Tools {
  return toolRegistry.filter(t => !t.isEnabled || t.isEnabled())
}

// ── FileReadTool ───────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/FileReadTool/

const fileReadTool: Tool<{ file_path: string; offset?: number; limit?: number }> = {
  name: 'Read',
  aliases: ['read_file', 'FileReadTool'],
  description: 'Liest den Inhalt einer Datei. Nutze offset/limit fuer grosse Dateien.',
  category: 'filesystem',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      file_path: { type: 'string', description: 'Absoluter oder relativer Pfad zur Datei' },
      offset: { type: 'number', description: 'Startzeile (0-basiert)' },
      limit: { type: 'number', description: 'Maximale Anzahl Zeilen' },
    },
    required: ['file_path'],
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(input, context) {
    const fullPath = resolvePath(input.file_path, context.cwd)
    const result = await invoke<string>('fs_extract_text', { path: fullPath, runId: context.runId })
    let content = result
    if (input.offset !== undefined || input.limit !== undefined) {
      const lines = content.split('\n')
      const start = input.offset ?? 0
      const end = input.limit ? start + input.limit : lines.length
      content = lines.slice(start, end).join('\n')
    }
    return { data: content }
  },
}

// ── FileWriteTool ──────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/FileWriteTool/

const fileWriteTool: Tool<{ file_path: string; content: string; create_backup?: boolean }> = {
  name: 'Write',
  aliases: ['write_file', 'FileWriteTool'],
  description: 'Schreibt Inhalt in eine Datei. Erstellt bei Bedarf uebergeordnete Verzeichnisse.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      file_path: { type: 'string', description: 'Absoluter oder relativer Pfad zur Datei' },
      content: { type: 'string', description: 'Der zu schreibende Inhalt' },
      create_backup: { type: 'boolean', description: 'Backup der Originaldatei erstellen' },
    },
    required: ['file_path', 'content'],
  },
  isReadOnly: () => false,
  isDestructive: () => false,
  isConcurrencySafe: () => false,
  async call(input, context) {
    const fullPath = resolvePath(input.file_path, context.cwd)
    const result = await invoke<{ diff: string }>('fs_write_text_file', {
      path: fullPath,
      content: input.content,
      createBackup: input.create_backup ?? false,
      runId: context.runId,
    })
    return { data: result.diff || 'Datei geschrieben.' }
  },
}

// ── FileEditTool ───────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/FileEditTool/

const createDirectoryTool: Tool<{ path: string }> = {
  name: 'CreateDirectory',
  aliases: ['create_directory', 'mkdir', 'make_dir', 'MakeDirectoryTool'],
  description: 'Erstellt ein Verzeichnis inklusive fehlender Zwischenordner.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Pfad zum zu erstellenden Verzeichnis' },
    },
    required: ['path'],
  },
  isReadOnly: () => false,
  isDestructive: () => false,
  isConcurrencySafe: () => false,
  async call(input, context, onProgress) {
    const fullPath = resolvePath(input.path, context.cwd)
    onProgress?.({
      toolUseID: '',
      data: {
        type: 'file_progress',
        path: fullPath,
        operation: 'create_dir',
      },
    })
    const result = await invoke<FsCreateDirectoryResponse>('fs_create_directory', {
      path: fullPath,
      runId: context.runId,
    })
    return { data: result.created ? `Verzeichnis erstellt: ${result.path}` : `Verzeichnis existiert bereits: ${result.path}` }
  },
}

const movePathTool: Tool<{ source_path: string; destination_path: string; overwrite?: boolean }> = {
  name: 'MovePath',
  aliases: ['move_path', 'move_file', 'move_directory', 'rename_path', 'MovePathTool'],
  description: 'Verschiebt oder benennt eine Datei oder einen Ordner um.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      source_path: { type: 'string', description: 'Ausgangspfad der Datei oder des Ordners' },
      destination_path: { type: 'string', description: 'Zielpfad nach dem Verschieben' },
      overwrite: { type: 'boolean', description: 'Bestehendes Ziel ueberschreiben (Standard: false)' },
    },
    required: ['source_path', 'destination_path'],
  },
  isReadOnly: () => false,
  isDestructive: () => false,
  isConcurrencySafe: () => false,
  async call(input, context, onProgress) {
    const sourcePath = resolvePath(input.source_path, context.cwd)
    const destinationPath = resolvePath(input.destination_path, context.cwd)
    onProgress?.({
      toolUseID: '',
      data: {
        type: 'file_progress',
        path: `${sourcePath} -> ${destinationPath}`,
        operation: 'move',
      },
    })
    const result = await invoke<FsPathMutationResponse>('fs_move_path', {
      sourcePath,
      destinationPath,
      overwrite: input.overwrite ?? false,
      runId: context.runId,
    })
    const notes = [
      `${result.itemKind} verschoben: ${result.sourcePath} -> ${result.destinationPath}`,
      result.createdParent ? 'Zielordner wurde automatisch erstellt.' : '',
      result.replacedExisting ? 'Bestehendes Ziel wurde ersetzt.' : '',
    ].filter(Boolean)
    return { data: notes.join('\n') }
  },
}

const copyPathTool: Tool<{ source_path: string; destination_path: string; overwrite?: boolean }> = {
  name: 'CopyPath',
  aliases: ['copy_path', 'copy_file', 'copy_directory', 'CopyPathTool'],
  description: 'Kopiert eine Datei oder einen Ordner an einen neuen Pfad.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      source_path: { type: 'string', description: 'Quelle der Datei oder des Ordners' },
      destination_path: { type: 'string', description: 'Zielpfad fuer die Kopie' },
      overwrite: { type: 'boolean', description: 'Bestehendes Ziel ueberschreiben (Standard: false)' },
    },
    required: ['source_path', 'destination_path'],
  },
  isReadOnly: () => false,
  isDestructive: () => false,
  isConcurrencySafe: () => false,
  async call(input, context, onProgress) {
    const sourcePath = resolvePath(input.source_path, context.cwd)
    const destinationPath = resolvePath(input.destination_path, context.cwd)
    onProgress?.({
      toolUseID: '',
      data: {
        type: 'file_progress',
        path: `${sourcePath} -> ${destinationPath}`,
        operation: 'copy',
      },
    })
    const result = await invoke<FsPathMutationResponse>('fs_copy_path', {
      sourcePath,
      destinationPath,
      overwrite: input.overwrite ?? false,
      runId: context.runId,
    })
    const notes = [
      `${result.itemKind} kopiert: ${result.sourcePath} -> ${result.destinationPath}`,
      result.createdParent ? 'Zielordner wurde automatisch erstellt.' : '',
      result.replacedExisting ? 'Bestehendes Ziel wurde ersetzt.' : '',
    ].filter(Boolean)
    return { data: notes.join('\n') }
  },
}

const fileEditTool: Tool<{ file_path: string; old_string: string; new_string: string }> = {
  name: 'Edit',
  aliases: ['edit_file', 'FileEditTool'],
  description: 'Ersetzt eine exakte Zeichenfolge in einer Datei. old_string muss eindeutig sein.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      file_path: { type: 'string', description: 'Pfad zur zu bearbeitenden Datei' },
      old_string: { type: 'string', description: 'Exakter zu ersetzender Text' },
      new_string: { type: 'string', description: 'Neuer Ersetzungstext' },
    },
    required: ['file_path', 'old_string', 'new_string'],
  },
  isReadOnly: () => false,
  isDestructive: () => false,
  isConcurrencySafe: () => false,
  async call(input, context) {
    const fullPath = resolvePath(input.file_path, context.cwd)
    // Read file, replace, write back
    const content = await invoke<string>('fs_extract_text', { path: fullPath, runId: context.runId })
    const occurrences = content.split(input.old_string).length - 1
    if (occurrences === 0) {
      return { data: `Fehler: old_string wurde nicht in ${input.file_path} gefunden.` }
    }
    if (occurrences > 1) {
      return { data: `Fehler: old_string wurde ${occurrences}x gefunden. Muss eindeutig sein.` }
    }
    const newContent = content.replace(input.old_string, input.new_string)
    await invoke('fs_write_text_file', {
      path: fullPath,
      content: newContent,
      createBackup: true,
      runId: context.runId,
    })
    return { data: `Datei bearbeitet: ${input.file_path}` }
  },
}

// ── GlobTool ───────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/GlobTool/

const globTool: Tool<{ pattern: string; path?: string }> = {
  name: 'Glob',
  aliases: ['glob', 'GlobTool'],
  description: 'Sucht Dateien nach Glob-Muster. Schnell zum Finden von Dateien nach Name/Extension.',
  category: 'search',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      pattern: { type: 'string', description: 'Glob-Muster (z.B. **/*.ts, src/**/*.rs)' },
      path: { type: 'string', description: 'Basisverzeichnis fuer die Suche (optional)' },
    },
    required: ['pattern'],
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(input, context) {
    const basePath = input.path ? resolvePath(input.path, context.cwd) : context.cwd
    const result = await invoke<FsAttachmentMetadataResponse>('fs_collect_attachment_metadata', {
      path: basePath,
      maxEntries: 200,
      runId: context.runId,
    })
    // Filter by glob pattern (simplified)
    const pattern = globToRegex(input.pattern)
    const matches = result.files
      .map((file) => file.path)
      .filter((filePath) => pattern.test(filePath))
    return { data: matches.length > 0 ? matches.join('\n') : 'Keine Treffer.' }
  },
}

// ── GrepTool ───────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/GrepTool/

const grepTool: Tool<{ pattern: string; path?: string; include?: string }> = {
  name: 'Grep',
  aliases: ['grep', 'GrepTool', 'search'],
  description: 'Durchsucht Dateiinhalte mit regulaeren Ausdruecken. Schnell fuer Code-Suche.',
  category: 'search',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      pattern: { type: 'string', description: 'Regulaerer Ausdruck zum Suchen' },
      path: { type: 'string', description: 'Verzeichnis fuer die Suche (optional)' },
      include: { type: 'string', description: 'Dateinamenmuster zum Einschliessen (z.B. *.ts)' },
    },
    required: ['pattern'],
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(input, context) {
    // Use bash to run grep/findstr (PowerShell on Windows)
    const searchPath = input.path ? resolvePath(input.path, context.cwd) : context.cwd
    const includeFlag = input.include ? `-Include "${input.include}"` : ''
    const cmd = `Get-ChildItem -Path "${searchPath}" -Recurse -File ${includeFlag} | Select-String -Pattern "${input.pattern}" | Select-Object -First 50 | Format-Table -AutoSize Path, LineNumber, Line`
    try {
      const result = await invoke<{ stdout: string; stderr: string; exitCode: number }>('exec_command', {
        command: cmd,
        cwd: context.cwd,
        runId: context.runId,
      })
      return { data: result.stdout || 'Keine Treffer.' }
    } catch {
      return { data: `Grep-Suche nach "${input.pattern}" — Tauri exec_command nicht verfuegbar. Nutze Fallback.` }
    }
  },
}

// ── BashTool ───────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/BashTool/

const bashTool: Tool<{ command: string; timeout?: number }> = {
  name: 'Bash',
  aliases: ['bash', 'shell', 'BashTool', 'execute'],
  description: 'Fuehrt einen Shell-Befehl aus (PowerShell auf Windows). Nutze fuer Build, Test, Git etc.',
  category: 'shell',
  riskLevel: 'high',
  inputSchema: {
    type: 'object',
    properties: {
      command: { type: 'string', description: 'Der auszufuehrende Shell-Befehl' },
      timeout: { type: 'number', description: 'Timeout in Millisekunden (Standard: 30000)' },
    },
    required: ['command'],
  },
  isReadOnly: () => false,
  isDestructive: (input) => {
    const dangerous = ['rm -rf', 'del /s', 'format', 'Remove-Item -Recurse']
    return dangerous.some(d => input.command.includes(d))
  },
  isConcurrencySafe: () => false,
  async call(input, context, onProgress) {
    const streamId = createToolStreamId()
    let unlisten: (() => void) | null = null

    try {
      try {
        unlisten = await listen<ExecCommandChunkPayload>('exec-command-chunk', (event) => {
          if (event.payload.streamId !== streamId || !onProgress) return

          if (event.payload.channel === 'done') {
            onProgress({
              toolUseID: '',
              data: {
                type: 'bash_progress',
                output: `status: ${event.payload.content}`,
              },
            })
            return
          }

          onProgress({
            toolUseID: '',
            data: {
              type: 'bash_progress',
              output: `${event.payload.channel}: ${event.payload.content}`,
            },
          })
        })
      } catch {
        unlisten = null
      }

      const result = await invoke<ExecCommandResult>('exec_command', {
        command: input.command,
        cwd: context.cwd,
        timeoutMs: input.timeout ?? 30000,
        streamId,
        runId: context.runId,
      })
      const resolvedCurrentCwd = result.currentCwd ?? inferShellCwdFromCommand(input.command, context.cwd)
      if (resolvedCurrentCwd && resolvedCurrentCwd !== context.cwd) {
        context.setAppState((prev) => ({ ...prev, cwd: resolvedCurrentCwd }))
        if (onProgress) {
          onProgress({
            toolUseID: '',
            data: {
              type: 'bash_progress',
              output: `cwd: ${resolvedCurrentCwd}`,
            },
          })
        }
      }
      if (onProgress) {
        onProgress({
          toolUseID: '',
          data: {
            type: 'bash_progress',
            output: `exit code: ${result.exitCode}`,
            exitCode: result.exitCode,
          },
        })
      }
      const shouldMirrorCwdToStdout = /\b(?:pwd|Get-Location)\b/i.test(input.command)
      const effectiveStdout = result.stdout || (shouldMirrorCwdToStdout && resolvedCurrentCwd ? resolvedCurrentCwd : '')

      const output = [
        effectiveStdout ? `stdout:\n${effectiveStdout}` : '',
        result.stderr ? `stderr:\n${result.stderr}` : '',
        resolvedCurrentCwd ? `current cwd: ${resolvedCurrentCwd}` : '',
        `exit code: ${result.exitCode}`,
      ].filter(Boolean).join('\n\n')
      return { data: output }
    } catch (err) {
      return { data: `Fehler beim Ausfuehren: ${err instanceof Error ? err.message : String(err)}` }
    } finally {
      if (unlisten) {
        unlisten()
      }
    }
  },
}

// ── WebFetchTool ───────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/WebFetchTool/

const webFetchTool: Tool<{ url: string; max_chars?: number }> = {
  name: 'WebFetch',
  aliases: ['web_fetch', 'fetch', 'WebFetchTool'],
  description: 'Ruft den Textinhalt einer URL ab und extrahiert den Haupttext.',
  category: 'web',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      url: { type: 'string', description: 'Die abzurufende URL' },
      max_chars: { type: 'number', description: 'Maximale Zeichenanzahl (Standard: 50000)' },
    },
    required: ['url'],
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(input, context) {
    const result = await invoke<{ title?: string | null; content: string; url: string; truncated: boolean }>('web_fetch_url', {
      request: {
        url: input.url,
        maxChars: input.max_chars ?? 50000,
      },
      runId: context.runId,
    })
    const title = result.title?.trim() || input.url
    return { data: `# ${title}\n\n${result.content}` }
  },
}

// ── WebSearchTool ──────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/WebSearchTool/

const webSearchTool: Tool<{ query: string; max_results?: number }> = {
  name: 'WebSearch',
  aliases: ['web_search', 'WebSearchTool'],
  description: 'Durchsucht das Web nach Informationen zu einem Thema.',
  category: 'web',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      query: { type: 'string', description: 'Suchanfrage' },
      max_results: { type: 'number', description: 'Maximale Ergebnisanzahl (Standard: 5)' },
    },
    required: ['query'],
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(input, context, onProgress) {
    try {
      const result = await invoke<{ query: string; results: Array<{ title: string; url: string; snippet: string }> }>('web_search', {
        request: {
          query: input.query,
          maxResults: input.max_results ?? 5,
        },
        runId: context.runId,
      })
      if (onProgress) {
        onProgress({
          toolUseID: '',
          data: { type: 'web_search_progress', query: input.query, results: result.results.length },
        })
      }
      const lines = result.results.map((item, index) => {
        const snippet = item.snippet ? `\n${item.snippet}` : ''
        return `${index + 1}. ${item.title}\n${item.url}${snippet}`
      })
      return { data: lines.join('\n\n') || `Keine Treffer fuer "${input.query}"` }
    } catch {
      return { data: `Web-Suche fehlgeschlagen fuer: "${input.query}"` }
    }
  },
}

// ── MCPTool ────────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/MCPTool/

const mcpTool: Tool<{ server_name: string; tool_name: string; arguments: Record<string, unknown> }> = {
  name: 'MCPTool',
  aliases: ['mcp_call', 'mcp'],
  description: 'Ruft ein Tool auf einem MCP-Server auf.',
  category: 'mcp',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      server_name: { type: 'string', description: 'Name des MCP-Servers' },
      tool_name: { type: 'string', description: 'Name des Tools auf dem Server' },
      arguments: { type: 'object', description: 'Argumente fuer den Tool-Aufruf' },
    },
    required: ['server_name', 'tool_name', 'arguments'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input, context) {
    const server = findMcpServerConfig(input.server_name)
    if (!server) {
      return { data: `MCP Fehler: Server "${input.server_name}" ist nicht konfiguriert.` }
    }

    const requestPayload = {
      name: server.name,
      command: server.command,
      args: splitCommandArgs(server.args),
      env: server.env ?? {},
      toolName: input.tool_name,
      toolArgs: input.arguments,
    }

    const normalizeResponse = (raw: McpCallResponse | LegacyMcpCallResponse): { ok: boolean; message: string } => {
      if ('content' in raw) {
        return {
          ok: !raw.isError,
          message: raw.content,
        }
      }
      return {
        ok: raw.success,
        message: raw.success ? raw.result : (raw.error ?? raw.result),
      }
    }

    const toToolResult = (normalized: { ok: boolean; message: string }) => {
      if (!normalized.ok) {
        return { data: `MCP Fehler: ${normalized.message}` }
      }

      const screenshotAttachment = createMcpScreenshotAttachment(input.tool_name, normalized.message)
      if (screenshotAttachment) {
        return {
          data: screenshotAttachment.textSummary,
          newMessages: [screenshotAttachment.attachment],
        }
      }

      return { data: normalized.message }
    }

    try {
      const primary = await invoke<McpCallResponse | LegacyMcpCallResponse>('mcp_call_tool', {
        request: requestPayload,
        runId: context.runId,
      })
      const normalized = normalizeResponse(primary)
      return toToolResult(normalized)
    } catch {
      // Backward-compat fallback for alternate envelope contracts.
      const fallback = await invoke<McpCallResponse | LegacyMcpCallResponse>('mcp_call_tool', {
        mcpCallRequest: requestPayload,
        server_name: input.server_name,
        tool_name: input.tool_name,
        arguments: input.arguments,
        runId: context.runId,
      })
      const normalized = normalizeResponse(fallback)
      return toToolResult(normalized)
    }
  },
}

const desktopScreenshotTool: Tool<Record<string, never>> = {
  name: 'DesktopScreenshot',
  aliases: ['desktop_screenshot', 'capture_desktop_screenshot'],
  description: 'Nimmt einen Screenshot des Primaerdisplays mit Koordinatenraster auf und haengt das Bild fuer visuelle Analyse an die Unterhaltung an. Nutze dieses Tool vor Maus- oder Tastaturaktionen.',
  category: 'desktop',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {},
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => false,
  async call() {
    const screenshot = await invoke<DesktopScreenshotResponse>('desktop_capture_primary_annotated_screenshot')
    const attachment = createDesktopScreenshotAttachment(screenshot)
    return {
      data: `${formatDesktopScreenshotSummary(screenshot)} ${attachment ? 'Das Bild wurde als Attachment fuer visuelle Analyse angehaengt.' : 'Das Bild konnte nicht als Attachment angehaengt werden.'}`,
      newMessages: attachment ? [attachment] : undefined,
    }
  },
}

const desktopPrimaryDisplayTool: Tool<Record<string, never>> = {
  name: 'DesktopPrimaryDisplay',
  aliases: ['desktop_primary_display', 'get_desktop_primary_display'],
  description: 'Liest Geometrie und Ursprung des Primaerdisplays aus. Hilfreich fuer Koordinaten bei Mausaktionen.',
  category: 'desktop',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {},
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call() {
    const display = await invoke<DesktopDisplayInfo>('desktop_primary_display')
    return { data: JSON.stringify(display, null, 2) }
  },
}

const desktopListWindowsTool: Tool<Record<string, never>> = {
  name: 'DesktopListWindows',
  aliases: ['desktop_list_windows', 'list_desktop_windows'],
  description: 'Listet sichtbare Desktop-Fenster mit Titel, Prozess und Bounds auf. Nutze dies, um Ziel-Fenster fuer Fokus oder Interaktionen zu finden.',
  category: 'desktop',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {},
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call() {
    const windows = await invoke<DesktopWindowInfo[]>('desktop_list_windows')
    if (windows.length === 0) {
      return { data: 'Keine sichtbaren Desktop-Fenster gefunden.' }
    }
    return { data: JSON.stringify(windows, null, 2) }
  },
}

const desktopFocusWindowTool: Tool<{ title?: string; process_name?: string; process_id?: number; exact_match?: boolean }> = {
  name: 'DesktopFocusWindow',
  aliases: ['desktop_focus_window', 'focus_desktop_window'],
  description: 'Bringt ein Desktop-Fenster in den Vordergrund. Gib mindestens title, process_name oder process_id an.',
  category: 'desktop',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      title: { type: 'string', description: 'Fenstertitel oder Teilstring' },
      process_name: { type: 'string', description: 'Optionaler Prozessname' },
      process_id: { type: 'number', description: 'Optionale Prozess-ID' },
      exact_match: { type: 'boolean', description: 'Exakten Match statt Teilstring verwenden' },
    },
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input) {
    if (!input.title?.trim() && !input.process_name?.trim() && typeof input.process_id !== 'number') {
      return { data: 'Fehler: title, process_name oder process_id ist erforderlich.' }
    }

    const windowInfo = await invoke<DesktopWindowInfo>('desktop_focus_window', {
      request: {
        title: input.title?.trim() || undefined,
        processName: input.process_name?.trim() || undefined,
        processId: input.process_id,
        exactMatch: Boolean(input.exact_match),
      },
    })

    const verification = await captureDesktopVerificationAttachment(`Fensterfokus fuer ${windowInfo.title || input.process_name || input.process_id || 'Ziel'} angefordert`)

    return {
      data: `${JSON.stringify(windowInfo, null, 2)}\nFokus-Anfrage wurde gesendet. ${verification ? 'Ein aktueller Verifikations-Screenshot wurde angehaengt. Pruefe ihn, bevor du Erfolg behauptest.' : 'Automatische Verifikation nicht verfuegbar; nutze DesktopScreenshot zur Kontrolle.'}`,
      newMessages: verification ? [verification] : undefined,
    }
  },
}

const desktopLaunchAppTool: Tool<{ app_path: string; args?: string[]; cwd?: string; initial_delay_ms?: number }> = {
  name: 'DesktopLaunchApp',
  aliases: ['desktop_launch_app', 'launch_desktop_app'],
  description: 'Startet eine Windows-Desktop-App lokal. Nutze dies, um eine Zielanwendung vor Screenshot- oder UI-Aktionen zu oeffnen.',
  category: 'desktop',
  riskLevel: 'high',
  inputSchema: {
    type: 'object',
    properties: {
      app_path: { type: 'string', description: 'Pfad zur auszufuehrenden .exe oder Anwendung' },
      args: { type: 'array', description: 'Optionale Startargumente' },
      cwd: { type: 'string', description: 'Optionales Arbeitsverzeichnis' },
      initial_delay_ms: { type: 'number', description: 'Optionales Delay nach dem Start' },
    },
    required: ['app_path'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input) {
    const launch = await invoke<DesktopLaunchResponse>('desktop_launch_app', {
      request: {
        path: input.app_path,
        args: input.args ?? [],
        cwd: input.cwd?.trim() || undefined,
        initialDelayMs: input.initial_delay_ms,
      },
    })

    const verification = await captureDesktopVerificationAttachment(`App-Start fuer ${launch.path}`)

    return {
      data: `${JSON.stringify(launch, null, 2)}\nStart-Anfrage wurde gesendet. ${verification ? 'Ein aktueller Verifikations-Screenshot wurde angehaengt. Pruefe ihn, bevor du Folgezustaende beschreibst.' : 'Automatische Verifikation nicht verfuegbar; nutze DesktopScreenshot zur Kontrolle.'}`,
      newMessages: verification ? [verification] : undefined,
    }
  },
}

const desktopMoveMouseTool: Tool<{ x: number; y: number; coordinate_space?: DesktopCoordinateSpace }> = {
  name: 'DesktopMoveMouse',
  aliases: ['desktop_move_mouse', 'move_desktop_mouse'],
  description: 'Bewegt den Mauszeiger. Standard: x/y sind Koordinaten relativ zum aktuellen DesktopScreenshot des Primaerdisplays. Mit coordinate_space="screen" kannst du stattdessen absolute virtuelle Bildschirmkoordinaten angeben.',
  category: 'desktop',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      x: { type: 'number', description: 'X-Koordinate im Screenshot des Primaerdisplays oder absolute X-Koordinate bei coordinate_space="screen"' },
      y: { type: 'number', description: 'Y-Koordinate im Screenshot des Primaerdisplays oder absolute Y-Koordinate bei coordinate_space="screen"' },
      coordinate_space: { type: 'string', description: 'Optional: "display" (Standard, relativ zum Screenshot) oder "screen" (absolut im virtuellen Desktop)', enum: ['display', 'screen'] },
    },
    required: ['x', 'y'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input) {
    const point = await resolveDesktopPoint(input.x, input.y, input.coordinate_space)
    const result = await invoke<DesktopActionResponse>('desktop_move_mouse', {
      request: {
        x: point.absoluteX,
        y: point.absoluteY,
      },
    })

    return { data: `${JSON.stringify(result, null, 2)}\nMausbewegung gesendet: ${describeResolvedDesktopPoint(point)}.` }
  },
}

const desktopClickTool: Tool<{ x: number; y: number; button?: 'left' | 'right'; double_click?: boolean; coordinate_space?: DesktopCoordinateSpace }> = {
  name: 'DesktopClick',
  aliases: ['desktop_click', 'click_desktop'],
  description: 'Klickt an einer Position auf dem Primaerdisplay. Standard: x/y sind Koordinaten relativ zum aktuellen DesktopScreenshot. Mit coordinate_space="screen" kannst du absolute virtuelle Bildschirmkoordinaten angeben.',
  category: 'desktop',
  riskLevel: 'high',
  inputSchema: {
    type: 'object',
    properties: {
      x: { type: 'number', description: 'X-Koordinate im Screenshot des Primaerdisplays oder absolute X-Koordinate bei coordinate_space="screen"' },
      y: { type: 'number', description: 'Y-Koordinate im Screenshot des Primaerdisplays oder absolute Y-Koordinate bei coordinate_space="screen"' },
      button: { type: 'string', description: 'Maustaste', enum: ['left', 'right'] },
      double_click: { type: 'boolean', description: 'Doppelklick statt Einfachklick' },
      coordinate_space: { type: 'string', description: 'Optional: "display" (Standard, relativ zum Screenshot) oder "screen" (absolut im virtuellen Desktop)', enum: ['display', 'screen'] },
    },
    required: ['x', 'y'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input) {
    const point = await resolveDesktopPoint(input.x, input.y, input.coordinate_space)
    const result = await invoke<DesktopActionResponse>('desktop_click', {
      request: {
        x: point.absoluteX,
        y: point.absoluteY,
        button: input.button ?? 'left',
        doubleClick: Boolean(input.double_click),
      },
    })

    const actionLabel = `Klick bei (${point.absoluteX}, ${point.absoluteY})${input.double_click ? ' als Doppelklick' : ''}`
    const verification = await captureDesktopVerificationAttachment(actionLabel)

    return {
      data: `${JSON.stringify(result, null, 2)}\nKlick-Anfrage wurde an ${input.button ?? 'left'} gesendet: ${describeResolvedDesktopPoint(point)}.${input.double_click ? ' Doppelklick aktiv.' : ''} ${verification ? 'Ein aktueller Verifikations-Screenshot wurde angehaengt. Pruefe ihn, bevor du behauptest, dass ein Button wirklich getroffen wurde.' : 'Automatische Verifikation nicht verfuegbar; nutze DesktopScreenshot zur Kontrolle.'}`,
      newMessages: verification ? [verification] : undefined,
    }
  },
}

const desktopTypeTextTool: Tool<{ text: string }> = {
  name: 'DesktopTypeText',
  aliases: ['desktop_type_text', 'type_desktop_text'],
  description: 'Gibt Text in das aktuell fokussierte Windows-Fenster ein. Verwendet Zwischenablage-Einfuegen fuer robuste Eingabe.',
  category: 'desktop',
  riskLevel: 'high',
  inputSchema: {
    type: 'object',
    properties: {
      text: { type: 'string', description: 'Einzugebender Text' },
    },
    required: ['text'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input) {
    const result = await invoke<DesktopActionResponse>('desktop_type_text', {
      request: {
        text: input.text,
      },
    })

    const verification = await captureDesktopVerificationAttachment(`Texteingabe mit ${input.text.length} Zeichen`)

    return {
      data: `${JSON.stringify(result, null, 2)}\nTexteingabe wurde gesendet (${input.text.length} Zeichen). ${verification ? 'Ein aktueller Verifikations-Screenshot wurde angehaengt. Pruefe ihn, bevor du eine erfolgreiche Eingabe beschreibst.' : 'Automatische Verifikation nicht verfuegbar; nutze DesktopScreenshot zur Kontrolle.'}`,
      newMessages: verification ? [verification] : undefined,
    }
  },
}

const desktopKeypressTool: Tool<{ keys: string[] }> = {
  name: 'DesktopKeypress',
  aliases: ['desktop_keypress', 'press_desktop_keys'],
  description: 'Sendet Tasten oder Tastenkombinationen an das aktuell fokussierte Fenster, z. B. ["CTRL", "L"] oder ["ENTER"].',
  category: 'desktop',
  riskLevel: 'high',
  inputSchema: {
    type: 'object',
    properties: {
      keys: { type: 'array', description: 'Array von Tasten oder Modifiern' },
    },
    required: ['keys'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input) {
    const result = await invoke<DesktopActionResponse>('desktop_keypress', {
      request: {
        keys: input.keys,
      },
    })

    const verification = await captureDesktopVerificationAttachment(`Tastendruck ${input.keys.join(' + ')}`)

    return {
      data: `${JSON.stringify(result, null, 2)}\nTastendruck wurde gesendet: ${input.keys.join(' + ')}. ${verification ? 'Ein aktueller Verifikations-Screenshot wurde angehaengt. Pruefe ihn, bevor du einen UI-Erfolg behauptest.' : 'Automatische Verifikation nicht verfuegbar; nutze DesktopScreenshot zur Kontrolle.'}`,
      newMessages: verification ? [verification] : undefined,
    }
  },
}

const desktopScrollTool: Tool<{ scroll_y: number; x?: number; y?: number; coordinate_space?: DesktopCoordinateSpace }> = {
  name: 'DesktopScroll',
  aliases: ['desktop_scroll', 'scroll_desktop'],
  description: 'Scrollt im aktuell fokussierten Fenster oder optional an einer Position auf dem Primaerdisplay. Standard: x/y sind relativ zum Screenshot des Primaerdisplays. Mit coordinate_space="screen" kannst du absolute virtuelle Bildschirmkoordinaten angeben.',
  category: 'desktop',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      scroll_y: { type: 'number', description: 'Vertikaler Scrollwert; positiv nach oben, negativ nach unten' },
      x: { type: 'number', description: 'Optionale X-Koordinate fuer den Mausfokus im Screenshot oder absolut bei coordinate_space="screen"' },
      y: { type: 'number', description: 'Optionale Y-Koordinate fuer den Mausfokus im Screenshot oder absolut bei coordinate_space="screen"' },
      coordinate_space: { type: 'string', description: 'Optional: "display" (Standard, relativ zum Screenshot) oder "screen" (absolut im virtuellen Desktop)', enum: ['display', 'screen'] },
    },
    required: ['scroll_y'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input) {
    const point = typeof input.x === 'number' && typeof input.y === 'number'
      ? await resolveDesktopPoint(input.x, input.y, input.coordinate_space)
      : undefined

    const result = await invoke<DesktopActionResponse>('desktop_scroll', {
      request: {
        x: point?.absoluteX ?? (typeof input.x === 'number' ? Math.round(input.x) : undefined),
        y: point?.absoluteY ?? (typeof input.y === 'number' ? Math.round(input.y) : undefined),
        scrollY: Math.round(input.scroll_y),
      },
    })

    const verification = await captureDesktopVerificationAttachment(`Scrollen mit Delta ${Math.round(input.scroll_y)}`)

    return {
      data: `${JSON.stringify(result, null, 2)}\nScroll-Anfrage wurde gesendet (Delta ${Math.round(input.scroll_y)}).${point ? ` Mausfokus: ${describeResolvedDesktopPoint(point)}.` : ''} ${verification ? 'Ein aktueller Verifikations-Screenshot wurde angehaengt. Pruefe ihn, bevor du das Ergebnis beschreibst.' : 'Automatische Verifikation nicht verfuegbar; nutze DesktopScreenshot zur Kontrolle.'}`,
      newMessages: verification ? [verification] : undefined,
    }
  },
}

// ── AgentTool ──────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/AgentTool/

const agentTool: Tool<{ agent_name: string; prompt: string }> = {
  name: 'Agent',
  aliases: ['agent', 'subagent', 'AgentTool'],
  description: 'Startet einen Sub-Agenten fuer eine bestimmte Aufgabe. Der Agent laeuft in einer isolierten Worker-Sandbox.',
  category: 'agent',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      agent_name: { type: 'string', description: 'Name/Typ des zu startenden Agenten' },
      prompt: { type: 'string', description: 'Aufgabe/Prompt fuer den Sub-Agenten' },
    },
    required: ['agent_name', 'prompt'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input, _context, onProgress) {
    // Sub-agent runs will be dispatched through the query engine
    // This is a placeholder that the query engine intercepts
    if (onProgress) {
      onProgress({ toolUseID: '', data: { type: 'agent_progress', agentName: input.agent_name, content: `Agent "${input.agent_name}" gestartet...` } })
    }
    return { data: `Sub-Agent "${input.agent_name}" fuer Aufgabe: ${input.prompt}` }
  },
}

// ── AskUserTool ────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/AskUserQuestionTool/

type AskUserToolInput = {
  question: string
  options?: Array<string | { label?: string; value?: string }>
  allow_multiple?: boolean
  free_text_label?: string
  free_text_placeholder?: string
}

const askUserTool: Tool<AskUserToolInput> = {
  name: 'AskUser',
  aliases: ['ask_user', 'AskUserQuestionTool'],
  description: 'Stellt dem Benutzer eine strukturierte Rueckfrage und wartet auf Antwort. Nutze options fuer Auswahlmoeglichkeiten und Freitext fuer Zusatzkontext.',
  category: 'user_interaction',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      question: { type: 'string', description: 'Die Frage an den Benutzer' },
      options: {
        type: 'array',
        description: 'Optionale Auswahlmoeglichkeiten als Strings oder Objekte mit label/value.',
        items: { type: 'string' },
      },
      allow_multiple: {
        type: 'boolean',
        description: 'Ob mehrere Optionen ausgewaehlt werden duerfen. Standard: false bei Entscheidungsfragen, sonst true.',
        default: false,
      },
      free_text_label: {
        type: 'string',
        description: 'Beschriftung des Freitextfelds.',
        default: 'Zusatzangaben',
      },
      free_text_placeholder: {
        type: 'string',
        description: 'Placeholder fuer das Freitextfeld.',
        default: 'Optional ergaenzen...',
      },
    },
    required: ['question'],
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => false,
  async call(input, context) {
    const options = Array.isArray(input.options)
      ? input.options.map((opt, idx) => {
          if (typeof opt === 'string') return { label: opt, value: opt }
          const label = opt.label || opt.value || String(idx)
          const value = opt.value || label
          return { label, value }
        })
      : undefined

    const allowMultiple = input.allow_multiple ?? (options && options.length > 0 ? !/(\b(eine|einer|eines|one)\b[^.?!]{0,40}\b(option|optionen|auswahl|choice|choices)\b)/i.test(input.question) : false)

    context.setToolUI?.({
      type: 'input',
      toolName: 'AskUser',
      content: input.question,
      details: { input },
      options,
      allowMultiple,
      allowFreeformInput: true,
      freeTextLabel: input.free_text_label || 'Freitext',
      freeTextPlaceholder: input.free_text_placeholder || 'Optional ergaenzen...',
    })
    return {
      data: `[Warte auf Benutzerantwort: ${input.question}]`,
      awaitUserInput: true,
    }
  },
}

// ── TaskTools ──────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/TaskTool/

const taskCreateTool: Tool<{ title: string; description: string }> = {
  name: 'TaskCreate',
  aliases: ['task_create', 'todo_add'],
  description: 'Erstellt eine neue Aufgabe/Todo.',
  category: 'task',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      title: { type: 'string', description: 'Titel der Aufgabe' },
      description: { type: 'string', description: 'Beschreibung der Aufgabe' },
    },
    required: ['title', 'description'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => true,
  async call(input) {
    const taskId = createToolStreamId()
    await invoke('db_save_task', {
      id: taskId,
      title: input.title,
      prompt: input.description,
      status: 'pending',
      threadId: null,
      createdAt: new Date().toISOString(),
    })
    return { data: `Aufgabe erstellt: ${input.title} (ID: ${taskId})` }
  },
}

const taskListTool: Tool<{ status?: string }> = {
  name: 'TaskList',
  aliases: ['task_list', 'todo_list'],
  description: 'Listet alle aktiven Aufgaben/Todos auf.',
  category: 'task',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      status: { type: 'string', description: 'Filtere nach Status (pending/running/completed/failed)', enum: ['pending', 'running', 'completed', 'failed'] },
    },
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(input) {
    const tasks = await invoke<Array<{ id: string; title: string; status: string }>>('db_list_tasks')
    const filtered = input.status ? tasks.filter(t => t.status === input.status) : tasks
    if (filtered.length === 0) return { data: 'Keine Aufgaben gefunden.' }
    const list = filtered.map(t => `- [${t.status}] ${t.title} (${t.id.slice(0, 8)})`).join('\n')
    return { data: `Aufgaben (${filtered.length}):\n${list}` }
  },
}

// ── MemoryTool ─────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/memory functionality

const memoryReadTool: Tool<{ scope?: string; key?: string }> = {
  name: 'MemoryRead',
  aliases: ['memory_read', 'recall'],
  description: 'Liest Eintraege aus dem Gedaechtnis-System.',
  category: 'memory',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      scope: { type: 'string', description: 'Scope: agent, user, session, shared', enum: ['agent', 'user', 'session', 'shared'] },
      key: { type: 'string', description: 'Optionaler Schluessel zum Filtern' },
    },
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(input) {
    const scope = normalizeMemoryScope(input.scope)
    const entries = await invoke<MemoryEntry[]>('memory_search', {
      scope,
      category: null,
      keyword: null,
      limit: 100,
    })
    const filtered = input.key ? entries.filter(e => e.key.includes(input.key ?? '')) : entries
    if (filtered.length === 0) return { data: 'Keine Erinnerungen gefunden.' }
    return { data: filtered.map(e => `[${e.scope}/${e.category}/${e.key}]: ${e.content}`).join('\n\n') }
  },
}

const memoryWriteTool: Tool<{ scope: string; key: string; content: string }> = {
  name: 'MemoryWrite',
  aliases: ['memory_write', 'remember'],
  description: 'Speichert einen Eintrag im Gedaechtnis-System.',
  category: 'memory',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      scope: { type: 'string', description: 'Scope: agent, user, session, shared', enum: ['agent', 'user', 'session', 'shared'] },
      key: { type: 'string', description: 'Eindeutiger Schluessel' },
      content: { type: 'string', description: 'Zu speichernder Inhalt' },
    },
    required: ['scope', 'key', 'content'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => true,
  async call(input, context) {
    const scope = normalizeMemoryScope(input.scope)
    if (!scope) {
      return { data: 'Fehler: scope ist erforderlich.' }
    }

    await invoke('memory_upsert', {
      id: createToolStreamId(),
      scope,
      category: 'user',
      key: input.key,
      content: input.content,
      sourceSessionId: context.sessionId ?? null,
      confidence: 1.0,
    })
    return { data: `Erinnerung gespeichert: [${scope}/user/${input.key}]` }
  },
}

// ── PlanModeTool ───────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/EnterPlanModeTool/

const enterPlanTool: Tool = {
  name: 'EnterPlanMode',
  aliases: ['plan', 'enter_plan_mode'],
  description: 'Wechselt in den Plan-Modus. Alle Aenderungen werden nur vorgeschlagen, nicht ausgefuehrt.',
  category: 'planning',
  riskLevel: 'low',
  inputSchema: { type: 'object', properties: {} },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(_input, context) {
    context.setAppState(prev => ({ ...prev, planMode: true }))
    return { data: 'Plan-Modus aktiviert. Aenderungen werden nur vorgeschlagen.' }
  },
}

const exitPlanTool: Tool = {
  name: 'ExitPlanMode',
  aliases: ['execute', 'exit_plan_mode'],
  description: 'Verlaesst den Plan-Modus und kehrt zur direkten Ausfuehrung zurueck.',
  category: 'planning',
  riskLevel: 'low',
  inputSchema: { type: 'object', properties: {} },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(_input, context) {
    context.setAppState(prev => ({ ...prev, planMode: false }))
    return { data: 'Plan-Modus deaktiviert. Aenderungen werden direkt ausgefuehrt.' }
  },
}

// ── SkillTool ──────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/SkillTool/

const skillTool: Tool<{ skill_name: string; input: string }> = {
  name: 'Skill',
  aliases: ['skill', 'SkillTool', 'run_skill'],
  description: 'Fuehrt eine gespeicherte Faehigkeit (Skill) aus.',
  category: 'skill',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      skill_name: { type: 'string', description: 'Name des Skills' },
      input: { type: 'string', description: 'Eingabe fuer den Skill' },
    },
    required: ['skill_name', 'input'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input) {
    // Delegate to the existing skill system
    return { data: `Skill "${input.skill_name}" ausgefuehrt mit: ${input.input}` }
  },
}

// ── ListDirTool ────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/LSListDirTool/
// Uses native fs_collect_attachment_metadata IPC instead of PowerShell.

const officeWorkflowTool: Tool<{
  format: 'docx' | 'pptx'
  output_path: string
  mode?: 'parallel' | 'native' | 'template'
  template_path?: string
  transforms?: Record<string, string>
  title?: string
  paragraphs?: string[]
  bullets?: string[]
}> = {
  name: 'OfficeWorkflowTool',
  aliases: ['office_workflow', 'generate_office_workflow', 'docx_template_workflow', 'pptx_template_workflow'],
  description: 'Erzeugt DOCX/PPTX mit nativer Generierung und optionalem Template-Transform (Modi: parallel, native, template).',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      format: { type: 'string', description: 'Zielformat', enum: ['docx', 'pptx'] },
      output_path: { type: 'string', description: 'Zieldatei fuer native/template Ausgabe' },
      mode: { type: 'string', description: 'Workflow-Modus', enum: ['parallel', 'native', 'template'], default: 'parallel' },
      template_path: { type: 'string', description: 'Optionales Template fuer Placeholder-Transform' },
      transforms: { type: 'object', description: 'Schluessel/Wert-Map fuer {{placeholder}} Ersetzung' },
      title: { type: 'string', description: 'Optionaler Titel fuer native Ausgabe und Transform-Defaults' },
      paragraphs: { type: 'array', items: { type: 'string' }, description: 'Optionale Abschnitte fuer native Ausgabe (jeder Eintrag = ein Absatz / Folie)' },
      bullets: { type: 'array', items: { type: 'string' }, description: 'Optionale Bullet-Liste fuer native Ausgabe' },
    },
    required: ['format', 'output_path'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input, context) {
    const response = await invoke<OfficeWorkflowResponse>('fs_generate_office_workflow', {
      request: {
        format: input.format,
        outputPath: resolvePath(input.output_path, context.cwd),
        mode: input.mode,
        templatePath: input.template_path ? resolvePath(input.template_path, context.cwd) : undefined,
        transforms: input.transforms ?? {},
        title: input.title,
        paragraphs: input.paragraphs ?? [],
        bullets: input.bullets ?? [],
      },
      runId: context.runId,
    })

    return { data: JSON.stringify(response, null, 2) }
  },
}

const listDirTool: Tool<{ path: string; recursive?: boolean; max_depth?: number; max_entries?: number }> = {
  name: 'ListDir',
  aliases: ['list_directory', 'ls', 'ListDirTool', 'list_dir', 'dir'],
  description: 'Listet den Inhalt eines Verzeichnisses auf. Zeigt Dateien mit Groesse und Typ.',
  category: 'filesystem',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Pfad zum Verzeichnis' },
      recursive: { type: 'boolean', description: 'Rekursiv auflisten (Standard: false)' },
      max_depth: { type: 'number', description: 'Maximale Tiefe bei Rekursion (Standard: 3)' },
      max_entries: { type: 'number', description: 'Optional: maximale Anzahl Eintraege (Standard: 200)' },
    },
    required: ['path'],
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(input, context) {
    const targetPath = resolvePath(input.path, context.cwd)
    try {
      const maxEntries = input.max_entries ?? 200
      const result = await invoke<FsAttachmentMetadataResponse>('fs_collect_attachment_metadata', {
        path: targetPath,
        maxEntries,
        runId: context.runId,
      })

      if (result.files.length === 0) {
        return { data: 'Verzeichnis ist leer.' }
      }

      const basePath = result.rootPath.replace(/\\/g, '/')
      const lines = result.files.map((file) => {
        const relativePath = file.path.replace(/\\/g, '/').replace(basePath + '/', '').replace(basePath, '')
        const displayPath = relativePath || file.fileName
        const sizeKb = (file.sizeBytes / 1024).toFixed(1)
        const lang = file.language ? ` [${file.language}]` : ''
        const ext = file.extension ? `.${file.extension}` : ''
        return `${displayPath} (${sizeKb} KB${ext}${lang})`
      })

      const header = `Verzeichnis: ${result.rootPath} (${result.totalFiles} Dateien${result.truncated ? ', gekuerzt' : ''})`
      return { data: `${header}\n${lines.join('\n')}` }
    } catch (err) {
      return { data: `Fehler beim Auflisten von "${input.path}": ${err instanceof Error ? err.message : String(err)}` }
    }
  },
}

// ── MultiEditTool ──────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/MultiEditTool/

const multiEditTool: Tool<{ file_path: string; edits: Array<{ old_string: string; new_string: string }> }> = {
  name: 'MultiEdit',
  aliases: ['multi_edit', 'batch_edit', 'MultiEditTool'],
  description: 'Fuehrt mehrere Ersetzungen in einer Datei aus. Jede old_string muss eindeutig sein.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      file_path: { type: 'string', description: 'Pfad zur zu bearbeitenden Datei' },
      edits: {
        type: 'object',
        description: 'Array von {old_string, new_string} Ersetzungen',
      },
    },
    required: ['file_path', 'edits'],
  },
  isReadOnly: () => false,
  isDestructive: () => false,
  isConcurrencySafe: () => false,
  async call(input, context) {
    const fullPath = resolvePath(input.file_path, context.cwd)
    let content = await invoke<string>('fs_extract_text', { path: fullPath, runId: context.runId })

    const results: string[] = []
    let editCount = 0
    const edits = Array.isArray(input.edits) ? input.edits : []

    for (const edit of edits) {
      const occurrences = content.split(edit.old_string).length - 1
      if (occurrences === 0) {
        results.push(`Edit ${editCount + 1}: old_string nicht gefunden`)
        continue
      }
      if (occurrences > 1) {
        results.push(`Edit ${editCount + 1}: old_string ${occurrences}x gefunden (muss eindeutig sein)`)
        continue
      }
      content = content.replace(edit.old_string, edit.new_string)
      editCount++
      results.push(`Edit ${editCount}: OK`)
    }

    if (editCount > 0) {
      await invoke('fs_write_text_file', {
        path: fullPath,
        content,
        createBackup: true,
        runId: context.runId,
      })
    }

    return { data: `${editCount}/${edits.length} Edits ausgefuehrt in ${input.file_path}\n${results.join('\n')}` }
  },
}

// ── TaskUpdateTool ─────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/TaskTool/ (update)

const taskUpdateTool: Tool<{ task_id: string; status?: string; note?: string }> = {
  name: 'TaskUpdate',
  aliases: ['task_update', 'todo_update'],
  description: 'Aktualisiert den Status einer Aufgabe.',
  category: 'task',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      task_id: { type: 'string', description: 'ID der Aufgabe' },
      status: { type: 'string', description: 'Neuer Status', enum: ['pending', 'running', 'completed', 'failed'] },
      note: { type: 'string', description: 'Optionale Notiz/Kommentar' },
    },
    required: ['task_id'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => true,
  async call(input) {
    try {
      await invoke('db_update_task_status', {
        id: input.task_id,
        status: input.status ?? 'running',
      })
      return { data: `Aufgabe ${input.task_id.slice(0, 8)} aktualisiert: ${input.status}${input.note ? ` — ${input.note}` : ''}` }
    } catch (err) {
      return { data: `Fehler beim Aktualisieren: ${err instanceof Error ? err.message : String(err)}` }
    }
  },
}

// ── FileAppendTool ─────────────────────────────────────────────────────────
// Additional utility tool

const fileAppendTool: Tool<{ file_path: string; content: string }> = {
  name: 'Append',
  aliases: ['append_file', 'file_append'],
  description: 'Haengt Inhalt an das Ende einer bestehenden Datei an.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      file_path: { type: 'string', description: 'Pfad zur Datei' },
      content: { type: 'string', description: 'Anzuhaengender Inhalt' },
    },
    required: ['file_path', 'content'],
  },
  isReadOnly: () => false,
  isDestructive: () => false,
  isConcurrencySafe: () => false,
  async call(input, context) {
    const fullPath = resolvePath(input.file_path, context.cwd)
    try {
      const existing = await invoke<string>('fs_extract_text', { path: fullPath, runId: context.runId })
      const newContent = existing + input.content
      await invoke('fs_write_text_file', {
        path: fullPath,
        content: newContent,
        createBackup: false,
        runId: context.runId,
      })
      return { data: `${input.content.length} Zeichen an ${input.file_path} angehaengt.` }
    } catch {
      // File doesn't exist — create it
      await invoke('fs_write_text_file', {
        path: fullPath,
        content: input.content,
        createBackup: false,
        runId: context.runId,
      })
      return { data: `Neue Datei erstellt: ${input.file_path}` }
    }
  },
}

// ── DeleteFileTool ─────────────────────────────────────────────────────────
// Löscht eine Datei mit Sicherheitsbestätigung

const deleteFileTool: Tool<{ file_path: string; confirm: boolean }> = {
  name: 'DeleteFile',
  aliases: ['delete_file', 'remove_file', 'rm', 'DeleteFileTool'],
  description: 'Loescht eine Datei. confirm muss auf true gesetzt werden, um die Loeschung zu bestaetigen.',
  category: 'filesystem',
  riskLevel: 'high',
  inputSchema: {
    type: 'object',
    properties: {
      file_path: { type: 'string', description: 'Pfad zur zu loeschenden Datei' },
      confirm: { type: 'boolean', description: 'Muss true sein, um die Loeschung zu bestaetigen' },
    },
    required: ['file_path', 'confirm'],
  },
  isReadOnly: () => false,
  isDestructive: () => true,
  isConcurrencySafe: () => false,
  async call(input, context, onProgress) {
    if (!input.confirm) {
      return { data: 'Fehler: confirm muss auf true gesetzt werden, um die Datei zu loeschen.' }
    }
    const fullPath = resolvePath(input.file_path, context.cwd)
    onProgress?.({
      toolUseID: '',
      data: {
        type: 'file_progress',
        path: fullPath,
        operation: 'delete',
      },
    })
    try {
      await invoke('fs_delete_file', {
        path: fullPath,
        confirmToken: 'DELETE',
      })
      return { data: `Datei geloescht: ${input.file_path}` }
    } catch (err) {
      return { data: `Fehler beim Loeschen: ${err instanceof Error ? err.message : String(err)}` }
    }
  },
}

// ── FileInfoTool ───────────────────────────────────────────────────────────
// Zeigt Metadaten einer Datei an

const fileInfoTool: Tool<{ path: string }> = {
  name: 'FileInfo',
  aliases: ['file_info', 'stat', 'FileInfoTool', 'file_metadata'],
  description: 'Zeigt Metadaten einer Datei: Groesse, Format, Sprache, Extension.',
  category: 'filesystem',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Pfad zur Datei oder zum Verzeichnis' },
    },
    required: ['path'],
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(input, context) {
    const fullPath = resolvePath(input.path, context.cwd)
    try {
      const result = await invoke<FsAttachmentMetadataResponse>('fs_collect_attachment_metadata', {
        path: fullPath,
        maxEntries: 1,
        runId: context.runId,
      })

      if (result.rootKind === 'file' && result.files.length > 0) {
        const file = result.files[0]
        const sizeKb = (file.sizeBytes / 1024).toFixed(1)
        const sizeMb = (file.sizeBytes / (1024 * 1024)).toFixed(2)
        const lines = [
          `Datei: ${file.fileName}`,
          `Pfad: ${file.path}`,
          `Groesse: ${sizeKb} KB (${sizeMb} MB, ${file.sizeBytes} Bytes)`,
          file.extension ? `Extension: .${file.extension}` : null,
          file.language ? `Sprache: ${file.language}` : null,
        ].filter(Boolean)
        return { data: lines.join('\n') }
      }

      if (result.rootKind === 'folder') {
        return {
          data: [
            `Verzeichnis: ${result.rootPath}`,
            `Dateien insgesamt: ${result.totalFiles}`,
            `Dateien angezeigt: ${result.returnedFiles}`,
            result.truncated ? 'Anzeige gekuerzt.' : null,
          ].filter(Boolean).join('\n'),
        }
      }

      return { data: `Pfad nicht gefunden: ${input.path}` }
    } catch (err) {
      return { data: `Fehler: ${err instanceof Error ? err.message : String(err)}` }
    }
  },
}

// ── RenameFileTool ─────────────────────────────────────────────────────────
// Benennt eine Datei oder einen Ordner um (Wrapper um fs_move_path)

const renameFileTool: Tool<{ path: string; new_name: string }> = {
  name: 'RenameFile',
  aliases: ['rename_file', 'rename', 'RenameFileTool'],
  description: 'Benennt eine Datei oder einen Ordner um. Nur der Name aendert sich, der Speicherort bleibt gleich.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Pfad zur umzubenennenden Datei oder zum Ordner' },
      new_name: { type: 'string', description: 'Neuer Dateiname (ohne Pfad, nur der Name)' },
    },
    required: ['path', 'new_name'],
  },
  isReadOnly: () => false,
  isDestructive: () => false,
  isConcurrencySafe: () => false,
  async call(input, context, onProgress) {
    const fullPath = resolvePath(input.path, context.cwd)
    // Build destination: same parent directory + new name
    const pathParts = fullPath.replace(/\\/g, '/').split('/')
    pathParts[pathParts.length - 1] = input.new_name
    const destinationPath = pathParts.join('/')

    onProgress?.({
      toolUseID: '',
      data: {
        type: 'file_progress',
        path: `${fullPath} -> ${destinationPath}`,
        operation: 'rename',
      },
    })

    try {
      const result = await invoke<FsPathMutationResponse>('fs_move_path', {
        sourcePath: fullPath,
        destinationPath,
        overwrite: false,
        runId: context.runId,
      })
      return { data: `${result.itemKind} umbenannt: ${input.path} -> ${input.new_name}` }
    } catch (err) {
      return { data: `Fehler beim Umbenennen: ${err instanceof Error ? err.message : String(err)}` }
    }
  },
}

// ── ThinkTool ──────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/ThinkTool/

const thinkTool: Tool<{ thought: string }> = {
  name: 'Think',
  aliases: ['think', 'ThinkTool', 'reasoning'],
  description: 'Nutze dieses Tool zum Nachdenken und Planen, bevor du handelst. Hilft bei komplexen mehrstufigen Aufgaben.',
  category: 'planning',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      thought: { type: 'string', description: 'Dein Gedanke/Plan/Ueberlegung' },
    },
    required: ['thought'],
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(input) {
    return { data: `[Gedanke notiert: ${input.thought.slice(0, 200)}]` }
  },
}

// ── Register All Built-in Tools ────────────────────────────────────────────

export function registerAllBuiltinTools(): void {
  // Prevent double registration
  if (toolRegistry.length > 0) return

  const tools = [
    fileReadTool,
    fileWriteTool,
    createDirectoryTool,
    movePathTool,
    copyPathTool,
    fileEditTool,
    globTool,
    grepTool,
    bashTool,
    webFetchTool,
    webSearchTool,
    mcpTool,
    desktopScreenshotTool,
    desktopPrimaryDisplayTool,
    desktopListWindowsTool,
    desktopFocusWindowTool,
    desktopLaunchAppTool,
    desktopMoveMouseTool,
    desktopClickTool,
    desktopTypeTextTool,
    desktopKeypressTool,
    desktopScrollTool,
    agentTool,
    askUserTool,
    taskCreateTool,
    taskListTool,
    taskUpdateTool,
    memoryReadTool,
    memoryWriteTool,
    enterPlanTool,
    exitPlanTool,
    skillTool,
    // Filesystem tools
    officeWorkflowTool,
    listDirTool,
    multiEditTool,
    fileAppendTool,
    deleteFileTool,
    fileInfoTool,
    renameFileTool,
    // Utility tools
    thinkTool,
  ]
  for (const tool of tools) {
    registerTool(tool)
  }
}

// ── Get Anthropic Tool Definitions ─────────────────────────────────────────

export function getToolDefinitions(): Array<{ name: string; description: string; input_schema: ToolInputSchema; aliases?: string[] }> {
  return getEnabledTools().map(t => ({
    name: t.name,
    description: t.description,
    input_schema: t.inputSchema,
    aliases: t.aliases,
  }))
}

// ── Helpers ────────────────────────────────────────────────────────────────

function resolvePath(path: string, cwd: string): string {
  if (path.match(/^[a-zA-Z]:\\/)) return path  // absolute Windows
  if (path.startsWith('/')) return path  // absolute Unix
  return `${cwd.replace(/\/$/, '')}/${path}`
}

function globToRegex(pattern: string): RegExp {
  const escaped = pattern
    .replace(/[.+^${}()|[\]\\]/g, '\\$&')
    .replace(/\*\*/g, '§DOUBLESTAR§')
    .replace(/\*/g, '[^/\\\\]*')
    .replace(/§DOUBLESTAR§/g, '.*')
    .replace(/\?/g, '.')
  return new RegExp(escaped, 'i')
}
