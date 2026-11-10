import {describe,expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {MemoryMetadataView} from "./MemoryMetadataEditor";
import {defaultMemoryMetadata,memoryMetadataForm,validateMemoryMetadata} from "./memoryMetadata";
describe("memory metadata",()=>{
 it("preserves explicit scope, disable, cleared profile key and declared confidence in the save body",()=>{const data=new FormData();data.set("kind","memory");data.set("memory",JSON.stringify({...defaultMemoryMetadata(),scope:{kind:"chat",order_id:"chat"},enabled:false,confidence:0,profile_key:null}));expect(memoryMetadataForm(data)).toEqual({...defaultMemoryMetadata(),scope:{kind:"chat",order_id:"chat"},enabled:false,confidence:0,profile_key:null});data.set("kind","knowledge");expect(memoryMetadataForm(data)).toBeNull();});
 it("rejects unknown transfer or credential fields and invalid scope/confidence before saving",()=>{expect(()=>validateMemoryMetadata({...defaultMemoryMetadata(),scope:{kind:"chat",order_id:""}})).toThrow();expect(()=>validateMemoryMetadata({...defaultMemoryMetadata(),confidence:1.1})).toThrow();expect(()=>validateMemoryMetadata({...defaultMemoryMetadata(),api_key:"secret"})).toThrow();expect(()=>validateMemoryMetadata({...defaultMemoryMetadata(),owner_id:"other"})).toThrow();});
 it("labels disabled scoped memory and confidence as self-reported",()=>{const html=renderToStaticMarkup(<MemoryMetadataView value={{...defaultMemoryMetadata(),scope:{kind:"agent",agent_id:"agent"},enabled:false,confidence:0.8}}/>);expect(html).toContain("Deaktiviert");expect(html).toContain("Nur dieser Agent in diesem Projekt");expect(html).toContain("Selbsteinschätzung");});
});
