// ── Tool Registry & Built-in Tools (ported from Claude Code) ────────────────
// Mirrors: claude-code-main/src/tools.ts + tools/*
// All file/shell operations delegate to Tauri IPC commands

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useConfigStore } from '../../stores/configStore'
import { useTerminalStore } from '../../stores/terminalStore'
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

type DesktopscreenshotResponse = DesktopDisplayInfo & {
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

type McpscreenshotPayload = {
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

function formatDesktopscreenshotSummary(screenshot: DesktopscreenshotResponse): string {
  const imageWidth = screenshot.imageWidth ?? screenshot.width
  const imageHeight = screenshot.imageHeight ?? screenshot.height
  const scaleFactor = typeof screenshot.scaleFactor === 'number' ? ` DPI-Skalierung ca. ${screenshot.scaleFactor.toFixed(2)}x.` : ''
  const overlay = screenshot.coordinateOverlay
    ? ' The image contains a coordinate grid: fine lines every 50 px, labeled major lines every 100 px.'
    : ''
  return `Desktop screenshot captured: Image ${imageWidth}x${imageHeight}, Display ${screenshot.width}x${screenshot.height} on ${screenshot.deviceName}${screenshot.primary ? ' [primary]' : ''}.${scaleFactor} Use local display coordinates from this exact image for clicks, with origin (0, 0) at the top-left; this display's virtual screen origin is at (${screenshot.x}, ${screenshot.y}).${overlay}`
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
    return `Screen coordinates (${point.absoluteX}, ${point.absoluteY})`
  }

  return `Display coordinates (${point.requestedX}, ${point.requestedY}) were converted from display origin (${point.display.x}, ${point.display.y}) to screen coordinates (${point.absoluteX}, ${point.absoluteY}) umgerechnet`
}

function createDesktopscreenshotAttachment(
  screenshot: DesktopscreenshotResponse,
  options?: { title?: string; preface?: string },
): AttachmentMessage | null {
  const parsed = parseBase64DataUrl(screenshot.dataUrl)
  if (!parsed) return null

  const summary = formatDesktopscreenshotSummary(screenshot)
  const text = options?.preface ? `${options.preface}\n${summary}` : summary

  return {
    type: 'attachment',
    uuid: createToolStreamId(),
    title: options?.title ?? `Desktop screenshot ${screenshot.width}x${screenshot.height}`,
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

function parseMcpscreenshotPayload(message: string): McpscreenshotPayload | null {
  try {
    const parsed = JSON.parse(message)
    if (!parsed || typeof parsed !== 'object') return null
    return parsed as McpscreenshotPayload
  } catch {
    return null
  }
}

function createMcpscreenshotAttachment(
  toolName: string,
  message: string,
): { attachment: AttachmentMessage; textSummary: string } | null {
  if (toolName !== 'screenshot_for_display') {
    return null
  }

  const payload = parseMcpscreenshotPayload(message)
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
    ? `MCP screenshot ${width}x${height}${reused ? ' (reused)' : ''}`
    : `MCP screenshot${reused ? ' (reused)' : ''}`

  const overlayEnabled = payload.coordinateOverlay === true || payload.displayInfo?.coordinateOverlay === true
  const grid = payload.coordinateGrid ?? payload.displayInfo?.coordinateGrid
  const coordinateGuide = overlayEnabled
    ? `coordinate hint: Das attachede screenshot-Image enthaelt ein Raster in lokalen Display coordinates mit origin (0, 0) top-left. Minor lines: ${grid?.minorStepPx ?? 50}px, labeled major lines: ${grid?.majorStepPx ?? 100}px. Use these image coordinates directly for DesktopClick/DesktopMoveMouse with coordinate_space="display".`
    : 'coordinate hint: Use lokale Display coordinates aus dem attacheden screenshot-Image direkt for DesktopClick/DesktopMoveMouse mit coordinate_space="display".'
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
    const screenshot = await invoke<DesktopscreenshotResponse>('desktop_capture_primary_annotated_screenshot')
    return createDesktopscreenshotAttachment(screenshot, {
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
  description: 'Reads file contents. Use offset/limit for large files.',
  category: 'filesystem',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      file_path: { type: 'string', description: 'Absolute or relative path to the file' },
      offset: { type: 'number', description: 'Start line (0-based)' },
      limit: { type: 'number', description: 'Maximum number of lines' },
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
  description: 'Writes content to a file. Creates parent directories if needed.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      file_path: { type: 'string', description: 'Absolute or relative path to the file' },
      content: { type: 'string', description: 'Content to write' },
      create_backup: { type: 'boolean', description: 'Create a backup of the original file' },
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
    return { data: result.diff || 'File geschrieben.' }
  },
}

// ── FileEditTool ───────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/FileEditTool/

const createDirectoryTool: Tool<{ path: string }> = {
  name: 'CreateDirectory',
  aliases: ['create_directory', 'mkdir', 'make_dir', 'MakeDirectoryTool'],
  description: 'Creates a directory, including missing parent folders.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Path to the directory to create' },
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
    return { data: result.created ? `directory created: ${result.path}` : `directory existiert readys: ${result.path}` }
  },
}

const movePathTool: Tool<{ source_path: string; destination_path: string; overwrite?: boolean }> = {
  name: 'MovePath',
  aliases: ['move_path', 'move_file', 'move_directory', 'rename_path', 'MovePathTool'],
  description: 'Moves or renames a file or folder.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      source_path: { type: 'string', description: 'Source path of the file or folder' },
      destination_path: { type: 'string', description: 'Destination path after moving' },
      overwrite: { type: 'boolean', description: 'Overwrite existing target (default: false)' },
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
      `${result.itemKind} moved: ${result.sourcePath} -> ${result.destinationPath}`,
      result.createdParent ? 'Target folder was created automatically.' : '',
      result.replacedExisting ? 'Existing Target was replaced.' : '',
    ].filter(Boolean)
    return { data: notes.join('\n') }
  },
}

