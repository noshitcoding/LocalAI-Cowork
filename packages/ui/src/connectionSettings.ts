export function connectionEndpoint(value: string): string {
  const raw = value.trim();
  if (!raw) return "";
  const url = new URL(raw);
  const loopback = ["localhost", "127.0.0.1"].includes(url.hostname);
  if ((url.protocol !== "https:" && !(url.protocol === "http:" && loopback))
      || url.username || url.password || url.search || url.hash
      || url.pathname !== "/") throw new TypeError("invalidServerAddress");
  return url.origin;
}

export type ConnectionSettings = {token: string; base: string; kind: "native" | "server"};

export function nativeConnectionAllowed(endpoint: string, kind: ConnectionSettings["kind"]): boolean {
  return kind === "native" && endpoint === "http://127.0.0.1:43821";
}

export function preferNativeConnection(storage: Pick<Storage, "getItem">): boolean {
  if (storage.getItem("cowork-v2-connection-kind") === "server") return false;
  try {
    const endpoint = connectionEndpoint(storage.getItem("cowork-v2-endpoint") ?? "");
    return !endpoint || nativeConnectionAllowed(endpoint, "native");
  } catch { return false; }
}

export function storedConnection(storage: Pick<Storage, "getItem">): ConnectionSettings {
  try {
    return {token: storage.getItem("cowork-v2-token") ?? "",
      base: connectionEndpoint(storage.getItem("cowork-v2-endpoint") ?? ""), kind: "server"};
  } catch {
    // Invalid persisted addresses must never receive the saved bearer.
    return {token: "", base: "", kind: "server"};
  }
}
