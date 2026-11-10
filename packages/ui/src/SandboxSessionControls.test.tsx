import {describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client} from "@cowork/client";
import {SandboxSessionControls} from "./SandboxSessionControls";

vi.mock("./view",()=>({useView:(select:(state:{language:"en"})=>unknown)=>select({language:"en"})}));

describe("sandbox session reload",()=>{
  it("keeps every new lifecycle mutation disabled until history and current identity are observed",()=>{
    const markup=renderToStaticMarkup(<SandboxSessionControls client={new Client("secret","https://cowork.example")} project="project" order="order" environment={{environment_id:"env",version_id:"version",definition_sha256:"a".repeat(64),qualification_sha256:"b".repeat(64)}}/>);
    const buttons=[...markup.matchAll(/<button\b([^>]*)>([^<]+)<\/button>/g)];
    for(const label of ["Start environment","Suspend","Resume","Stop"]){
      const button=buttons.find(([, ,text])=>text===label);
      expect(button,label).toBeDefined();expect(button?.[1],label).toContain("disabled");
    }
  });
});