const copyPathTool: Tool<{ source_path: string; destination_path: string; overwrite?: boolean }> = {
  name: 'CopyPath',
  aliases: ['copy_path', 'copy_file', 'copy_directory', 'CopyPathTool'],
  description: 'Copies a file or folder to a new path.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      source_path: { type: 'string', description: 'Source of the file or folder' },
      destination_path: { type: 'string', description: 'Destination path for the copy' },
      overwrite: { type: 'boolean', description: 'Overwrite existing target (default: false)' },
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
      `${result.itemKind} copied: ${result.sourcePath} -> ${result.destinationPath}`,
      result.createdParent ? 'Target folder was created automatically.' : '',
      result.replacedExisting ? 'Existing Target was replaced.' : '',
    ].filter(Boolean)
    return { data: notes.join('\n') }
  },
}

const fileEditTool: Tool<{ file_path: string; old_string: string; new_string: string }> = {
  name: 'Edit',
  aliases: ['edit_file', 'FileEditTool'],
  description: 'Replaces an exact string in a file. old_string must be unique.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      file_path: { type: 'string', description: 'Path to the file to edit' },
      old_string: { type: 'string', description: 'Exact text to replace' },
      new_string: { type: 'string', description: 'New replacement text' },
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
      return { data: `Error: old_string was not found in ${input.file_path}.` }
    }
    if (occurrences > 1) {
      return { data: `Error: old_string was found ${occurrences} times. It must be unique.` }
    }
    const newContent = content.replace(input.old_string, input.new_string)
    await invoke('fs_write_text_file', {
      path: fullPath,
      content: newContent,
      createBackup: true,
      runId: context.runId,
    })
    return { data: `File edited: ${input.file_path}` }
  },
}

// ── GlobTool ───────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/GlobTool/

const globTool: Tool<{ pattern: string; path?: string }> = {
  name: 'Glob',
  aliases: ['glob', 'GlobTool'],
  description: 'Searches files by glob pattern. Fast for finding files by name/extension.',
  category: 'search',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      pattern: { type: 'string', description: 'Glob pattern (e.g. **/*.ts, src/**/*.rs)' },
      path: { type: 'string', description: 'Base directory for the search (optional)' },
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
    return { data: matches.length > 0 ? matches.join('\n') : 'No results.' }
  },
}

// ── GrepTool ───────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/GrepTool/

const grepTool: Tool<{ pattern: string; path?: string; include?: string }> = {
  name: 'Grep',
  aliases: ['grep', 'GrepTool', 'search'],
  description: 'Searches file contents with regular expressions. Fast for code search.',
  category: 'search',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      pattern: { type: 'string', description: 'Regular expression to search for' },
      path: { type: 'string', description: 'Directory for the search (optional)' },
      include: { type: 'string', description: 'Filename pattern to include (e.g. *.ts)' },
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
      return { data: result.stdout || 'No results.' }
    } catch {
      return { data: `Grep search for "${input.pattern}" — Tauri exec_command is not available. Use fallback.` }
    }
  },
}

// ── BashTool ───────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/BashTool/

