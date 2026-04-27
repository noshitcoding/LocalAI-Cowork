import { safeInvoke } from '../../utils/safeInvoke'
import { useConfigStore } from '../../stores/configStore'

export type ComputerUseActionLog = {
  step: number
  actionType: string
  summary: string
  safetyChecks: string[]
}

export type ComputerUseRunOptions = {
  goal: string
  appPath?: string
  appArgs?: string[]
  cwd?: string
  windowTitle?: string
  processName?: string
  processId?: number
  exactMatch?: boolean
  maxSteps?: number
  actionDelayMs?: number
  launchDelayMs?: number
  autoAcknowledgeSafetyChecks?: boolean
}

export type ComputerUseRunResult = {
  status: 'completed' | 'max_steps' | 'blocked'
  stepsExecuted: number
  finalText: string
  responseId: string | null
  windowTarget: {
    windowTitle?: string
    processName?: string
    processId?: number
  }
  launch?: {
    pid: number
    path: string
    args: string[]
  }
  actions: ComputerUseActionLog[]
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

type DesktopScreenshotResponse = {
  dataUrl: string
  width: number
  height: number
  x: number
  y: number
  primary: boolean
  deviceName: string
  scaleFactor?: number
  imageWidth?: number
  imageHeight?: number
  coordinateOverlay?: boolean
}

function resolveDisplayRelativePoint(
  display: DesktopDisplayInfo,
  x: number,
  y: number,
  inputWidth = display.width,
  inputHeight = display.height,
): { x: number; y: number } {
  const scaleX = inputWidth > 0 ? display.width / inputWidth : 1
  const scaleY = inputHeight > 0 ? display.height / inputHeight : 1
  return {
    x: Math.round(x * scaleX) + display.x,
    y: Math.round(y * scaleY) + display.y,
  }
}

function displayFromScreenshot(screenshot: DesktopScreenshotResponse): DesktopDisplayInfo {
  return {
    primary: screenshot.primary,
    x: screenshot.x,
    y: screenshot.y,
    width: screenshot.width,
    height: screenshot.height,
    deviceName: screenshot.deviceName,
    scaleFactor: screenshot.scaleFactor,
  }
}

function screenshotInputSize(screenshot: DesktopScreenshotResponse): { width: number; height: number } {
  return {
    width: screenshot.imageWidth ?? screenshot.width,
    height: screenshot.imageHeight ?? screenshot.height,
  }
}

type DesktopLaunchResponse = {
  pid: number
  path: string
  args: string[]
}

type PendingSafetyCheck = {
  id: string
  code: string
  message: string
}

type ComputerAction =
  | { type: 'click'; x: number; y: number; button?: 'left' | 'right'; double_click?: boolean }
  | { type: 'scroll'; x?: number; y?: number; scroll_x?: number; scroll_y?: number }
  | { type: 'keypress'; keys: string[] }
  | { type: 'type'; text: string }
  | { type: 'wait' }
  | { type: 'screenshot' }
  | { type: string; [key: string]: unknown }

type ResponseTextPart = {
  type?: string
  text?: string
}

type ResponseOutputItem = {
  type: string
  text?: string
  content?: ResponseTextPart[]
  summary?: ResponseTextPart[]
  call_id?: string
  action?: ComputerAction
  pending_safety_checks?: PendingSafetyCheck[]
}

type OpenAIResponse = {
  id: string
  output: ResponseOutputItem[]
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms))
}

function extractResponseText(response: OpenAIResponse): string {
  const chunks: string[] = []

  for (const item of response.output) {
    if (item.type === 'output_text' && typeof item.text === 'string') {
      chunks.push(item.text)
      continue
    }

    if (Array.isArray(item.content)) {
      for (const part of item.content) {
        if (part.type === 'output_text' || part.type === 'text') {
          if (typeof part.text === 'string' && part.text.trim()) {
            chunks.push(part.text)
          }
        }
      }
    }
  }

  return chunks.join('\n').trim()
}

function extractReasoningSummary(response: OpenAIResponse): string {
  for (const item of response.output) {
    if (item.type === 'reasoning' && Array.isArray(item.summary)) {
      const summary = item.summary
        .map((entry: ResponseTextPart) => entry.text?.trim())
        .filter((entry: string | undefined): entry is string => Boolean(entry))
        .join(' ')
      if (summary) return summary
    }
  }

  return ''
}

