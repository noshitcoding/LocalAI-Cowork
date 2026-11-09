import {describe,it,expect} from "vitest";
import {decode} from "@cowork/client";
import {knowledgeFolderPaths,knowledgeFolderSelection,knowledgeTopicProblem} from "./knowledgeTopics";
import {ProductRequestError} from "./productApi";

const folders=[{id:"a",name:"Research",parent_id:null},{id:"b",name:"Models",parent_id:"a"},{id:"c",name:"Quantization",parent_id:"b"},{id:"d",name:"Other",parent_id:null}];
describe("knowledge topics",()=>{
  it("shows nested paths and selects only the requested subtree",()=>{
    expect(knowledgeFolderPaths(folders).find(f=>f.id==="c")?.path).toBe("Research / Models / Quantization");
    expect([...knowledgeFolderSelection(folders,"a",true)].sort()).toEqual(["a","b","c"]);
    expect([...knowledgeFolderSelection(folders,"b",false)]).toEqual(["b"]);
    expect(()=>knowledgeFolderPaths([{id:"a",name:"loop",parent_id:"a"}])).toThrow("knowledge_folder_cycle_or_depth");
  });
  it("keeps omitted locations compatible while preserving explicit unfile requests",()=>{
    const base={id:null,expected_revision:0,kind:"knowledge",title:"Note",content:"Body",tags:[]};
    expect(decode("WriteKnowledge",base).location).toBeUndefined();
    expect(decode("WriteKnowledge",{...base,location:{folder_id:null}}).location?.folder_id).toBeNull();
    expect(decode("WriteKnowledge",{...base,location:{folder_id:"a"}}).location?.folder_id).toBe("a");
    expect(()=>decode("WriteKnowledge",{...base,location:{folder_id:42}})).toThrow();
  });
  it("explains stale graphs and concurrent folder edits",()=>{
    expect(knowledgeTopicProblem(new ProductRequestError("invalid_request",400,"knowledge_graph_stale"),true)).toContain("veraltet");
    expect(knowledgeTopicProblem(new ProductRequestError("revision_conflict",409),false)).toContain("changed elsewhere");
  });
});
