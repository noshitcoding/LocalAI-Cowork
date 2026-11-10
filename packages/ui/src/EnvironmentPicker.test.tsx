import {describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {QueryClient,QueryClientProvider} from "@tanstack/react-query";
import {Client} from "@cowork/client";
import {EnvironmentPicker} from "./EnvironmentPicker";
vi.mock("./view",()=>({useView:(selector:(state:{language:"en"})=>unknown)=>selector({language:"en"})}));
const environment={environment_id:"env",version_id:"version",definition_sha256:"a".repeat(64),qualification_sha256:"b".repeat(64)};
function render(nativeTools:string[],localAvailable:boolean,disabled=false){
  const client=new Client("synthetic"),queries=new QueryClient({defaultOptions:{queries:{retry:false}}});
  queries.setQueryData(["environments",client.base,client.token,"project"],[{id:"env",name:"Linux tools",revoked:false,active_reference:environment,versions:[]}]);
  return renderToStaticMarkup(<QueryClientProvider client={queries}><EnvironmentPicker client={client} project="project" value={null} onChange={vi.fn()} nativeTools={nativeTools} onNativeToolsChange={vi.fn()} nativeChoices={["execute_program"]} localAvailable={localAvailable} disabled={disabled}/></QueryClientProvider>);
}
describe("execution mode presentation",()=>{
  it("describes folder isolation and separate guest pairing without offering native rights on a server connection",()=>{
    const html=render([],false);
    expect(html).toContain('<option value="user_rights" disabled="">');
    expect(html).toContain("Granted files only; no processes");
    expect(html).toContain("do not gain access to installed Windows apps");
    expect(html).toContain("Windows test computer (pair separately)");
    expect(html).not.toContain('type="checkbox"');
  });
  it("keeps restored native authority visible and disabled when local access is unavailable",()=>{
    const html=render(["execute_program"],false,true);
    expect(html).toContain("The folder selection does not confine these programs");
    expect(html).toContain("A server connection does not grant laptop permissions");
    expect(html).toContain('<fieldset disabled="">');
    expect(html).toContain('checked=""');
  });
});
