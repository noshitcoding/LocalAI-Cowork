import type {SettingsSection} from "./SettingsWorkspace";
export type WorkspaceCommandActions={newChat:()=>void|Promise<void>;newProject:()=>void;search:(query?:string)=>void;settings:(section:SettingsSection)=>void;tasks:()=>void;projectSettings:()=>void;library:(tab:"workflows"|"personalities"|"agents")=>void;activity:()=>void;toggleTheme:()=>void;help?:()=>void};
export type WorkspaceCommand={id:string;label:string;keywords:string;shortcut?:string;disabled?:boolean;run:()=>void|Promise<void>};
const sections:Record<string,SettingsSection>={general:"general",allgemein:"general",ai:"ai",ki:"ai",models:"ai",modelle:"ai",codex:"ai",teams:"teams",server:"teams",mcp:"mcp",tools:"mcp",agents:"agents",agenten:"agents",knowledge:"knowledge",wissen:"knowledge",automation:"automation",skills:"automation",schedules:"automation",zeitpläne:"automation",terminal:"terminal",sandbox:"sandbox",environments:"sandbox",umgebungen:"sandbox",interface:"general",oberfläche:"general",security:"security",sicherheit:"security",usage:"usage",nutzung:"usage",verbrauch:"usage",data:"data",daten:"data",system:"system"};
const libraryTabs:Record<string,"workflows"|"personalities"|"agents">={workflows:"workflows",workflow:"workflows",personalities:"personalities",personality:"personalities",persönlichkeiten:"personalities",agents:"agents",agent:"agents",agenten:"agents"};
export type WorkspaceSlashCommand={kind:"new"|"tasks"|"help"}|{kind:"settings";section:SettingsSection}|{kind:"library";tab:"workflows"|"personalities"|"agents"}|{kind:"search";query:string}|{kind:"invalid";usage:string};
/** Only complete, explicitly recognized local commands are intercepted. */
export function parseWorkspaceSlashCommand(input:string):WorkspaceSlashCommand|null{
 const value=input.trim();if(!value.startsWith("/")||value.includes("\n"))return null;
 const match=/^\/(\S+)(?:\s+(.*))?$/.exec(value);if(!match)return null;const name=match[1].toLowerCase(),arg=(match[2]??"").trim();
 if(["new","new-chat","neu"].includes(name))return arg?{kind:"invalid",usage:"/new"}:{kind:"new"};
 if(["tasks","aufgaben"].includes(name))return arg?{kind:"invalid",usage:"/tasks"}:{kind:"tasks"};
 if(["help","hilfe"].includes(name))return arg?{kind:"invalid",usage:"/help"}:{kind:"help"};
 if(["settings","einstellungen"].includes(name)){const section=arg?sections[arg.toLowerCase()]:"general";return section?{kind:"settings",section}:{kind:"invalid",usage:"/settings [general|ai|agents|knowledge|automation|mcp|terminal|sandbox|security|teams|data|usage|system]"};}
 if(["library","bibliothek"].includes(name)){const tab=arg?libraryTabs[arg.toLowerCase()]:"workflows";return tab?{kind:"library",tab}:{kind:"invalid",usage:"/library [workflows|personalities|agents]"};}
 if(["search","suche"].includes(name))return {kind:"search",query:arg};
 return null;
}
export function runWorkspaceSlashCommand(command:WorkspaceSlashCommand,actions:WorkspaceCommandActions):void|Promise<void>{switch(command.kind){case "new":return actions.newChat();case "tasks":return actions.tasks();case "help":return actions.help?.();case "settings":return actions.settings(command.section);case "library":return actions.library(command.tab);case "search":return actions.search(command.query);case "invalid":throw new Error(command.usage);}}
export function workspaceCommands({language,hasProject,hasWorkspace=hasProject,hasChat,actions}:{language:"de"|"en";hasProject:boolean;hasWorkspace?:boolean;hasChat:boolean;actions:WorkspaceCommandActions}):WorkspaceCommand[]{const tr=(a:string,b:string)=>language==="de"?a:b;return [
 {id:"new-chat",label:tr("Neuer Chat","New Chat"),keywords:"new neu conversation chat",run:actions.newChat},
 {id:"new-project",label:tr("Neues Projekt","New project"),keywords:"project create erstellen projekt",run:actions.newProject},
 {id:"search",label:tr("Chatverlauf durchsuchen","Search chat history"),keywords:"suche finden search history",run:()=>actions.search()},
 {id:"tasks",label:tr("Aufgaben öffnen","Open tasks"),keywords:"tasks aufgaben repeat workflow",disabled:!hasWorkspace,run:actions.tasks},
 {id:"project-settings",label:tr("Projekt konfigurieren","Configure project"),keywords:"sources quellen instructions anweisungen archiv archive chat",disabled:!hasProject,run:actions.projectSettings},
 {id:"library-workflows",label:tr("Workflow-Bibliothek öffnen","Open workflow library"),keywords:"workflow library ablauf",disabled:!hasWorkspace,run:()=>actions.library("workflows")},
 {id:"library-personalities",label:tr("Persönlichkeiten verwalten","Manage personalities"),keywords:"personality personalities persönlichkeit prompt",run:()=>actions.library("personalities")},
 {id:"library-agents",label:tr("Agentvorlagen verwalten","Manage agent templates"),keywords:"agent templates vorlagen rollen",run:()=>actions.library("agents")},
 {id:"settings",label:tr("Einstellungen öffnen","Open settings"),keywords:"settings einstellungen preferences",run:()=>actions.settings("general")},
 {id:"models",label:tr("Modelle konfigurieren","Configure models"),keywords:"models modelle provider ollama codex ki ai",run:()=>actions.settings("ai")},
 {id:"mcp",label:tr("MCP und Werkzeuge konfigurieren","Configure MCP and tools"),keywords:"mcp tools werkzeuge servers",run:()=>actions.settings("mcp")},
 {id:"schedules",label:tr("Zeitpläne öffnen","Open schedules"),keywords:"schedules recurring cron automation zeitplan",disabled:!hasWorkspace,run:()=>actions.settings("automation")},
 {id:"activity",label:tr("Aktivität und Freigaben öffnen","Open activity and approvals"),keywords:"activity inbox approvals freigaben aktivität",run:actions.activity},
 {id:"theme",label:tr("Farbschema wechseln","Toggle color theme"),keywords:"theme dark light dunkel hell design",run:actions.toggleTheme},
 ...(actions.help?[{id:"help",label:tr("Chatbefehle anzeigen","Show chat commands"),keywords:"help hilfe slash commands befehle",disabled:!hasChat,run:actions.help}]:[])
 ];}
export function filterWorkspaceCommands(commands:WorkspaceCommand[],query:string):WorkspaceCommand[]{const terms=query.toLocaleLowerCase().trim().split(/\s+/).filter(Boolean);return commands.filter(command=>terms.every(term=>(command.label+" "+command.keywords).toLocaleLowerCase().includes(term)));}
export const workspaceSlashHelp="/new · /tasks · /settings [category] · /library [workflows|personalities|agents] · /search [text] · /help";