function getComputerCall(response: OpenAIResponse) {
  return response.output.find(
    (item): item is ResponseOutputItem & { type: 'computer_call'; call_id: string; action: ComputerAction } =>
      item.type === 'computer_call' && typeof item.call_id === 'string' && typeof item.action === 'object' && item.action !== null,
  )
}

function normalizeSafetyChecks(
  checks: PendingSafetyCheck[] | undefined,
  autoAcknowledge: boolean,
) {
  const safetyChecks = checks ?? []
  if (safetyChecks.length === 0) return { safetyChecks, acknowledged: undefined }

  if (!autoAcknowledge) {
    const message = safetyChecks
      .map((check) => `${check.code}: ${check.message}`)
      .join('\n')
    throw new Error(`computer-use safety check requires acknowledgement:\n${message}`)
  }

  return {
    safetyChecks,
    acknowledged: safetyChecks.map((check) => ({
      id: check.id,
      code: check.code,
      message: check.message,
    })),
  }
}

async function executeComputerAction(
  action: ComputerAction,
  display: DesktopDisplayInfo,
  inputSize: { width: number; height: number },
): Promise<void> {
  switch (action.type) {
    case 'click': {
      const clickAction = action as Extract<ComputerAction, { type: 'click' }>
      const point = resolveDisplayRelativePoint(display, clickAction.x, clickAction.y, inputSize.width, inputSize.height)
      await safeInvoke('desktop_click', {
        request: {
          x: point.x,
          y: point.y,
          button: clickAction.button ?? 'left',
          doubleClick: Boolean(clickAction.double_click),
        },
      })
      return
    }
    case 'scroll': {
      const scrollAction = action as Extract<ComputerAction, { type: 'scroll' }>
      const point = typeof scrollAction.x === 'number' && typeof scrollAction.y === 'number'
        ? resolveDisplayRelativePoint(display, scrollAction.x, scrollAction.y, inputSize.width, inputSize.height)
        : undefined
      await safeInvoke('desktop_scroll', {
        request: {
          x: point?.x,
          y: point?.y,
          scrollY: Math.round(scrollAction.scroll_y ?? 0),
        },
      })
      return
    }
    case 'keypress': {
      const keypressAction = action as Extract<ComputerAction, { type: 'keypress' }>
      await safeInvoke('desktop_keypress', {
        request: {
          keys: keypressAction.keys,
        },
      })
      return
    }
    case 'type': {
      const typeAction = action as Extract<ComputerAction, { type: 'type' }>
      await safeInvoke('desktop_type_text', {
        request: {
          text: typeAction.text,
        },
      })
      return
    }
    case 'wait':
    case 'screenshot':
      return
    default:
      throw new Error(`unsupported computer action: ${action.type}`)
  }
}

async function createResponse(
  apiKey: string,
  baseUrl: string,
  body: Record<string, unknown>,
): Promise<OpenAIResponse> {
  const response = await fetch(`${baseUrl.replace(/\/$/, '')}/responses`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify(body),
  })

  if (!response.ok) {
    throw new Error(`OpenAI responses.create failed (${response.status}): ${await response.text()}`)
  }

  return response.json() as Promise<OpenAIResponse>
}

function buildInstructions(goal: string, target: ComputerUseRunResult['windowTarget']): string {
  const targetHints = [
    target.windowTitle ? `Target window title: ${target.windowTitle}` : null,
    target.processName ? `Target process name: ${target.processName}` : null,
    typeof target.processId === 'number' ? `Target process id: ${target.processId}` : null,
  ]
    .filter((value): value is string => Boolean(value))
    .join('\n')

  return [
    'You are testing a Windows desktop application through screenshots and UI control.',
    'Stay inside the target application window and verify visible outcomes after each meaningful interaction.',
    'Be systematic, cautious, and avoid destructive actions that are not required by the goal.',
    'When you finish, provide a concise test report with passed checks, failed or blocked checks, and any reproduction notes.',
    targetHints,
    `Goal: ${goal}`,
  ]
    .filter(Boolean)
    .join('\n')
}

