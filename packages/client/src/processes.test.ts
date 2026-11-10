import {describe, expect, it} from "vitest";
import {decode} from "./index";
import {defaultRunBudgets} from "./generated";

describe("durable process contracts", () => {
  it("preserves binary evidence and rejects bytes or invocation fields outside the contract", () => {
    const capture = {bytes: [0, 255, 128], sha256: "a".repeat(64), total_bytes: 3, truncated: false};
    expect(decode("ProcessCapture", capture).bytes).toEqual([0, 255, 128]);
    for (const byte of [-1, 256, 1.5]) {
      expect(() => decode("ProcessCapture", {...capture, bytes: [byte]})).toThrow("contract_mismatch");
    }
    const invocation = {program: "registered-program", arguments: ["one quoted argument"], timeout_seconds: 10};
    expect(decode("ProcessInvocation", invocation)).toEqual(invocation);
    for (const extra of [{environment: {TOKEN: "unexpected"}}, {shell: true}, {working_directory: "outside"}]) {
      expect(() => decode("ProcessInvocation", {...invocation, ...extra})).toThrow("contract_mismatch");
    }
  });

  it("reads historical run configurations and validates an explicit environment binding", () => {
    const historical = {
      order_id: "order", project_id: "project", prompt: "historical", provider: null,
      file_action: null, access_mode: "confirm_mutations", executor: "local",
      policy_revision: 1, allowed_tools: [], budgets: defaultRunBudgets, folders: [],
    };
    expect(decode("RunSpec", historical).environment).toBeUndefined();
    const environment = {
      environment_id: "environment", version_id: "version",
      definition_sha256: "a".repeat(64), qualification_sha256: "b".repeat(64),
    };
    expect(decode("RunSpec", {...historical, environment}).environment).toEqual(environment);
    expect(() => decode("RunSpec", {...historical, environment: {...environment, credentials: "unexpected"}}))
      .toThrow("contract_mismatch");
  });
});
