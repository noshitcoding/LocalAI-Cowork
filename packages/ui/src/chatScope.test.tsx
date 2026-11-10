import {describe,expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {QueryClient,QueryClientProvider} from "@tanstack/react-query";
import {Client} from "@cowork/client";
import {independentChats,visibleProjects} from "./chatScope";
import {ChatManagementDialog} from "./ChatManagementDialog";
import {SearchPanel} from "./WorkPanels";
import {ArchivedChatsPanel} from "./ArchivedChatsPanel";
import {workspaceManagementKey} from "./workspaceManagementApi";
import {de} from "./i18n";

const client=new Client("fixture");
const projects=[
  {id:independentChats,name:"Internal chat storage",revision:1,created_at:0},
  {id:"user-project",name:"Chats",revision:1,created_at:0},
];
describe("personal chats stay outside user projects",()=>{
  it("excludes the internal ID while retaining a user project called Chats",()=>{
    expect(visibleProjects(projects)).toEqual([projects[1]]);
    expect(projects).toHaveLength(2);
  });
  it("shows one Without project choice and no internal project in chat management",()=>{
    const markup=renderToStaticMarkup(<QueryClientProvider client={new QueryClient()}>
      <ChatManagementDialog client={client} projects={projects}
        order={{id:"chat",project_id:independentChats,title:"Existing chat",draft:"Keep draft",revision:4,created_at:0}}
        onChanged={()=>{}} onClose={()=>{}}/>
    </QueryClientProvider>);
    expect(markup).toContain(`<option value="${independentChats}" selected="">Ohne Projekt</option>`);
    expect(markup.match(/<option /g)).toHaveLength(2);
    expect(markup).not.toContain("Internal chat storage");
    expect(markup).toContain('<option value="user-project">Chats</option>');
  });
  it("labels the personal search scope as Chats instead of Current project",()=>{
    const markup=renderToStaticMarkup(<QueryClientProvider client={new QueryClient()}>
      <SearchPanel client={client} project={independentChats} t={key=>de[key]} open={()=>{}}/>
    </QueryClientProvider>);
    expect(markup).toContain(`<option value="${independentChats}">Chats</option>`);
    expect(markup).not.toContain(de.currentProject);
  });
  it("keeps the personal archive readable without describing it as a project",()=>{
    const queries=new QueryClient();
    queries.setQueryData(["archived-chats",...workspaceManagementKey(client),independentChats],[]);
    const markup=renderToStaticMarkup(<QueryClientProvider client={queries}>
      <ArchivedChatsPanel client={client} project={independentChats}/>
    </QueryClientProvider>);
    expect(markup).toContain("Keine archivierten Chats.");
    expect(markup).not.toContain("in diesem Projekt");
  });
});