export async function runComputerUseAppTest(options: ComputerUseRunOptions): Promise<ComputerUseRunResult> {
  const {
    openAIComputerUse,
  } = useConfigStore.getState()

  if (!openAIComputerUse.apiKey.trim()) {
    throw new Error('OpenAI API key for Computer Use is not configured.')
  }

  const autoAcknowledge = options.autoAcknowledgeSafetyChecks ?? openAIComputerUse.autoAcknowledgeSafetyChecks
  const maxSteps = options.maxSteps ?? openAIComputerUse.maxSteps
  const actionDelayMs = options.actionDelayMs ?? openAIComputerUse.actionDelayMs
  const launchDelayMs = options.launchDelayMs ?? openAIComputerUse.launchDelayMs

  const windowTarget: ComputerUseRunResult['windowTarget'] = {
    windowTitle: options.windowTitle,
    processName: options.processName,
    processId: options.processId,
  }

  let launch: DesktopLaunchResponse | undefined
  if (options.appPath) {
    launch = await safeInvoke<DesktopLaunchResponse>('desktop_launch_app', {
      request: {
        path: options.appPath,
        args: options.appArgs ?? [],
        cwd: options.cwd,
        initialDelayMs: launchDelayMs,
      },
    })

    if (!windowTarget.processId) {
      windowTarget.processId = launch.pid
    }
  }

  if (windowTarget.windowTitle || windowTarget.processName || typeof windowTarget.processId === 'number') {
    await safeInvoke('desktop_focus_window', {
      request: {
        title: windowTarget.windowTitle,
        processName: windowTarget.processName,
        processId: windowTarget.processId,
        exactMatch: options.exactMatch ?? false,
      },
    })
  }

  let screenshot = await safeInvoke<DesktopScreenshotResponse>('desktop_capture_primary_screenshot')
  let actionDisplay = displayFromScreenshot(screenshot)
  let actionInputSize = screenshotInputSize(screenshot)

  let response = await createResponse(openAIComputerUse.apiKey, openAIComputerUse.baseUrl, {
    model: openAIComputerUse.model,
    truncation: 'auto',
    reasoning: { summary: 'concise' },
    tools: [{
      type: 'computer_use_preview',
      display_width: actionInputSize.width,
      display_height: actionInputSize.height,
      environment: 'windows',
    }],
    input: [{
      role: 'user',
      content: [
        { type: 'input_text', text: buildInstructions(options.goal, windowTarget) },
        { type: 'input_image', image_url: screenshot.dataUrl },
      ],
    }],
  })

  const actions: ComputerUseActionLog[] = []
  let stepsExecuted = 0

  while (stepsExecuted < maxSteps) {
    const computerCall = getComputerCall(response)
    if (!computerCall) {
      return {
        status: 'completed',
        stepsExecuted,
        finalText: extractResponseText(response),
        responseId: response.id,
        windowTarget,
        launch,
        actions,
      }
    }

    const reasoningSummary = extractReasoningSummary(response)
    const { safetyChecks, acknowledged } = normalizeSafetyChecks(
      computerCall.pending_safety_checks,
      autoAcknowledge,
    )

    await executeComputerAction(computerCall.action, actionDisplay, actionInputSize)
    await sleep(actionDelayMs)
    screenshot = await safeInvoke<DesktopScreenshotResponse>('desktop_capture_primary_screenshot')
    actionDisplay = displayFromScreenshot(screenshot)
    actionInputSize = screenshotInputSize(screenshot)

    stepsExecuted += 1
    actions.push({
      step: stepsExecuted,
      actionType: computerCall.action.type,
      summary: reasoningSummary || `Executed ${computerCall.action.type}`,
      safetyChecks: safetyChecks.map((check) => check.code),
    })

    response = await createResponse(openAIComputerUse.apiKey, openAIComputerUse.baseUrl, {
      model: openAIComputerUse.model,
      previous_response_id: response.id,
      truncation: 'auto',
      tools: [{
        type: 'computer_use_preview',
        display_width: actionInputSize.width,
        display_height: actionInputSize.height,
        environment: 'windows',
      }],
      input: [{
        type: 'computer_call_output',
        call_id: computerCall.call_id,
        acknowledged_safety_checks: acknowledged,
        output: {
          type: 'input_image',
          image_url: screenshot.dataUrl,
        },
      }],
    })
  }

  return {
    status: 'max_steps',
    stepsExecuted,
    finalText: extractResponseText(response),
    responseId: response.id,
    windowTarget,
    launch,
    actions,
  }
}