const bashTool: Tool<{ command: string; timeout?: number }> = {
  name: 'Bash',
  aliases: ['bash', 'shell', 'BashTool', 'execute'],
  description: 'Runs a shell command (PowerShell on Windows). Use for builds, tests, Git, etc.',
  category: 'shell',
  riskLevel: 'high',
  inputSchema: {
    type: 'object',
    properties: {
      command: { type: 'string', description: 'Shell command to execute' },
      timeout: { type: 'number', description: 'Timeout in milliseconds (default: 30000)' },
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
    const terminalThreadId = useTerminalStore.getState().activeAiThreadId
    if (terminalThreadId) {
      try {
        await invoke('shell_command_validate', {
          command: input.command,
          cwd: context.cwd,
          runId: context.runId,
        })
        if (onProgress) {
          onProgress({
            toolUseID: '',
            data: {
              type: 'bash_progress',
              output: 'terminal: starting command',
            },
          })
        }
        const result = await useTerminalStore.getState().runAiCommand({
          threadId: terminalThreadId,
          command: input.command,
          cwd: context.cwd,
          timeoutMs: input.timeout ?? 30000,
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
              exitCode: result.exitCode ?? undefined,
            },
          })
        }
        const shouldMirrorCwdToStdout = /\b(?:pwd|Get-Location)\b/i.test(input.command)
        const effectiveStdout = result.stdout || (shouldMirrorCwdToStdout && resolvedCurrentCwd ? resolvedCurrentCwd : '')
        const output = [
          effectiveStdout ? `stdout:\n${effectiveStdout}` : '',
          result.stderr ? `stderr:\n${result.stderr}` : '',
          result.interruptedByUser ? 'note: user manually intervened in the terminal while this command was running.' : '',
          resolvedCurrentCwd ? `current cwd: ${resolvedCurrentCwd}` : '',
          `exit code: ${result.exitCode}`,
        ].filter(Boolean).join('\n\n')
        return { data: output }
      } catch (err) {
        return { data: `Error while running in terminal: ${err instanceof Error ? err.message : String(err)}` }
      }
    }

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
      return { data: `Error while running: ${err instanceof Error ? err.message : String(err)}` }
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
  description: 'Fetches URL text content and extracts the main text.',
  category: 'web',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      url: { type: 'string', description: 'URL to fetch' },
      max_chars: { type: 'number', description: 'Maximum character count (default: 50000)' },
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
  description: 'Searches the web for information about a topic.',
  category: 'web',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      query: { type: 'string', description: 'Search query' },
      max_results: { type: 'number', description: 'Maximum result count (default: 5)' },
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
      return { data: lines.join('\n\n') || `No results for "${input.query}"` }
    } catch {
      return { data: `Web search failed for: "${input.query}"` }
    }
  },
}

// ── MCPTool ────────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/MCPTool/

const mcpTool: Tool<{ server_name: string; tool_name: string; arguments: Record<string, unknown> }> = {
  name: 'MCPTool',
  aliases: ['mcp_call', 'mcp'],
  description: 'Calls a tool on an MCP server.',
  category: 'mcp',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      server_name: { type: 'string', description: 'Name of the MCP server' },
      tool_name: { type: 'string', description: 'Name of the tool on the server' },
      arguments: { type: 'object', description: 'Arguments for the tool call' },
    },
    required: ['server_name', 'tool_name', 'arguments'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input, context) {
    const server = findMcpServerConfig(input.server_name)
    if (!server) {
      return { data: `MCP Error: Server "${input.server_name}" ist not configured.` }
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
        return { data: `MCP Error: ${normalized.message}` }
      }

      const screenshotAttachment = createMcpscreenshotAttachment(input.tool_name, normalized.message)
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

const desktopscreenshotTool: Tool<Record<string, never>> = {
  name: 'Desktopscreenshot',
  aliases: ['desktop_screenshot', 'capture_desktop_screenshot'],
  description: 'Captures a screenshot of the primary display with a coordinate grid and attaches the image for visual analysis. Use this tool before mouse or keyboard actions.',
  category: 'desktop',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {},
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => false,
  async call() {
    const screenshot = await invoke<DesktopscreenshotResponse>('desktop_capture_primary_annotated_screenshot')
    const attachment = createDesktopscreenshotAttachment(screenshot)
    return {
      data: `${formatDesktopscreenshotSummary(screenshot)} ${attachment ? 'The image was attached for visual analysis.' : 'The image could not be attached.'}`,
      newMessages: attachment ? [attachment] : undefined,
    }
  },
}

const desktopPrimaryDisplayTool: Tool<Record<string, never>> = {
  name: 'DesktopPrimaryDisplay',
  aliases: ['desktop_primary_display', 'get_desktop_primary_display'],
  description: 'Reads geometry and origin of the primary display. Useful for coordinates during mouse actions.',
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
  description: 'Lists visible desktop windows with title, process, and bounds. Use this to find target windows for focus or interaction.',
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
      return { data: 'No visible desktop windows found.' }
    }
    return { data: JSON.stringify(windows, null, 2) }
  },
}

const desktopFocusWindowTool: Tool<{ title?: string; process_name?: string; process_id?: number; exact_match?: boolean }> = {
  name: 'DesktopFocusWindow',
  aliases: ['desktop_focus_window', 'focus_desktop_window'],
  description: 'Brings a desktop window to the foreground. Provide at least title, process_name, or process_id.',
  category: 'desktop',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      title: { type: 'string', description: 'Window title or substring' },
      process_name: { type: 'string', description: 'Optional process name' },
      process_id: { type: 'number', description: 'Optional process ID' },
      exact_match: { type: 'boolean', description: 'Use exact match instead of substring' },
    },
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input) {
    if (!input.title?.trim() && !input.process_name?.trim() && typeof input.process_id !== 'number') {
      return { data: 'Error: title, process_name, or process_id is required.' }
    }

    const windowInfo = await invoke<DesktopWindowInfo>('desktop_focus_window', {
      request: {
        title: input.title?.trim() || undefined,
        processName: input.process_name?.trim() || undefined,
        processId: input.process_id,
        exactMatch: Boolean(input.exact_match),
      },
    })

    const verification = await captureDesktopVerificationAttachment(`windowfokus for ${windowInfo.title || input.process_name || input.process_id || 'Target'} angefordert`)

    return {
      data: `${JSON.stringify(windowInfo, null, 2)}\nFocus request was sent. ${verification ? 'A current verification screenshot was attached. Check it before claiming success.' : 'Automatic verification is not available; use DesktopScreenshot to check.'}`,
      newMessages: verification ? [verification] : undefined,
    }
  },
}

