import {afterEach,describe,it,expect,vi} from "vitest";
import {Client,decode} from "./index";
import {createServer} from "node:http";
const pool=()=>({currency:{code:"USD",scale:6},policy_version:"1",limit_units:"18446744073709551615",charged_units:"9007199254740993",confirmed_units:"9007199254740993",pending_units:"0",unknown_units:"0",remaining_units:"18437736874454810622",confirmed_attempts:"1",pending_attempts:"1",unknown_attempts:"1",overrun:false,limit_exceeded:false,no_headroom:false});
const configured=()=>({origin:"operator_configuration" as const,installation:pool(),defaults:{project_limit_units:"18446744073709551615",run_limit_units:"9007199254740993",pool_policy_version:"1"}});
afterEach(()=>vi.unstubAllGlobals());
describe("read-only personal money status",()=>{
  it("keeps the unbound UI callback attached to its client through a real HTTP GET",async()=>{
    let requests=0;
    const server=createServer((request,response)=>{
      requests++;
      expect(request.method).toBe("GET");expect(request.url).toBe("/api/v2/personal-money");
      expect(request.headers.authorization).toBe("Bearer private-callback-fixture");
      response.setHeader("Content-Type","application/json");response.end(JSON.stringify({origin:"not_configured"}));
    });
    await new Promise<void>(resolve=>server.listen(0,"127.0.0.1",resolve));
    try {
      const address=server.address();if(!address||typeof address==="string")throw new Error("private fixture address");
      const callback=new Client("private-callback-fixture",`http://127.0.0.1:${address.port}`).personalMoney;
      expect(await callback()).toEqual({origin:"not_configured"});expect(requests).toBe(1);
    } finally {
      await new Promise<void>((resolve,reject)=>server.close(error=>error?reject(error):resolve()));
    }
  });
  it("keeps maximum amounts exact and rejects unsafe or noncanonical values",()=>{
    const value=decode("PersonalMoneyStatus",configured());
    if(value.origin!=="operator_configuration")throw new Error("unexpected fixture");
    expect(value.installation.limit_units).toBe("18446744073709551615");
    expect(value.installation.charged_units).toBe("9007199254740993");
    for(const bad of [0,1.5,null,"18446744073709551616","01","+1","-1","1e3","1.0","1\n",""]){
      const invalid=configured();Object.assign(invalid.installation,{charged_units:bad});
      expect(()=>decode("PersonalMoneyStatus",invalid)).toThrow("contract_mismatch");
    }
    for(const bad of [{origin:"not_configured",ledger_revision:"1"},{origin:"legacy_manual"},{...configured(),config:{endpoint:"private"}},{...configured(),installation:{...pool(),currency:{code:"usd",scale:6}}}])expect(()=>decode("PersonalMoneyStatus",bad)).toThrow();
  });
  it("issues one authorized GET and keeps zero-priced incomplete counts visible",async()=>{
    const value={origin:"legacy_manual",installation:{...pool(),charged_units:"0",confirmed_units:"0",remaining_units:"18446744073709551615",confirmed_attempts:"0",pending_attempts:"1",unknown_attempts:"2"}};
    const fetcher=vi.fn().mockResolvedValue(new Response(JSON.stringify(value),{status:200,headers:{"Content-Type":"application/json"}}));vi.stubGlobal("fetch",fetcher);
    const result=await new Client("private-personal-fixture","http://127.0.0.1:9").personalMoney();
    expect(result).toEqual(value);expect(fetcher).toHaveBeenCalledTimes(1);
    expect(fetcher.mock.calls[0][0]).toBe("http://127.0.0.1:9/api/v2/personal-money");
    expect(fetcher.mock.calls[0][1]).toEqual({method:"GET",headers:{Authorization:"Bearer private-personal-fixture"},body:undefined});
  });
  it("rejects inconsistent aggregates and does not retry a failed status",async()=>{
    const fetcher=vi.fn();vi.stubGlobal("fetch",fetcher);
    for(const patch of [{remaining_units:"1"},{confirmed_units:"1"},{pending_units:"18446744073709551615"},{limit_exceeded:true},{no_headroom:true},{confirmed_attempts:"0"},{pending_attempts:"0",pending_units:"1",confirmed_units:"9007199254740992"},{unknown_attempts:"0",unknown_units:"1",confirmed_units:"9007199254740992"}]) {
      const value=configured();Object.assign(value.installation,patch);
      fetcher.mockClear();fetcher.mockResolvedValue(new Response(JSON.stringify(value),{status:200}));
      await expect(new Client("private").personalMoney()).rejects.toThrow("personal_money_integrity");expect(fetcher).toHaveBeenCalledTimes(1);
    }
    fetcher.mockClear();fetcher.mockResolvedValue(new Response(JSON.stringify({code:"storage_unavailable",message:"Storage operation failed",retryable:true,correlation_id:"private-fixture"}),{status:503}));
    await expect(new Client("private").personalMoney()).rejects.toMatchObject({status:503});expect(fetcher).toHaveBeenCalledTimes(1);
  });
});
