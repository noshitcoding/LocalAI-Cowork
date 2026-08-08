import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { hasTauriRuntime } from "../utils/safeInvoke";
import { useTerminalStore } from "./terminalStore";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

vi.mock("../utils/safeInvoke", () => ({
  hasTauriRuntime: vi.fn(() => false),
  safeInvoke: vi.fn(
    async (_cmd: string, _args: unknown, fallback: unknown) => fallback,
  ),
  safeInvokeVoid: vi.fn(),
}));

function resetTerminalState() {
  useTerminalStore.setState({
    backends: [],
    loading: false,
    error: null,
    sessionsByThread: {},
    activeSessionIds: {},
    dockOpenByThread: {},
    dockHeightByThread: {},
    hiddenActivityByThread: {},
    activeAiThreadId: null,
  });
}

describe("terminalStore dock state", () => {
  beforeEach(() => {
    vi.useRealTimers();
    vi.mocked(invoke).mockReset();
    vi.mocked(hasTauriRuntime).mockReturnValue(false);
    resetTerminalState();
  });

  it("marks hidden activity for hidden sessions and clears it when the dock opens", async () => {
    await useTerminalStore.getState().createSession({
      threadId: "thread-1",
      cwd: "C:/repo",
      kind: "ai",
      hidden: true,
    });

    expect(useTerminalStore.getState().hiddenActivityByThread["thread-1"]).toBe(
      true,
    );

    useTerminalStore.getState().setDockOpen("thread-1", true);

    expect(useTerminalStore.getState().hiddenActivityByThread["thread-1"]).toBe(
      false,
    );
  });

  it("mirrors native sandbox chunks into a dedicated output-only session", () => {
    const session = useTerminalStore.getState().startSandboxCommand({
      threadId: "thread-sandbox",
      streamId: "stream-1",
      cwd: "C:/sandbox/workspace",
      command: "Write-Output Hallo",
    });
    useTerminalStore
      .getState()
      .appendSandboxChunk("stream-1", "stdout", "Hallo\r\n");
    useTerminalStore
      .getState()
      .appendSandboxChunk("stream-1", "stderr", "Warnung\r\n");
    useTerminalStore
      .getState()
      .finishSandboxCommand("stream-1", 0, "completed");

    const mirrored =
      useTerminalStore.getState().sessionsByThread["thread-sandbox"][0];
    expect(session).toMatchObject({ kind: "sandbox", title: "AI Sandbox" });
    expect(mirrored.output).toContain("Hallo");
    expect(mirrored.output).toContain("[stderr] Warnung");
    expect(mirrored).toMatchObject({ status: "idle" });
  });

  it("fails closed instead of creating a host PTY for an AI command", async () => {
    await expect(
      useTerminalStore.getState().runAiCommand({
        threadId: "thread-1",
        cwd: "C:/repo",
        command: "Get-Location",
        timeoutMs: 1000,
      }),
    ).rejects.toThrow("AI host-PTY routing is disabled");

    const sessions =
      useTerminalStore.getState().sessionsByThread["thread-1"] ?? [];
    expect(sessions).toHaveLength(0);
  });

  it("does not reuse the visible manual tab for AI commands while the dock is closed", async () => {
    await useTerminalStore.getState().createSession({
      threadId: "thread-1",
      cwd: "C:/repo",
      kind: "manual",
      hidden: false,
    });

    await expect(
      useTerminalStore.getState().runAiCommand({
        threadId: "thread-1",
        cwd: "C:/repo",
        command: "Get-Location",
        timeoutMs: 1000,
      }),
    ).rejects.toThrow("AI host-PTY routing is disabled");

    const sessions =
      useTerminalStore.getState().sessionsByThread["thread-1"] ?? [];

    expect(sessions).toHaveLength(1);
    expect(sessions[0]).toMatchObject({ kind: "manual", hidden: false });
  });

  it("never invokes terminal_create, terminal_write, or terminal_kill for AI commands", async () => {
    vi.mocked(hasTauriRuntime).mockReturnValue(true);
    vi.mocked(invoke).mockResolvedValue(undefined);

    await expect(
      useTerminalStore.getState().runAiCommand({
        threadId: "thread-timeout",
        cwd: "C:/repo",
        command: "Start-Sleep -Seconds 30",
        timeoutMs: 1000,
      }),
    ).rejects.toThrow("AI host-PTY routing is disabled");
    expect(invoke).not.toHaveBeenCalled();
  });
});