const desktopLaunchAppTool: Tool<{ app_path: string; args?: string[]; cwd?: string; initial_delay_ms?: number }> = {
  name: 'DesktopLaunchApp',
  aliases: ['desktop_launch_app', 'launch_desktop_app'],
  description: 'Starts a Windows desktop app locally. Use this to open a target application before screenshot or UI actions.',
  category: 'desktop',
  riskLevel: 'high',
  inputSchema: {
    type: 'object',
    properties: {
      app_path: { type: 'string', description: 'Path to the .exe or application to run' },
      args: { type: 'array', description: 'Optional start arguments', items: { type: 'string', description: 'Single command-line argument' } },
      cwd: { type: 'string', description: 'Optional working directory' },
      initial_delay_ms: { type: 'number', description: 'Optional delay after launch' },
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

    const verification = await captureDesktopVerificationAttachment(`App-Start for ${launch.path}`)

    return {
      data: `${JSON.stringify(launch, null, 2)}\nStart request was sent. ${verification ? 'A current verification screenshot was attached. Check it before describing follow-up states.' : 'Automatic verification is not available; use DesktopScreenshot to check.'}`,
      newMessages: verification ? [verification] : undefined,
    }
  },
}

const desktopMoveMouseTool: Tool<{ x: number; y: number; coordinate_space?: DesktopCoordinateSpace }> = {
  name: 'DesktopMoveMouse',
  aliases: ['desktop_move_mouse', 'move_desktop_mouse'],
  description: 'Moves the mouse pointer. Default: x/y are coordinates relative to the current desktop screenshot of the primary display. With coordinate_space="screen", you can instead provide absolute virtual screen coordinates.',
  category: 'desktop',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      x: { type: 'number', description: 'X coordinate in the primary display screenshot, or absolute X coordinate when coordinate_space="screen"' },
      y: { type: 'number', description: 'Y coordinate in the primary display screenshot, or absolute Y coordinate when coordinate_space="screen"' },
      coordinate_space: { type: 'string', description: 'Optional: "display" (default, relative to the screenshot) or "screen" (absolute in the virtual desktop)', enum: ['display', 'screen'] },
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

    return { data: `${JSON.stringify(result, null, 2)}\nMouse movement sent: ${describeResolvedDesktopPoint(point)}.` }
  },
}

