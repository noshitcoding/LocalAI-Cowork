import {describe,it,expect} from "vitest";
import {decode} from "./index";
describe("Rust-derived API validation",()=>{
  it("rejects malformed state before the UI treats it as confirmed",()=>{
    expect(()=>decode("RunRecord",{id:"r",state:"made_up"})).toThrow("contract_mismatch");
    expect(()=>decode("ApiError",{message:"No stable error code"})).toThrow();
  });
  it("does not assume an unknown price is zero",()=>{
    const usage=decode("Usage",{input_tokens:12,output_tokens:3,cost:null,price_source_version:null});
    expect(usage.cost).toBeNull();
  });
});

