import {describe,expect,it} from "vitest";
import {RequestError} from "@cowork/client";
import {isPersonalEntry,knowledgeBaseDisabled,personalMemory,toggleException,validateActorAccess,validateDirectory,validateOwnerAccess,validatePolicy,validateProjectAccess} from "./knowledgeAccessApi";
import {validateMemoryMetadata} from "./memoryMetadata";

const user="0b9c7a3e-1f2d-4c5b-8a6e-9d0c1b2a3f4e",project="5e4d3c2b-1a09-4f8e-b7d6-c5b4a3928170",team="9f8e7d6c-5b4a-4392-8170-6f5e4d3c2b1a";
const policy={revision:2,private_projects:false,team_projects:false,enabled_users:[user],enabled_projects:[project],updated_at:1,updated_by:user};

describe("knowledge access responses",()=>{
  it("accepts exact local switch views and rejects foreign or extended ones",()=>{
    expect(validateOwnerAccess({revision:0,personal_knowledge:true})).toEqual({revision:0,personal_knowledge:true});
    expect(()=>validateOwnerAccess({revision:0,personal_knowledge:"yes"})).toThrow("knowledge_access_invalid");
    const view={project_id:"p1",revision:1,project_knowledge:false,personal_knowledge:true,personal_available:false};
    expect(validateProjectAccess(view,"p1")).toEqual(view);
    expect(()=>validateProjectAccess(view,"p2")).toThrow("knowledge_access_invalid");
    expect(()=>validateProjectAccess({...view,extra:1},"p1")).toThrow("knowledge_access_invalid");
    expect(()=>validateProjectAccess({...view,revision:-1},"p1")).toThrow("knowledge_access_invalid");
  });
  it("validates the administrator policy and directory",()=>{
    expect(validatePolicy(policy)).toEqual(policy);
    expect(()=>validatePolicy({...policy,enabled_users:[user,user]})).toThrow("knowledge_access_invalid");
    expect(()=>validatePolicy({...policy,enabled_projects:["not-a-uuid"]})).toThrow("knowledge_access_invalid");
    const directory={policy,users:[{id:user,username:"u",display_name:"User",active:true,private_projects:true}],projects:[{id:project,name:"P",team_id:team,team_name:"T",archived:false,team_projects:true}],truncated:false};
    expect(validateDirectory(directory).projects[0].team_name).toBe("T");
    expect(()=>validateDirectory({...directory,users:[{...directory.users[0],id:"x"}]})).toThrow("knowledge_access_invalid");
  });
  it("reads the separate switch for personal knowledge in team runs only when the server has it",()=>{
    // Older servers omit both fields; the panel then neither shows nor sends them.
    expect(validatePolicy(policy).personal_team_runs).toBeUndefined();
    const current={...policy,personal_team_runs:false,enabled_personal_team_users:[user]};
    expect(validatePolicy(current)).toEqual(current);
    expect(()=>validatePolicy({...policy,personal_team_runs:true})).toThrow("knowledge_access_invalid");
    expect(()=>validatePolicy({...current,enabled_personal_team_users:["x"]})).toThrow("knowledge_access_invalid");
    expect(()=>validatePolicy({...current,personal_team_runs:"yes"})).toThrow("knowledge_access_invalid");
    const listed={id:user,username:"u",display_name:"User",active:true,private_projects:false,personal_team_runs:true};
    expect(validateDirectory({policy:current,users:[listed],projects:[],truncated:false}).users[0].personal_team_runs).toBe(true);
  });
  it("treats a missing actor flag as not allowed",()=>{
    expect(validateActorAccess({private_projects:true})).toEqual({private_projects:true,personal_team_runs:false});
    expect(validateActorAccess({private_projects:false,personal_team_runs:true})).toEqual({private_projects:false,personal_team_runs:true});
    expect(()=>validateActorAccess({private_projects:false,personal_team_runs:1})).toThrow("knowledge_access_invalid");
    expect(()=>validateActorAccess({private_projects:false,extra:true})).toThrow("knowledge_access_invalid");
  });
});

describe("knowledge access helpers",()=>{
  it("keeps exception lists sorted and unique",()=>{
    expect(toggleException([project],user,true)).toEqual([user,project].sort());
    expect(toggleException([project,user],project,false)).toEqual([user]);
    expect(toggleException([user],user,true)).toEqual([user]);
  });
  it("creates valid user-scoped memory for the personal knowledge base",()=>{
    const memory=personalMemory();
    expect(validateMemoryMetadata(memory)).toEqual(memory);
    expect(isPersonalEntry({memory})).toBe(true);
    expect(isPersonalEntry({memory:{...memory,scope:{kind:"project"}}})).toBe(false);
    expect(isPersonalEntry({})).toBe(false);
  });
  it("recognizes disabled knowledge bases from both API styles",()=>{
    expect(knowledgeBaseDisabled(new RequestError({code:"invalid_request",message:"knowledge_base_disabled",retryable:false,correlation_id:"c"},400))).toBe(true);
    expect(knowledgeBaseDisabled(new Error("knowledge_base_disabled (403)"))).toBe(true);
    expect(knowledgeBaseDisabled(new Error("invalid_request (400)"))).toBe(false);
  });
});