const desktopClickTool: Tool<{ x: number; y: number; button?: 'left' | 'right'; double_click?: boolean; coordinate_space?: DesktopCoordinateSpace }> = {
  name: 'DesktopClick',
  aliases: ['desktop_click', 'click_desktop'],
  description: 'Clicks at a position on the primary display. Default: x/y are coordinates relative to the current desktop screenshot. With coordinate_space="screen", you can provide absolute virtual screen coordinates.',
  category: 'desktop',
  riskLevel: 'high',
  inputSchema: {
    type: 'object',
    properties: {
      x: { type: 'number', description: 'X coordinate in the primary display screenshot, or absolute X coordinate when coordinate_space="screen"' },
      y: { type: 'number', description: 'Y coordinate in the primary display screenshot, or absolute Y coordinate when coordinate_space="screen"' },
      button: { type: 'string', description: 'mouse button', enum: ['left', 'right'] },
      double_click: { type: 'boolean', description: 'Double-click instead of single click' },
      coordinate_space: { type: 'string', description: 'Optional: "display" (default, relative to the screenshot) or "screen" (absolute in the virtual desktop)', enum: ['display', 'screen'] },
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

    const actionLabel = `Click at (${point.absoluteX}, ${point.absoluteY})${input.double_click ? ' as double-click' : ''}`
    const verification = await captureDesktopVerificationAttachment(actionLabel)

    return {
      data: `${JSON.stringify(result, null, 2)}\nClick request was sent to ${input.button ?? 'left'}: ${describeResolvedDesktopPoint(point)}.${input.double_click ? ' Double-click active.' : ''} ${verification ? 'A current verification screenshot was attached. Check it before claiming that a button was actually hit.' : 'Automatic verification is not available; use DesktopScreenshot to check.'}`,
      newMessages: verification ? [verification] : undefined,
    }
  },
}

const desktopTypeTextTool: Tool<{ text: string }> = {
  name: 'DesktopTypeText',
  aliases: ['desktop_type_text', 'type_desktop_text'],
  description: 'Types text into the currently focused Windows window. Uses clipboard paste for robust input.',
  category: 'desktop',
  riskLevel: 'high',
  inputSchema: {
    type: 'object',
    properties: {
      text: { type: 'string', description: 'Text to type' },
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
      data: `${JSON.stringify(result, null, 2)}\nText input was sent (${input.text.length} characters). ${verification ? 'A current verification screenshot was attached. Check it before describing the input as successful.' : 'Automatic verification is not available; use DesktopScreenshot to check.'}`,
      newMessages: verification ? [verification] : undefined,
    }
  },
}

const desktopKeypressTool: Tool<{ keys: string[] }> = {
  name: 'DesktopKeypress',
  aliases: ['desktop_keypress', 'press_desktop_keys'],
  description: 'Sends keys or key combinations to the currently focused window, e.g. ["CTRL", "L"] or ["ENTER"].',
  category: 'desktop',
  riskLevel: 'high',
  inputSchema: {
    type: 'object',
    properties: {
      keys: { type: 'array', description: 'Array of keys or modifiers', items: { type: 'string', description: 'Single key or modifier' } },
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
      data: `${JSON.stringify(result, null, 2)}\nKeystroke was sent: ${input.keys.join(' + ')}. ${verification ? 'A current verification screenshot was attached. Check it before claiming UI success.' : 'Automatic verification is not available; use DesktopScreenshot to check.'}`,
      newMessages: verification ? [verification] : undefined,
    }
  },
}

const desktopScrollTool: Tool<{ scroll_y: number; x?: number; y?: number; coordinate_space?: DesktopCoordinateSpace }> = {
  name: 'DesktopScroll',
  aliases: ['desktop_scroll', 'scroll_desktop'],
  description: 'Scrolls in the currently focused window or optionally at a position on the primary display. Default: x/y are relative to the primary display screenshot. With coordinate_space="screen", you can provide absolute virtual screen coordinates.',
  category: 'desktop',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      scroll_y: { type: 'number', description: 'Vertikaler Scrollwert; positiv nach oben, negativ nach unten' },
      x: { type: 'number', description: 'Optional X coordinate for mouse focus in the screenshot, or absolute when coordinate_space="screen"' },
      y: { type: 'number', description: 'Optional Y coordinate for mouse focus in the screenshot, or absolute when coordinate_space="screen"' },
      coordinate_space: { type: 'string', description: 'Optional: "display" (default, relative to the screenshot) or "screen" (absolute in the virtual desktop)', enum: ['display', 'screen'] },
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
      data: `${JSON.stringify(result, null, 2)}\nScroll request was sent (Delta ${Math.round(input.scroll_y)}).${point ? ` mouse focus: ${describeResolvedDesktopPoint(point)}.` : ''} ${verification ? 'A current verification screenshot was attached. Check it before describing the result.' : 'Automatic verification is not available; use DesktopScreenshot to check.'}`,
      newMessages: verification ? [verification] : undefined,
    }
  },
}

// ── AgentTool ──────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/AgentTool/

const agentTool: Tool<{ agent_name: string; prompt: string }> = {
  name: 'Agent',
  aliases: ['agent', 'subagent', 'AgentTool'],
  description: 'Starts a sub-agent for a specific task. The agent runs in an isolated worker sandbox.',
  category: 'agent',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      agent_name: { type: 'string', description: 'Name/type of the agent to start' },
      prompt: { type: 'string', description: 'Task/prompt for the sub-agent' },
    },
    required: ['agent_name', 'prompt'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input, _context, onProgress) {
    // Sub-agent runs will be dispatched through the query engine
    // This is a placeholder that the query engine intercepts
    if (onProgress) {
      onProgress({ toolUseID: '', data: { type: 'agent_progress', agentName: input.agent_name, content: `Agent "${input.agent_name}" started...` } })
    }
    return { data: `Sub-Agent "${input.agent_name}" for Task: ${input.prompt}` }
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
  description: 'Asks the user a structured question and waits for an answer. Use options for choices and free text for additional context.',
  category: 'user_interaction',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      question: { type: 'string', description: 'Question for the user' },
      options: {
        type: 'array',
        description: 'Optional choices as strings or objects with label/value.',
        items: { type: 'string' },
      },
      allow_multiple: {
        type: 'boolean',
        description: 'Whether multiple options may be selected. Default: false for decision questions, otherwise true.',
        default: false,
      },
      free_text_label: {
        type: 'string',
        description: 'Label for the free-text field.',
        default: 'Zusatzangaben',
      },
      free_text_placeholder: {
        type: 'string',
        description: 'Placeholder for the free-text field.',
        default: 'Add optional details...',
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
      freeTextPlaceholder: input.free_text_placeholder || 'Add optional details...',
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
  description: 'Creates a new task/todo.',
  category: 'task',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      title: { type: 'string', description: 'Task title' },
      description: { type: 'string', description: 'Task description' },
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
    return { data: `Task created: ${input.title} (ID: ${taskId})` }
  },
}

