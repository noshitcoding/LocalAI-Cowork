import {describe, expect, it, vi,afterEach} from "vitest";
import {decode,Client} from "./index";
import {defaultRunBudgets} from "./generated";

describe("durable MCP contracts", () => {
  afterEach(()=>vi.unstubAllGlobals());
  it("accepts explicit references without accepting model-supplied definitions",()=>{
    const reference={server_id:"server",server_revision:1,tool_name:"append"};
    expect(decode("McpToolSelection",reference)).toEqual(reference);
    expect(()=>decode("McpToolSelection",{...reference,definition:{name:"forged"}})).toThrow("contract_mismatch");
    const configuration={name:"External",endpoint:"https://example.invalid/mcp",credential_reference:null,credential_revision:0,catalog:[],selected_tools:[],enabled:false};
    expect(()=>decode("McpServerConfiguration",{...configuration,bearer:"secret"})).toThrow("contract_mismatch");
  });
  it("never automatically retries an unconfirmed configuration mutation",async()=>{
    const fetch=vi.fn().mockRejectedValue(new TypeError("network"));vi.stubGlobal("fetch",fetch);
    const client=new Client("owner");
    const input={server_id:"server",base_revision:0,idempotency_key:"same",configuration:{name:"External",endpoint:"https://example.invalid/mcp",credential_reference:null,credential_revision:0,catalog:[],selected_tools:[],enabled:false}};
    await expect(client.configureMcpServer("a/b",input)).rejects.toThrow("network");
    expect(fetch).toHaveBeenCalledTimes(1);expect(fetch.mock.calls[0][0]).toBe("/api/v2/projects/a%2Fb/mcp-servers");
    expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(input);
  });
  it("preserves historical run serialization and rejects credentials in frozen bindings", () => {
    const historical = {
      order_id: "order", project_id: "project", prompt: "historical", provider: null,
      file_action: null, access_mode: "confirm_mutations", executor: "local",
      policy_revision: 1, allowed_tools: [], budgets: defaultRunBudgets, folders: [],
    };
    expect(JSON.stringify(decode("RunSpec", historical))).toBe(JSON.stringify(historical));
    const binding = {
      alias: "mcp_alias", server_id: "server", server_revision: 1,
      endpoint: "http://127.0.0.1:1234/mcp", configuration_sha256: "a".repeat(64),
      catalog_sha256: "b".repeat(64), definition: {name: "append", inputSchema: {type: "object"}},
    };
    expect(decode("RunSpec", {...historical, mcp_tools: [binding]}).mcp_tools).toEqual([binding]);
    expect(() => decode("McpToolBinding", {...binding, bearer_token: "unexpected"})).toThrow("contract_mismatch");
  });

  it("requires confirmed result evidence and keeps uncertainty a separate state", () => {
    expect(decode("McpAttemptState", {state: "dispatch_committed"})).toEqual({state: "dispatch_committed"});
    const uncertain = {state: "outcome_uncertain", code: "mcp_timeout", response: null, response_sha256: null};
    expect(decode("McpAttemptState", uncertain)).toEqual(uncertain);
    for (const invalid of [{state: "completed"}, {state: "retry_ready"}, {...uncertain, automatic_retry: true}]) {
      expect(() => decode("McpAttemptState", invalid)).toThrow("contract_mismatch");
    }
  });
});
