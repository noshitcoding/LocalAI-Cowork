import {connectionEndpoint, nativeConnectionAllowed, type ConnectionSettings} from "./connectionSettings";

export type WorkspaceProfile = {
  id: string;
  name: string;
  base: string;
  kind: ConnectionSettings["kind"];
};

const profilesKey = "cowork-v2-workspaces";
const tokenKey = (id: string) => "cowork-v2-workspace-token:" + id;

function normalizedConnection(value: {base: unknown; kind: unknown}): Pick<ConnectionSettings, "base" | "kind"> {
  if (typeof value.base !== "string" || (value.kind !== "native" && value.kind !== "server")) {
    throw new TypeError("invalidServerAddress");
  }
  const base = connectionEndpoint(value.base);
  if (value.kind === "native" && !nativeConnectionAllowed(base, value.kind)) {
    throw new TypeError("invalidServerAddress");
  }
  return {base, kind: value.kind};
}

export function workspaceId(connection: Pick<ConnectionSettings, "base" | "kind">): string {
  const {base, kind} = normalizedConnection(connection);
  return JSON.stringify([kind, base]);
}

function readProfile(value: unknown): WorkspaceProfile | undefined {
  if (!value || typeof value !== "object") return;
  const candidate = value as Record<string, unknown>;
  if (typeof candidate.id !== "string" || typeof candidate.name !== "string") return;
  try {
    const connection = normalizedConnection({base: candidate.base, kind: candidate.kind});
    const id = workspaceId(connection);
    // A changed address must never inherit the token belonging to its old ID.
    if (candidate.id !== id) return;
    return {id, name: candidate.name.trim(), ...connection};
  } catch { return; }
}

function selectedConnection(session: Pick<Storage, "getItem">): ConnectionSettings | undefined {
  try {
    const token = session.getItem("cowork-v2-token") ?? "";
    if (!token.trim()) return;
    const connection = normalizedConnection({
      base: session.getItem("cowork-v2-endpoint") ?? "",
      // Older sessions did not record a kind and cannot establish native authority.
      kind: session.getItem("cowork-v2-connection-kind") ?? "server",
    });
    return {token, ...connection};
  } catch { return; }
}

export function loadWorkspaceProfiles(local: Pick<Storage, "getItem">, session: Pick<Storage, "getItem">): WorkspaceProfile[] {
  const profiles: WorkspaceProfile[] = [];
  try {
    const saved: unknown = JSON.parse(local.getItem(profilesKey) ?? "null");
    if (saved && typeof saved === "object" && "version" in saved && saved.version === 1
      && "profiles" in saved && Array.isArray(saved.profiles)) {
      for (const value of saved.profiles) {
        const profile = readProfile(value);
        if (profile && !profiles.some(existing => existing.id === profile.id)) profiles.push(profile);
      }
    }
  } catch { /* A damaged or unavailable profile list must not prevent connecting. */ }
  const selected = selectedConnection(session);
  if (selected) {
    const id = workspaceId(selected);
    if (!profiles.some(profile => profile.id === id)) {
      profiles.push({id, name: "", base: selected.base, kind: selected.kind});
    }
  }
  return profiles;
}

export function rememberWorkspace(
  local: Pick<Storage, "setItem">,
  session: Pick<Storage, "setItem">,
  profiles: WorkspaceProfile[],
  next: ConnectionSettings,
  name = "",
): WorkspaceProfile[] {
  const connection = normalizedConnection(next), id = workspaceId(connection);
  const remembered = savedProfiles(profiles);
  const index = remembered.findIndex(profile => profile.id === id);
  const profile: WorkspaceProfile = {id, name: name.trim() || remembered[index]?.name || "", ...connection};
  if (index < 0) remembered.push(profile);
  else remembered[index] = profile;
  session.setItem(tokenKey(id), next.token);
  // Serialize only explicit metadata fields; bearer tokens stay in this browser session.
  local.setItem(profilesKey, JSON.stringify({version: 1, profiles: remembered}));
  return remembered;
}

function savedProfiles(profiles: WorkspaceProfile[], except: string[] = []): WorkspaceProfile[] {
  const remembered: WorkspaceProfile[] = [];
  for (const value of profiles) {
    const profile = readProfile(value);
    if (profile && !except.includes(profile.id) && !remembered.some(existing => existing.id === profile.id)) remembered.push(profile);
  }
  return remembered;
}

export function updateWorkspace(
  local: Pick<Storage, "setItem">,
  session: Pick<Storage, "setItem" | "removeItem">,
  profiles: WorkspaceProfile[],
  previous: WorkspaceProfile,
  next: {name: string; base: string; token?: string},
): WorkspaceProfile[] {
  const connection = normalizedConnection({base: next.base, kind: previous.kind}), id = workspaceId(connection);
  const moved = id !== previous.id;
  // A changed address must never inherit the token belonging to its old ID.
  if (moved && !next.token) throw new TypeError("newTokenRequired");
  const saved = savedProfiles(profiles, moved ? [id] : []);
  const position = saved.findIndex(profile => profile.id === previous.id);
  const remembered = saved.filter(profile => profile.id !== previous.id);
  remembered.splice(position < 0 ? remembered.length : position, 0, {id, name: next.name.trim(), ...connection});
  if (moved) session.removeItem(tokenKey(previous.id));
  if (next.token) session.setItem(tokenKey(id), next.token);
  local.setItem(profilesKey, JSON.stringify({version: 1, profiles: remembered}));
  return remembered;
}

export function forgetWorkspace(
  local: Pick<Storage, "setItem">,
  session: Pick<Storage, "getItem" | "removeItem">,
  profiles: WorkspaceProfile[],
  id: string,
): WorkspaceProfile[] {
  const remembered = savedProfiles(profiles, [id]);
  session.removeItem(tokenKey(id));
  const selected = selectedConnection(session);
  // Otherwise loading the profiles would bring the removed connection back from the selected session.
  if (selected && workspaceId(selected) === id) {
    for (const key of ["cowork-v2-token", "cowork-v2-endpoint", "cowork-v2-connection-kind"]) session.removeItem(key);
  }
  local.setItem(profilesKey, JSON.stringify({version: 1, profiles: remembered}));
  return remembered;
}

export function workspaceToken(session: Pick<Storage, "getItem">, profile: WorkspaceProfile): string {
  const validated = readProfile(profile);
  if (!validated) return "";
  try {
    const saved = session.getItem(tokenKey(validated.id));
    if (saved !== null) return saved;
    const selected = selectedConnection(session);
    return selected && selected.base === validated.base && selected.kind === validated.kind ? selected.token : "";
  } catch { return ""; }
}