const taskListTool: Tool<{ status?: string }> = {
  name: 'TaskList',
  aliases: ['task_list', 'todo_list'],
  description: 'Lists all active tasks/todos.',
  category: 'task',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      status: { type: 'string', description: 'Filter by status (pending/running/completed/failed)', enum: ['pending', 'running', 'completed', 'failed'] },
    },
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(input) {
    const tasks = await invoke<Array<{ id: string; title: string; status: string }>>('db_list_tasks')
    const filtered = input.status ? tasks.filter(t => t.status === input.status) : tasks
    if (filtered.length === 0) return { data: 'No tasks found.' }
    const list = filtered.map(t => `- [${t.status}] ${t.title} (${t.id.slice(0, 8)})`).join('\n')
    return { data: `Tasks (${filtered.length}):\n${list}` }
  },
}

// ── MemoryTool ─────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/memory functionality

const memoryReadTool: Tool<{ scope?: string; key?: string }> = {
  name: 'MemoryRead',
  aliases: ['memory_read', 'recall'],
  description: 'Reads entries from the memory system.',
  category: 'memory',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      scope: { type: 'string', description: 'Scope: agent, user, session, shared', enum: ['agent', 'user', 'session', 'shared'] },
      key: { type: 'string', description: 'Optional key to filter by' },
    },
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(input) {
    const scope = normalizeMemoryScope(input.scope)
    if (scope === 'user') {
      const profile = await invoke<Array<{ key: string; value: string; source: string; confidence: number }>>('user_profile_list')
      const filtered = input.key
        ? profile.filter((entry) => entry.key.includes(input.key ?? ''))
        : profile
      if (filtered.length === 0) return { data: 'No user-profile memories found.' }
      return { data: filtered.map((entry) => `[user/${entry.key}]: ${entry.value}`).join('\n\n') }
    }
    const entries = await invoke<MemoryEntry[]>('memory_search', {
      scope,
      category: null,
      keyword: null,
      limit: 100,
    })
    const filtered = input.key ? entries.filter(e => e.key.includes(input.key ?? '')) : entries
    if (filtered.length === 0) return { data: 'No reminders found.' }
    return { data: filtered.map(e => `[${e.scope}/${e.category}/${e.key}]: ${e.content}`).join('\n\n') }
  },
}

type MemoryWriteInput = {
  action?: 'add' | 'replace' | 'remove'
  target?: 'memory' | 'user'
  old_text?: string
  content?: string
  scope?: string
  key?: string
}

type MemoryMutationResponse = {
  success: boolean
  changed: boolean
  action: string
  target: string
  message: string
  usageChars: number
  limitChars: number
  entries: string[]
}

const memoryWriteTool: Tool<MemoryWriteInput> = {
  name: 'MemoryWrite',
  aliases: ['memory', 'memory_write', 'remember'],
  description: 'Curates persistent memory with add, replace, or remove. Target memory for durable project/environment facts and user for stable user preferences. Writes are bounded, deduplicated, and visible from the next session snapshot.',
  category: 'memory',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      action: { type: 'string', description: 'Mutation action', enum: ['add', 'replace', 'remove'] },
      target: { type: 'string', description: 'memory for agent notes, user for user profile', enum: ['memory', 'user'] },
      old_text: { type: 'string', description: 'Unique substring for replace or remove' },
      content: { type: 'string', description: 'New content for add or replace' },
      scope: { type: 'string', description: 'Legacy scope for compatibility', enum: ['agent', 'user', 'session', 'shared'] },
      key: { type: 'string', description: 'Legacy unique key for session/shared writes' },
    },
    required: ['action', 'target'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => true,
  async call(input, context) {
    const action = input.action ?? 'add'
    const target = input.target ?? (input.scope === 'user' ? 'user' : 'memory')

    // Preserve compatibility for explicit session/shared keyed writes.
    const legacyScope = normalizeMemoryScope(input.scope)
    if (legacyScope === 'session' || legacyScope === 'shared') {
      if (!input.key?.trim() || !input.content?.trim()) {
        return { data: 'Error: key and content are required for session/shared memory.' }
      }
      await invoke('memory_upsert', {
        id: createToolStreamId(),
        scope: legacyScope,
        category: legacyScope === 'shared' ? 'knowledge' : 'context',
        key: input.key,
        content: input.content,
        sourceSessionId: context.sessionId ?? null,
        confidence: 1.0,
      })
      return { data: `Memory saved: [${legacyScope}/${input.key}]` }
    }

    const response = await invoke<MemoryMutationResponse>('memory_mutate', {
      action,
      target,
      oldText: input.old_text ?? null,
      content: input.content ?? null,
      sourceSessionId: context.sessionId ?? null,
    })
    return {
      data: [
        response.success ? response.message : `Error: memory mutation rejected: ${response.message}`,
        `Usage: ${response.usageChars}/${response.limitChars} chars`,
        response.changed ? 'Persisted for future sessions.' : 'No stored entry changed.',
      ].join('\n'),
    }
  },
}

type SessionSearchRow = {
  session_id: string
  session_title: string
  started_at: string
  matched_content: string | null
  matched_role: string | null
}

