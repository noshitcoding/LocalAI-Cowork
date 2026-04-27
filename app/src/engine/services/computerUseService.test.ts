import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useConfigStore } from '../../stores/configStore'

const invokeMock = vi.fn()
const fetchMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

describe('computerUseService', () => {
  beforeEach(() => {
    invokeMock.mockReset()
    fetchMock.mockReset()

    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      value: { invoke: vi.fn() },
      configurable: true,
      writable: true,
    })

    useConfigStore.setState({
      openAIComputerUse: {
        apiKey: 'sk-test',
        baseUrl: 'https://api.openai.com/v1',
        model: 'computer-use-preview',
        maxSteps: 5,
        actionDelayMs: 0,
        launchDelayMs: 0,
        autoAcknowledgeSafetyChecks: true,
      },
    })

    vi.stubGlobal('fetch', fetchMock)
  })

  it('runs a single computer-use action loop and returns the final report', async () => {
    const screenshots = [
      {
        dataUrl: 'data:image/png;base64,AAA',
        width: 1024,
        height: 768,
        x: 1920,
        y: 0,
        primary: true,
        deviceName: '\\\\.\\DISPLAY1',
      },
      {
        dataUrl: 'data:image/png;base64,BBB',
        width: 1024,
        height: 768,
        x: 1920,
        y: 0,
        primary: true,
        deviceName: '\\\\.\\DISPLAY1',
      },
    ]

    invokeMock.mockImplementation((command: string, payload?: Record<string, unknown>) => {
      switch (command) {
        case 'desktop_primary_display':
          return Promise.resolve({
            primary: true,
            x: 1920,
            y: 0,
            width: 1024,
            height: 768,
            deviceName: '\\\\.\\DISPLAY1',
          })
        case 'desktop_focus_window':
          return Promise.resolve({
            title: 'Example App',
            processId: 42,
            processName: 'example',
            handle: '0x100',
            x: 0,
            y: 0,
            width: 1024,
            height: 768,
            isForeground: true,
          })
        case 'desktop_capture_primary_screenshot':
          return Promise.resolve(screenshots.shift())
        case 'desktop_click':
          expect(payload).toEqual({
            request: {
              x: 2120,
              y: 120,
              button: 'left',
              doubleClick: false,
            },
          })
          return Promise.resolve({ ok: true, action: 'click' })
        default:
          throw new Error(`unexpected invoke command ${command}`)
      }
    })

    fetchMock
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({
          id: 'resp_1',
          output: [
            {
              type: 'reasoning',
              summary: [{ text: 'Opening the first visible control.' }],
            },
            {
              type: 'computer_call',
              call_id: 'call_1',
              action: { type: 'click', x: 200, y: 120, button: 'left' },
              pending_safety_checks: [],
            },
          ],
        }),
      })
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({
          id: 'resp_2',
          output: [
            {
              type: 'output_text',
              text: 'Passed: opened the expected screen.',
            },
          ],
        }),
      })

    const { runComputerUseAppTest } = await import('./computerUseService')
    const result = await runComputerUseAppTest({
      goal: 'Open the main workflow and verify the first screen.',
      windowTitle: 'Example App',
      actionDelayMs: 0,
      maxSteps: 3,
    })

    expect(fetchMock).toHaveBeenCalledTimes(2)
    expect(result.status).toBe('completed')
    expect(result.stepsExecuted).toBe(1)
    expect(result.finalText).toContain('Passed')
    expect(result.actions).toEqual([
      {
        step: 1,
        actionType: 'click',
        summary: 'Opening the first visible control.',
        safetyChecks: [],
      },
    ])

    const firstRequest = JSON.parse(String(fetchMock.mock.calls[0][1].body))
    expect(firstRequest.tools[0]).toMatchObject({
      type: 'computer_use_preview',
      environment: 'windows',
      display_width: 1024,
      display_height: 768,
    })

    const secondRequest = JSON.parse(String(fetchMock.mock.calls[1][1].body))
    expect(secondRequest.previous_response_id).toBe('resp_1')
    expect(secondRequest.input[0]).toMatchObject({
      type: 'computer_call_output',
      call_id: 'call_1',
    })
  })
})
