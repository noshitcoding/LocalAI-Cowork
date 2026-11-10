import {describe, expect, it} from "vitest";
import {connectionEndpoint, nativeConnectionAllowed, preferNativeConnection, storedConnection} from "./connectionSettings";

describe("explicit V2 connections", () => {
  it("uses the current web origin for an empty address and normalizes HTTPS", () => {
    expect(connectionEndpoint(" ")).toBe("");
    expect(connectionEndpoint(" https://COWORK.example:443/ ")).toBe("https://cowork.example");
    expect(connectionEndpoint("https://cowork.example:8443")).toBe("https://cowork.example:8443");
  });
  it("allows cleartext only on exact loopback hosts", () => {
    for (const host of ["localhost", "127.0.0.1"]) expect(connectionEndpoint(`http://${host}:43821/`)).toBe(`http://${host}:43821`);
    for (const host of ["cowork.example", "127.0.0.2", "localhost.example", "192.168.1.2", "[::1]"]) expect(() => connectionEndpoint(`http://${host}`)).toThrow();
  });
  it("rejects credentials, fragments, queries, paths and other schemes", () => {
    for (const value of ["https://user:secret@cowork.example", "https://cowork.example/#token", "https://cowork.example/?key=x", "https://cowork.example/api", "file:///secret", "javascript:alert(1)", "//cowork.example", "cowork.example"]) expect(() => connectionEndpoint(value)).toThrow();
  });
  it("keeps native folder and key authority on the exact local daemon", () => {
    expect(nativeConnectionAllowed("http://127.0.0.1:43821", "native")).toBe(true);
    expect(nativeConnectionAllowed("http://127.0.0.1:43821", "server")).toBe(false);
    for (const endpoint of ["", "https://cowork.example", "http://127.0.0.1:43822", "http://localhost:43821"]) expect(nativeConnectionAllowed(endpoint, "native")).toBe(false);
  });
  it("restores the selected endpoint and token together", () => {
    const values = {"cowork-v2-token": "private", "cowork-v2-endpoint": "https://cowork.example/"};
    expect(storedConnection({getItem: key => values[key as keyof typeof values] ?? null})).toEqual({token: "private", base: "https://cowork.example", kind: "server"});
  });
  it("withholds the bearer when the persisted endpoint is invalid", () => {
    expect(storedConnection({getItem: key => key === "cowork-v2-token" ? "private" : "http://foreign.example"})).toEqual({token: "", base: "", kind: "server"});
  });
  it("does not replace an explicit loopback server selection with native startup", () => {
    const values = {"cowork-v2-token": "private", "cowork-v2-endpoint": "http://127.0.0.1:43821", "cowork-v2-connection-kind": "server"};
    const storage = {getItem: (key: string) => values[key as keyof typeof values] ?? null};
    expect(preferNativeConnection(storage)).toBe(false);
    const restored = storedConnection(storage);
    expect(nativeConnectionAllowed(restored.base, restored.kind)).toBe(false);
  });
  it("restores a local preference only after fresh native connection verification", () => {
    const values = {"cowork-v2-token": "private", "cowork-v2-endpoint": "http://127.0.0.1:43821", "cowork-v2-connection-kind": "native"};
    const storage = {getItem: (key: string) => values[key as keyof typeof values] ?? null};
    expect(preferNativeConnection(storage)).toBe(true);
    const restored = storedConnection(storage);
    expect(nativeConnectionAllowed(restored.base, restored.kind)).toBe(false);
    expect(preferNativeConnection({getItem: () => null})).toBe(true);
    expect(preferNativeConnection({getItem: key => key === "cowork-v2-endpoint" ? "https://cowork.example" : null})).toBe(false);
    expect(preferNativeConnection({getItem: () => "http://foreign.example"})).toBe(false);
  });
});