const sessionSearchTool: Tool<{ query: string; limit?: number }> = {
  name: 'SessionSearch',
  aliases: ['session_search', 'search_sessions'],
  description: 'Searches persisted past conversations for exact details that are not in curated memory.',
  category: 'memory',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      query: { type: 'string', description: 'Full-text search query' },
      limit: { type: 'number', description: 'Maximum number of matches' },
    },
    required: ['query'],
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(input) {
    const query = input.query.trim()
    if (!query) return { data: 'Error: query is required.' }
    const rows = await invoke<SessionSearchRow[]>('session_search', {
      query,
      limit: Math.max(1, Math.min(50, input.limit ?? 10)),
    })
    if (rows.length === 0) return { data: `No past-session matches for "${query}".` }
    return {
      data: rows.map((row) => [
        `[${row.session_id}] ${row.session_title} (${row.started_at})`,
        row.matched_content ? `${row.matched_role ?? 'message'}: ${row.matched_content}` : '',
      ].filter(Boolean).join('\n')).join('\n\n'),
    }
  },
}

// ── PlanModeTool ───────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/EnterPlanModeTool/

const enterPlanTool: Tool = {
  name: 'EnterPlanMode',
  aliases: ['plan', 'enter_plan_mode'],
  description: 'Switches to plan mode. All changes are suggested only, not executed.',
  category: 'planning',
  riskLevel: 'low',
  inputSchema: { type: 'object', properties: {} },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(_input, context) {
    context.setAppState(prev => ({ ...prev, planMode: true }))
    return { data: 'Plan mode enabled. Changes will only be suggested.' }
  },
}

const exitPlanTool: Tool = {
  name: 'ExitPlanMode',
  aliases: ['execute', 'exit_plan_mode'],
  description: 'Leaves plan mode and returns to direct execution.',
  category: 'planning',
  riskLevel: 'low',
  inputSchema: { type: 'object', properties: {} },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  async call(_input, context) {
    context.setAppState(prev => ({ ...prev, planMode: false }))
    return { data: 'Plan mode disabled. Changes will be executed directly.' }
  },
}

// ── SkillTool ──────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/SkillTool/

const skillTool: Tool<{ skill_name: string; input: string }> = {
  name: 'Skill',
  aliases: ['skill', 'SkillTool', 'run_skill'],
  description: 'Runs a saved skill.',
  category: 'skill',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      skill_name: { type: 'string', description: 'Name of the skill' },
      input: { type: 'string', description: 'Input for the skill' },
    },
    required: ['skill_name', 'input'],
  },
  isReadOnly: () => false,
  isConcurrencySafe: () => false,
  async call(input) {
    // Delegate to the existing skill system
    return { data: `Skill "${input.skill_name}" executed mit: ${input.input}` }
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
  description: 'Creates DOCX/PPTX with native generation and optional template transform (modes: parallel, native, template).',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      format: { type: 'string', description: 'Target format', enum: ['docx', 'pptx'] },
      output_path: { type: 'string', description: 'Target file for native/template output' },
      mode: { type: 'string', description: 'Workflow mode', enum: ['parallel', 'native', 'template'], default: 'parallel' },
      template_path: { type: 'string', description: 'Optional template for placeholder transform' },
      transforms: { type: 'object', description: 'Key/value map for {{placeholder}} replacement' },
      title: { type: 'string', description: 'Optional title for native output and transform defaults' },
      paragraphs: { type: 'array', items: { type: 'string' }, description: 'Optional sections for native output (each entry = one paragraph / slide)' },
      bullets: { type: 'array', items: { type: 'string' }, description: 'Optional bullet list for native output' },
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
  description: 'Lists directory contents. Shows files with size and type.',
  category: 'filesystem',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Path to the directory' },
      recursive: { type: 'boolean', description: 'List recursively (default: false)' },
      max_depth: { type: 'number', description: 'Maximum recursion depth (default: 3)' },
      max_entries: { type: 'number', description: 'Optional: maximum number of entries (default: 200)' },
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
        return { data: 'directory ist leer.' }
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

      const header = `directory: ${result.rootPath} (${result.totalFiles} Files${result.truncated ? ', truncated' : ''})`
      return { data: `${header}\n${lines.join('\n')}` }
    } catch (err) {
      return { data: `Error beim Auflisten von "${input.path}": ${err instanceof Error ? err.message : String(err)}` }
    }
  },
}

// ── MultiEditTool ──────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/MultiEditTool/

const multiEditTool: Tool<{ file_path: string; edits: Array<{ old_string: string; new_string: string }> }> = {
  name: 'MultiEdit',
  aliases: ['multi_edit', 'batch_edit', 'MultiEditTool'],
  description: 'Runs multiple replacements in one file. Each old_string must be unique.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      file_path: { type: 'string', description: 'Path to the file to edit' },
      edits: {
        type: 'object',
        description: 'Array of {old_string, new_string} replacements',
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
        results.push(`Edit ${editCount + 1}: old_string not found`)
        continue
      }
      if (occurrences > 1) {
        results.push(`Edit ${editCount + 1}: old_string found ${occurrences} times (must be unique)`)
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

    return { data: `${editCount}/${edits.length} Edits executed in ${input.file_path}\n${results.join('\n')}` }
  },
}

