import {describe, expect, it} from "vitest";
import type {CreateProvider, ProviderProfile} from "@cowork/client";
import {completeProviderSave, discoverProviderModels, providerGeneration, providerSettingsError, type PendingProviderSave, type ProviderDraft} from "./ProviderSettingsPanel";

const profile: CreateProvider = {name: "Local", kind: "ollama", endpoint: "http://127.0.0.1:11434", model: "qwen3:8b", context_tokens: 32768};
const saved: ProviderProfile = {...profile, id: "provider-a", revision: 1, credential_ready: true, credential_required: false};
describe("provider settings save", () => {
  it("does not create a second provider when generation saving loses its response", async () => {
    const operation: PendingProviderSave = {generation: {temperature: 0}, create: {idempotency_key: "create-original", profile, api_key: null, explicitly_allow_no_key: true}};
    const calls: {path: string; method: string; body: unknown}[] = [];
    let updates = 0;
    const api = async <T,>(path: string, method: string, body?: unknown): Promise<T> => {
      calls.push({path, method, body});
      if (path.endsWith("with-credential")) return saved as T;
      if (++updates === 1) throw new Error("network response lost");
      return {profile: {...saved, revision: 2}, generation: {temperature: 0}} as T;
    };
    await expect(completeProviderSave(api, operation)).rejects.toThrow("response lost");
    expect(operation.target?.id).toBe(saved.id);
    const originalUpdate = operation.update;
    await expect(completeProviderSave(api, operation)).resolves.toMatchObject({id: saved.id, revision: 2});
    expect(calls.filter(call => call.method === "POST")).toHaveLength(1);
    expect(calls[1].body).toBe(originalUpdate);
    expect(calls[2].body).toBe(originalUpdate);
  });
  it("retains explicit zero temperature and converts seconds without inventing defaults", () => {
    expect(providerGeneration({temperature: "0", timeout: "30"})).toEqual({temperature: 0, timeout_ms: 30000});
    expect(providerGeneration({temperature: "", timeout: ""})).toEqual({});
    expect(() => providerGeneration({temperature: "NaN", timeout: ""})).toThrow("invalid_temperature");
    expect(() => providerGeneration({temperature: "", timeout: "0"})).toThrow("invalid_timeout");
  });
});

describe("model discovery before saving", () => {
  const draft = (): ProviderDraft => ({...profile, name:"", model:"", temperature:"", timeout:"", apiKey:"", allowNoKey:true, removeKey:false});
  it("loads models with no name or model ID and does not create a profile", async () => {
    const calls: unknown[] = [];
    const models = await discoverProviderModels(async <T,>(path:string, method:string, body?:unknown) => {
      calls.push({path,method,body}); return {models:["qwen38"]} as T;
    }, draft());
    expect(models).toEqual(["qwen38"]);
    expect(calls).toEqual([{path:"/providers/discover-models",method:"POST",body:{kind:"ollama",endpoint:profile.endpoint,api_key:null,explicitly_allow_no_key:true}}]);
  });
  it("uses the stored credential at its saved endpoint even while unrelated fields are edited", async () => {
    const calls:unknown[]=[];
    await discoverProviderModels(async <T,>(path:string,method:string,body?:unknown)=>{calls.push({path,method,body});return {models:["qwen38"]} as T;},draft(),{...saved,credential_required:true});
    expect(calls).toEqual([{path:"/providers/provider-a/models",method:"GET",body:undefined}]);
  });
  it("requires a fresh credential when the draft endpoint changes", async () => {
    let called=false;
    const api=async <T,>()=>{called=true;return {models:[]} as T;};
    await expect(discoverProviderModels(api,{...draft(),endpoint:"https://other.example/v1"},{...saved,credential_required:true})).rejects.toThrow("provider_discovery_key_required");
    expect(called).toBe(false);
  });
  it("uses a newly entered key only for the requested unsaved endpoint", async () => {
    const calls:unknown[]=[];
    await discoverProviderModels(async <T,>(path:string,method:string,body?:unknown)=>{calls.push({path,method,body});return {models:["qwen38"]} as T;}, {...draft(),kind:"openai_compatible",endpoint:" https://pcai.example/v1/ ",apiKey:"new-key",allowNoKey:false});
    expect(calls).toEqual([{path:"/providers/discover-models",method:"POST",body:{kind:"openai_compatible",endpoint:"https://pcai.example/v1",api_key:"new-key",explicitly_allow_no_key:false}}]);
  });
  it("explains wrong API path and wrong model separately", () => {
    expect(providerSettingsError(new Error("provider_endpoint_not_found (502)"),"de")).toContain("/v1");
    expect(providerSettingsError(new Error("provider_model_missing (502)"),"de")).toContain("Modell-ID");
    expect(providerSettingsError(new Error("provider_authentication_failed (502)"),"de")).toContain("401");
  });
});
