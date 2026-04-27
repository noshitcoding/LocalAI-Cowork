import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useConfigStore } from '../../stores/configStore'

const invokeMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
}))

describe('MCPTool compatibility', () => {
  beforeEach(() => {
    invokeMock.mockReset()
    useConfigStore.setState({
      mcpServer: {
        name: 'test-mcp',
        command: 'node',
        args: 'server.js',
        env: {},
      },
      mcpServers: [
        {
          name: 'test-mcp',
          command: 'node',
          args: 'server.js',
          env: {},
        },
      ],
      activeMcpServerName: 'test-mcp',
    })
  })

  it('handles current backend MCP response contract', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue({
      serverName: 'test-mcp',
      toolName: 'search_docs',
      success: true,
      result: 'ok-current',
      error: null,
    })

    const tool = getAllTools().find((entry) => entry.name === 'MCPTool')
    expect(tool).toBeTruthy()

    const result = await tool!.call(
      {
        server_name: 'test-mcp',
        tool_name: 'search_docs',
        arguments: { query: 'hello' },
      },
      { runId: 'run-1' } as never,
    )

    expect(result.data).toBe('ok-current')
    expect(invokeMock).toHaveBeenCalledWith(
      'mcp_call_tool',
      expect.objectContaining({
        request: expect.objectContaining({
          name: 'test-mcp',
          command: 'node',
          toolName: 'search_docs',
          toolArgs: { query: 'hello' },
        }),
      }),
    )
  })

  it('falls back to legacy MCP envelope and response shape', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockImplementation((_cmd: string, payload: Record<string, unknown>) => {
      if (payload.request) {
        throw new Error('primary contract failed')
      }
      return Promise.resolve({
        content: 'ok-legacy',
        isError: false,
      })
    })

    const tool = getAllTools().find((entry) => entry.name === 'MCPTool')
    expect(tool).toBeTruthy()

    const result = await tool!.call(
      {
        server_name: 'test-mcp',
        tool_name: 'search_docs',
        arguments: { query: 'legacy' },
      },
      { runId: 'run-2' } as never,
    )

    expect(result.data).toBe('ok-legacy')
    expect(invokeMock).toHaveBeenCalledTimes(2)
    expect(invokeMock).toHaveBeenLastCalledWith(
      'mcp_call_tool',
      expect.objectContaining({
        mcpCallRequest: expect.objectContaining({
          name: 'test-mcp',
          toolName: 'search_docs',
          toolArgs: { query: 'legacy' },
        }),
        server_name: 'test-mcp',
        tool_name: 'search_docs',
      }),
    )
  })

  it('converts screenshot_for_display payload into attachment message', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue({
      serverName: 'test-mcp',
      toolName: 'screenshot_for_display',
      success: true,
      result: JSON.stringify({
        success: true,
        reused: false,
        displayIndex: 0,
        displayInfo: {
          width: 1280,
          height: 720,
          coordinateOverlay: true,
          coordinateGrid: {
            minorStepPx: 50,
            majorStepPx: 100,
            origin: 'top-left',
            coordinateSpace: 'display',
          },
        },
        coordinateOverlay: true,
        imageDataUrl: 'data:image/png;base64,QUJD',
      }),
      error: null,
    })

    const tool = getAllTools().find((entry) => entry.name === 'MCPTool')
    expect(tool).toBeTruthy()

    const result = await tool!.call(
      {
        server_name: 'test-mcp',
        tool_name: 'screenshot_for_display',
        arguments: {},
      },
      { runId: 'run-3' } as never,
    )

    expect(typeof result.data).toBe('string')
    expect(result.data).not.toContain('imageDataUrl')
    expect(result.data).toContain('Koordinatenhinweis')
    expect(result.data).toContain('50px')
    expect(result.newMessages).toBeTruthy()
    expect(result.newMessages?.[0]).toMatchObject({
      type: 'attachment',
      attachmentType: 'tool_result',
    })
  })

  it('captures desktop screenshots with the annotated screenshot command', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue({
      dataUrl: 'data:image/png;base64,QUJD',
      width: 1280,
      height: 720,
      x: 0,
      y: 0,
      primary: true,
      deviceName: '\\\\.\\DISPLAY1',
      coordinateOverlay: true,
      imageWidth: 1280,
      imageHeight: 720,
      scaleFactor: 1,
    })

    const tool = getAllTools().find((entry) => entry.name === 'DesktopScreenshot')
    expect(tool).toBeTruthy()

    const result = await tool!.call({}, { runId: 'run-4' } as never)

    expect(invokeMock).toHaveBeenCalledWith('desktop_capture_primary_annotated_screenshot')
    expect(result.data).toContain('Koordinatenraster')
    expect(result.newMessages?.[0]).toMatchObject({
      type: 'attachment',
      attachmentType: 'tool_result',
    })
  })
})