// ── TaskUpdateTool ─────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/TaskTool/ (update)

const taskUpdateTool: Tool<{ task_id: string; status?: string; note?: string }> = {
  name: 'TaskUpdate',
  aliases: ['task_update', 'todo_update'],
  description: 'Updates task status.',
  category: 'task',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      task_id: { type: 'string', description: 'Task ID' },
      status: { type: 'string', description: 'New status', enum: ['pending', 'running', 'completed', 'failed'] },
      note: { type: 'string', description: 'Optional note/comment' },
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
      return { data: `Task ${input.task_id.slice(0, 8)} aktualisiert: ${input.status}${input.note ? ` — ${input.note}` : ''}` }
    } catch (err) {
      return { data: `Error beim Aktualisieren: ${err instanceof Error ? err.message : String(err)}` }
    }
  },
}

// ── FileAppendTool ─────────────────────────────────────────────────────────
// Additional utility tool

const fileAppendTool: Tool<{ file_path: string; content: string }> = {
  name: 'Append',
  aliases: ['append_file', 'file_append'],
  description: 'Appends content to the end of an existing file.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      file_path: { type: 'string', description: 'Path to the file' },
      content: { type: 'string', description: 'Content to append' },
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
      return { data: `${input.content.length} Zeichen an ${input.file_path} attached.` }
    } catch {
      // File doesn't exist — create it
      await invoke('fs_write_text_file', {
        path: fullPath,
        content: input.content,
        createBackup: false,
        runId: context.runId,
      })
      return { data: `New file created: ${input.file_path}` }
    }
  },
}

// ── DeleteFileTool ─────────────────────────────────────────────────────────
// Deletes a file with safety confirmation

const deleteFileTool: Tool<{ file_path: string; confirm: boolean }> = {
  name: 'DeleteFile',
  aliases: ['delete_file', 'remove_file', 'rm', 'DeleteFileTool'],
  description: 'Deletes a file. confirm must be set to true to confirm deletion.',
  category: 'filesystem',
  riskLevel: 'high',
  inputSchema: {
    type: 'object',
    properties: {
      file_path: { type: 'string', description: 'Path to the file to delete' },
      confirm: { type: 'boolean', description: 'Must be true to confirm deletion' },
    },
    required: ['file_path', 'confirm'],
  },
  isReadOnly: () => false,
  isDestructive: () => true,
  isConcurrencySafe: () => false,
  async call(input, context, onProgress) {
    if (!input.confirm) {
      return { data: 'Error: confirm must be set to true to delete the file.' }
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
        runId: context.runId,
      })
      return { data: `File geloescht: ${input.file_path}` }
    } catch (err) {
      return { data: `Error beim Delete: ${err instanceof Error ? err.message : String(err)}` }
    }
  },
}

// ── FileInfoTool ───────────────────────────────────────────────────────────
// Zeigt Metadaten einer File an

const fileInfoTool: Tool<{ path: string }> = {
  name: 'FileInfo',
  aliases: ['file_info', 'stat', 'FileInfoTool', 'file_metadata'],
  description: 'Shows file metadata: size, format, language, extension.',
  category: 'filesystem',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Path to the file or directory' },
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
          `File: ${file.fileName}`,
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
            `directory: ${result.rootPath}`,
            `Files insgesamt: ${result.totalFiles}`,
            `Files angezeigt: ${result.returnedFiles}`,
            result.truncated ? 'Anzeige gekuerzt.' : null,
          ].filter(Boolean).join('\n'),
        }
      }

      return { data: `Path not found: ${input.path}` }
    } catch (err) {
      return { data: `Error: ${err instanceof Error ? err.message : String(err)}` }
    }
  },
}

// ── RenameFileTool ─────────────────────────────────────────────────────────
// Renames a file or folder (wrapper around fs_move_path)

const renameFileTool: Tool<{ path: string; new_name: string }> = {
  name: 'RenameFile',
  aliases: ['rename_file', 'rename', 'RenameFileTool'],
  description: 'Renames a file or folder. Only the name changes; the location stays the same.',
  category: 'filesystem',
  riskLevel: 'medium',
  inputSchema: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Path to the file or folder to rename' },
      new_name: { type: 'string', description: 'New filename (without path, name only)' },
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
      return { data: `Error while renaming: ${err instanceof Error ? err.message : String(err)}` }
    }
  },
}

// ── ThinkTool ──────────────────────────────────────────────────────────────
// Mirrors: claude-code-main/src/tools/ThinkTool/

const thinkTool: Tool<{ thought: string }> = {
  name: 'Think',
  aliases: ['think', 'ThinkTool', 'reasoning'],
  description: 'Use this tool to think and plan before acting. Helps with complex multi-step tasks.',
  category: 'planning',
  riskLevel: 'low',
  inputSchema: {
    type: 'object',
    properties: {
      thought: { type: 'string', description: 'Your thought/plan/consideration' },
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
    desktopscreenshotTool,
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
    sessionSearchTool,
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
