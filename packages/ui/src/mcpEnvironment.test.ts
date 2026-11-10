import {describe, expect, it} from "vitest";
import {parseMcpEnvironment} from "./mcpBridge";
describe("MCP environment input", () => {
  it("preserves explicit strings, spaces, Unicode and empty values", () => {
    expect(parseMcpEnvironment('{"API_KEY":"private value/ä","EMPTY":""}')).toEqual({API_KEY: "private value/ä", EMPTY: ""});
  });
  it("rejects malformed objects, ambiguous names, NULs and bounded oversized values", () => {
    for (const value of [[], {KEY: 7}, {"A=B": "value"}, {PATH: "one", Path: "two"}, {KEY: "bad\0value"}, {KEY: "x".repeat(2001)}]) expect(() => parseMcpEnvironment(JSON.stringify(value))).toThrow();
  });
});
