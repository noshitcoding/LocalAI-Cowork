import {describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {CommandPalette} from "./CommandPalette";
import {filterWorkspaceCommands,parseWorkspaceSlashCommand,runWorkspaceSlashCommand,workspaceCommands,type WorkspaceCommandActions} from "./workspaceCommands";
function actions():WorkspaceCommandActions{return {newChat:vi.fn(),newProject:vi.fn(),search:vi.fn(),settings:vi.fn(),tasks:vi.fn(),projectSettings:vi.fn(),library:vi.fn(),activity:vi.fn(),toggleTheme:vi.fn(),help:vi.fn()};}
describe("local workspace commands",()=>{
 it("keeps ordinary chat tools available without exposing project settings",()=>{
  const commands=workspaceCommands({language:"de",hasProject:false,hasWorkspace:true,hasChat:true,actions:actions()});
  expect(commands.find(c=>c.id==="project-settings")?.disabled).toBe(true);
  for(const id of ["tasks","library-workflows","schedules"])expect(commands.find(c=>c.id===id)?.disabled).toBe(false);
 });
 it("parses only complete known commands and preserves ordinary slash prompts",()=>{
  expect(parseWorkspaceSlashCommand("/settings mcp")).toEqual({kind:"settings",section:"mcp"});expect(parseWorkspaceSlashCommand("/settings modelle")).toEqual({kind:"settings",section:"ai"});expect(parseWorkspaceSlashCommand("/settings sandbox")).toEqual({kind:"settings",section:"sandbox"});expect(parseWorkspaceSlashCommand("/library personalities")).toEqual({kind:"library",tab:"personalities"});expect(parseWorkspaceSlashCommand("/search quoted evidence")).toEqual({kind:"search",query:"quoted evidence"});expect(parseWorkspaceSlashCommand("/tasks unexpected")).toMatchObject({kind:"invalid"});expect(parseWorkspaceSlashCommand("/tmp/project notes")).toBeNull();expect(parseWorkspaceSlashCommand("/settings\nExplain the settings")).toBeNull();
 });
 it("routes commands only to their actual app actions",async()=>{
  const calls=actions();await runWorkspaceSlashCommand({kind:"settings",section:"automation"},calls);expect(calls.settings).toHaveBeenCalledWith("automation");expect(calls.newChat).not.toHaveBeenCalled();runWorkspaceSlashCommand({kind:"search",query:"checkpoints"},calls);expect(calls.search).toHaveBeenCalledWith("checkpoints");expect(()=>runWorkspaceSlashCommand({kind:"invalid",usage:"/tasks"},calls)).toThrow("/tasks");expect(calls.tasks).not.toHaveBeenCalled();
 });
 it("filters translated commands, keeps project actions disabled without a project and leaves library personalities available",async()=>{
  const calls=actions(),commands=workspaceCommands({language:"de",hasProject:false,hasChat:false,actions:calls});expect(commands.find(c=>c.id==="tasks")?.disabled).toBe(true);expect(commands.find(c=>c.id==="library-personalities")?.disabled).toBeUndefined();expect(filterWorkspaceCommands(commands,"mcp werkzeuge").map(c=>c.id)).toEqual(["mcp"]);await commands.find(c=>c.id==="library-agents")!.run();expect(calls.library).toHaveBeenCalledWith("agents");
 });
 it("renders a labeled keyboard-searchable command dialog",()=>{
  const html=renderToStaticMarkup(<CommandPalette open={false} onClose={()=>{}} commands={workspaceCommands({language:"en",hasProject:true,hasChat:true,actions:actions()})} language="en"/>);expect(html).toContain("Command palette");expect(html).toContain('role="combobox"');expect(html).toContain('role="listbox"');expect(html).toContain("Manage personalities");
 });
});
