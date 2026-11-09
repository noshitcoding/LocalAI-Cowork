import {describe, expect, it} from "vitest";
import {forgetWorkspace, loadWorkspaceProfiles, rememberWorkspace, updateWorkspace, workspaceId, workspaceToken, type WorkspaceProfile} from "./workspaceProfiles";
import type {ConnectionSettings} from "./connectionSettings";

function storage(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  return {values, getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {values.set(key, value);},
    removeItem: (key: string) => {values.delete(key);}};
}
const server = (base = "https://cowork.example", token = "server-secret"): ConnectionSettings => ({base, token, kind: "server"});
const native: ConnectionSettings = {base: "http://127.0.0.1:43821", token: "native-secret", kind: "native"};
const profile = (connection: ConnectionSettings, name = ""): WorkspaceProfile => ({id: workspaceId(connection), name, base: connection.base, kind: connection.kind});
const metadata = (profiles: unknown[]) => JSON.stringify({version: 1, profiles});

describe("saved workplaces", () => {
  it("normalizes server origins and keeps explicit loopback servers separate from native", () => {
    expect(workspaceId(server(" https://COWORK.example:443/ "))).toBe(workspaceId(server()));
    expect(workspaceId(server("http://127.0.0.1:43821"))).not.toBe(workspaceId(native));
    expect(workspaceId(server(""))).not.toBe(workspaceId(server()));
  });

  it("keeps each bearer in session storage and restores metadata without exposing it locally", () => {
    const local = storage(), session = storage();
    let profiles = rememberWorkspace(local, session, [], server(), "Work server");
    profiles = rememberWorkspace(local, session, profiles, native);
    expect(loadWorkspaceProfiles(local, session)).toEqual(profiles);
    expect(workspaceToken(session, profiles[0])).toBe("server-secret");
    expect(workspaceToken(session, profiles[1])).toBe("native-secret");
    expect([...local.values.values()].join("")).not.toContain("secret");
    expect(loadWorkspaceProfiles(local, storage())).toEqual(profiles);
    expect(workspaceToken(storage(), profiles[0])).toBe("");
  });

  it("updates one normalized workplace, preserves its name for blank input and replaces its bearer", () => {
    const local = storage(), session = storage();
    let profiles = rememberWorkspace(local, session, [], server(), " Main server ");
    profiles = rememberWorkspace(local, session, profiles, server("https://COWORK.example:443/", "renewed"), "  ");
    expect(profiles).toEqual([profile(server(), "Main server")]);
    expect(workspaceToken(session, profiles[0])).toBe("renewed");
    profiles = rememberWorkspace(local, session, profiles, server(), "Renamed");
    expect(profiles[0].name).toBe("Renamed");
  });

  it("migrates a legacy session only with its matching endpoint and recorded connection kind", () => {
    const local = storage(), session = storage({"cowork-v2-token": "legacy", "cowork-v2-endpoint": "https://COWORK.example:443/"});
    const profiles = loadWorkspaceProfiles(local, session);
    expect(profiles).toEqual([profile(server())]);
    expect(workspaceToken(session, profiles[0])).toBe("legacy");
    expect(workspaceToken(session, profile(server("https://other.example")))).toBe("");
    expect(local.values.size).toBe(0);
  });

  it("does not turn an explicit or older loopback server session into native authority", () => {
    for (const kind of [undefined, "server"]) {
      const session = storage({"cowork-v2-token": "legacy", "cowork-v2-endpoint": native.base,
        ...(kind ? {"cowork-v2-connection-kind": kind} : {})});
      expect(loadWorkspaceProfiles(storage(), session)).toEqual([profile(server(native.base))]);
      expect(workspaceToken(session, profile(native))).toBe("");
      expect(workspaceToken(session, profile(server(native.base)))).toBe("legacy");
    }
    const session = storage({"cowork-v2-token": "legacy", "cowork-v2-endpoint": native.base, "cowork-v2-connection-kind": "native"});
    expect(loadWorkspaceProfiles(storage(), session)).toEqual([profile(native)]);
    expect(workspaceToken(session, profile(server(native.base)))).toBe("");
  });

  it("keeps saved names when migrating the currently selected session and removes duplicate metadata", () => {
    const saved = profile(server(), "Existing name");
    const local = storage({"cowork-v2-workspaces": metadata([saved, {...saved, name: "Duplicate"}])});
    const session = storage({"cowork-v2-token": "legacy", "cowork-v2-endpoint": server().base});
    expect(loadWorkspaceProfiles(local, session)).toEqual([saved]);
  });

  it("ignores damaged metadata and unsafe legacy endpoints without yielding a bearer", () => {
    for (const value of ["broken JSON", "null", "[]", JSON.stringify({version: 2, profiles: [profile(server())]})]) {
      expect(loadWorkspaceProfiles(storage({"cowork-v2-workspaces": value}), storage())).toEqual([]);
    }
    for (const base of ["http://foreign.example", "https://cowork.example/api", "https://user:secret@cowork.example"]) {
      const session = storage({"cowork-v2-token": "legacy", "cowork-v2-endpoint": base});
      expect(loadWorkspaceProfiles(storage(), session)).toEqual([]);
      expect(workspaceToken(session, profile(server()))).toBe("");
    }
  });

  it("rejects altered endpoint bindings even when the old session token still exists", () => {
    const local = storage(), session = storage();
    const [saved] = rememberWorkspace(local, session, [], server(), "Original");
    const altered = {...saved, base: "https://foreign.example"};
    local.setItem("cowork-v2-workspaces", metadata([altered]));
    expect(loadWorkspaceProfiles(local, session)).toEqual([]);
    expect(workspaceToken(session, altered)).toBe("");
    expect(workspaceToken(session, {...altered, id: workspaceId(altered)})).toBe("");
    expect(workspaceToken(session, saved)).toBe("server-secret");
  });

  it("filters malformed entries independently and cannot load native profiles for other daemons", () => {
    const saved = profile(server());
    const entries = [null, {}, {...saved, name: 12}, {...saved, kind: "unknown"},
      {...profile(native), base: "https://foreign.example"}, saved];
    expect(loadWorkspaceProfiles(storage({"cowork-v2-workspaces": metadata(entries)}), storage())).toEqual([saved]);
    expect(() => workspaceId({...native, base: "http://127.0.0.1:43822"})).toThrow("invalidServerAddress");
    expect(() => rememberWorkspace(storage(), storage(), [], {...native, base: "https://foreign.example"})).toThrow("invalidServerAddress");
  });

  it("does not serialize extra properties from callers into persistent metadata", () => {
    const local = storage(), session = storage();
    rememberWorkspace(local, session, [{...profile(server()), token: "must-not-persist"} as WorkspaceProfile], native);
    expect([...local.values.values()].join("")).not.toContain("must-not-persist");
  });

  it("edits a workplace in place and moves its bearer only with a new token for a new address", () => {
    const local = storage(), session = storage();
    let profiles = rememberWorkspace(local, session, [], server(), "Main");
    profiles = rememberWorkspace(local, session, profiles, server("https://second.example", "second-secret"), "Second");
    profiles = updateWorkspace(local, session, profiles, profiles[0], {name: " Renamed ", base: "https://COWORK.example:443/"});
    expect(profiles).toEqual([profile(server(), "Renamed"), profile(server("https://second.example"), "Second")]);
    expect(workspaceToken(session, profiles[0])).toBe("server-secret");
    expect(() => updateWorkspace(local, session, profiles, profiles[0], {name: "Moved", base: "https://moved.example"})).toThrow("newTokenRequired");
    profiles = updateWorkspace(local, session, profiles, profiles[0], {name: "Moved", base: "https://moved.example", token: "moved-secret"});
    expect(profiles).toEqual([profile(server("https://moved.example"), "Moved"), profile(server("https://second.example"), "Second")]);
    expect(workspaceToken(session, profiles[0])).toBe("moved-secret");
    expect(workspaceToken(session, profile(server()))).toBe("");
    expect(loadWorkspaceProfiles(local, session)).toEqual(profiles);
  });

  it("merges an edited address into an existing workplace with the same address", () => {
    const local = storage(), session = storage();
    let profiles = rememberWorkspace(local, session, [], server(), "Main");
    profiles = rememberWorkspace(local, session, profiles, server("https://second.example", "second-secret"), "Second");
    profiles = updateWorkspace(local, session, profiles, profiles[1], {name: "Merged", base: server().base, token: "merged-secret"});
    expect(profiles).toEqual([profile(server(), "Merged")]);
    expect(workspaceToken(session, profiles[0])).toBe("merged-secret");
  });

  it("names the built-in local workplace without saving it beforehand", () => {
    const local = storage(), session = storage();
    const profiles = updateWorkspace(local, session, [], profile(native), {name: "Laptop", base: native.base});
    expect(profiles).toEqual([profile(native, "Laptop")]);
    expect(session.values.size).toBe(0);
  });

  it("forgets a workplace together with its bearer and the selected session that pointed to it", () => {
    const local = storage(), session = storage({"cowork-v2-token": "server-secret", "cowork-v2-endpoint": server().base, "cowork-v2-connection-kind": "server"});
    let profiles = rememberWorkspace(local, session, [], server(), "Main");
    profiles = rememberWorkspace(local, session, profiles, native);
    profiles = forgetWorkspace(local, session, profiles, workspaceId(server()));
    expect(profiles).toEqual([profile(native)]);
    expect(loadWorkspaceProfiles(local, session)).toEqual([profile(native)]);
    expect([...session.values.keys()]).toEqual(["cowork-v2-workspace-token:" + workspaceId(native)]);
  });

  it("keeps the selected session when another workplace is forgotten", () => {
    const local = storage(), session = storage({"cowork-v2-token": "native-secret", "cowork-v2-endpoint": native.base, "cowork-v2-connection-kind": "native"});
    let profiles = rememberWorkspace(local, session, [], server(), "Main");
    profiles = forgetWorkspace(local, session, profiles, profiles[0].id);
    expect(profiles).toEqual([]);
    expect(session.values.get("cowork-v2-token")).toBe("native-secret");
    expect(loadWorkspaceProfiles(local, session)).toEqual([profile(native)]);
  });

  it("continues without profiles when browser storage is unavailable", () => {
    const unavailable = {getItem: () => {throw new Error("storage blocked");}};
    expect(loadWorkspaceProfiles(unavailable, unavailable)).toEqual([]);
    expect(workspaceToken(unavailable, profile(server()))).toBe("");
  });
});
