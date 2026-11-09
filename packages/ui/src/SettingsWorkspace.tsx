import {createContext, useContext, useEffect, useId, useMemo, useRef, useState, type ReactNode} from "react";
import {useQueryClient} from "@tanstack/react-query";
import type {Client, McpToolSelection, ProviderProfile} from "@cowork/client";
import {ArrowLeft, BookOpen, Bot, Box, ChartNoAxesCombined, Clock, Cpu, Database, Info, Palette, Plug, ShieldCheck, SquareTerminal, Users, type LucideIcon} from "lucide-react";
import {ProductHubPanel, type ProductHubProps} from "./ProductHub";
import {ProviderSettingsPanel} from "./ProviderSettingsPanel";
import {CodexAccountsPanel} from "./CodexAccountsPanel";
import {SupportBundlePanel} from "./SupportBundlePanel";
import {DesktopPreferencesPanel,MaintenancePanel} from "./DesktopDataSettings";
import {TerminalDock} from "./DeferredWorkspaces";
import {ExternalMemoryPanel} from "./ExternalMemoryPanel";
import {ConnectorPanel} from "./ConnectorPanel";
import {McpLifecycleSettings} from "./McpLifecycleSettings";
import {McpTools} from "./McpTools";
import {ToolRuntimeSetup} from "./ToolRuntimeSetup";
import {AuditExecutionSummary} from "./AuditExecutionSummary";
import {PersonalMoneyStatus} from "./PersonalMoneyStatus";
import {NativeSecrets} from "./NativeSecrets";
import {NativeFolders} from "./NativeFolders";
import {DesktopUpdates} from "./DesktopUpdates";
import {ExecutionDefaultsPanel} from "./ExecutionDefaultsPanel";
import {useView} from "./view";
import {de, en, type Key} from "./i18n";
import {appearanceScope, defaultAppearancePreferences, useAppearancePreferences, type AppearancePreferences} from "./appearancePreferences";
import "./settingsWorkspace.css";

// Grouped like the V1 settings menu; every category opens real sub-tabs instead of collapsed blocks.
const settingsGroups = [
  {label: ["KI & Agents", "AI & agents"], sections: ["ai", "agents", "knowledge", "automation"]},
  {label: ["Werkzeuge & Ausführung", "Tools & execution"], sections: ["mcp", "terminal", "sandbox"]},
  {label: ["Arbeitsplatz", "Workspace"], sections: ["general", "security", "teams", "data"]},
  {label: ["Überblick & System", "Overview & system"], sections: ["usage", "system"]},
] as const;
export const settingsSections = settingsGroups.flatMap(group => group.sections);
export type SettingsSection = typeof settingsGroups[number]["sections"][number];
export type SettingsTabRequest = {section: SettingsSection; tab: string; nonce: number};
const labels: Record<SettingsSection, [string, string]> = {
  ai: ["KI & Modelle", "AI & models"], agents: ["Agents & Verhalten", "Agents & behavior"], knowledge: ["Wissen & Memory", "Knowledge & memory"],
  automation: ["Skills & Zeitpläne", "Skills & schedules"], mcp: ["MCP & Connectoren", "MCP & connectors"],
  terminal: ["Terminal & Prozesse", "Terminal & processes"], sandbox: ["Sandbox & Umgebungen", "Sandbox & environments"],
  general: ["Oberfläche", "Interface"], security: ["Sicherheit & Zugänge", "Security & access"], teams: ["Teams & Server", "Teams & server"],
  data: ["Daten & Sync", "Data & sync"], usage: ["Verbrauch & Audit", "Usage & audit"], system: ["System & Info", "System & info"],
};
const icons: Record<SettingsSection, LucideIcon> = {ai: Cpu, agents: Bot, knowledge: BookOpen, automation: Clock, mcp: Plug, terminal: SquareTerminal, sandbox: Box, general: Palette, security: ShieldCheck, teams: Users, data: Database, usage: ChartNoAxesCombined, system: Info};
export function normalizeSettingsSection(section: string): SettingsSection {return settingsSections.includes(section as SettingsSection) ? section as SettingsSection : "general";}
export function settingsSectionLabel(section: SettingsSection, language: "de" | "en") {return labels[section][language === "de" ? 0 : 1];}
export function SettingsNavigation({section, language, onSectionChange}: {section: SettingsSection; language: "de" | "en"; onSectionChange: (section: SettingsSection) => void}) {
  return <nav className="settings-navigation" aria-label={language === "de" ? "Einstellungskategorien" : "Settings categories"}>
    {settingsGroups.map(group => <div className="settings-navigation-group" key={group.label[1]}><p>{group.label[language === "de" ? 0 : 1]}</p>
      {group.sections.map(id => {const Icon = icons[id]; return <button type="button" key={id} aria-current={section === id ? "page" : undefined} onClick={() => onSectionChange(id)}><Icon size={17}/><span>{settingsSectionLabel(id, language)}</span></button>;})}</div>)}
  </nav>;
}

export type SettingsWorkspaceProps = ProductHubProps & {
  section: SettingsSection | string;
  onSectionChange: (section: SettingsSection) => void;
  /** Opens a sub-tab of a category, e.g. from the sidebar submenus. */
  tabRequest?: SettingsTabRequest | null;
  onLibrary?: (tab?: "workflows" | "personalities" | "agents") => void;
  onClose?: () => void;
  agentsPanel?: ReactNode;
};
const TabRequest = createContext<SettingsTabRequest | null>(null);
export function SettingsWorkspace(props: SettingsWorkspaceProps) {
  const language = useView(value => value.language), section = normalizeSettingsSection(props.section);
  const scope = useMemo(() => crypto.randomUUID(), [props.client]);
  // Visited sections remain mounted, retaining team sign-in and unfinished forms during navigation.
  const [visited, setVisited] = useState<SettingsSection[]>([section]);
  const content = useRef<HTMLDivElement>(null);
  useEffect(() => {if(content.current)content.current.scrollTop=0;}, [section]);
  useEffect(() => {setVisited(previous => previous.includes(section) ? previous : [...previous, section]);}, [section]);
  const sections = visited.includes(section) ? visited : [...visited, section];
  return <section className="settings-workspace" aria-label={language === "de" ? "Einstellungen" : "Settings"}>
    <header className="settings-header"><div>{props.onClose && <button type="button" className="quiet settings-back" onClick={props.onClose}><ArrowLeft size={16}/>{language === "de" ? "Zurück zum Chat" : "Back to chat"}</button>}<h1>{language === "de" ? "Einstellungen" : "Settings"}</h1><p>{language === "de" ? "Passe deinen Arbeitsbereich und deine Agenten an." : "Configure your workspace and agents."}</p></div><span className="settings-scope"><span className="status-dot"/>{props.nativeAvailable ? language === "de" ? "Lokaler Arbeitsbereich" : "Local workspace" : language === "de" ? "Server-Arbeitsbereich" : "Server workspace"}</span></header>
    <div className="settings-layout"><SettingsNavigation section={section} language={language} onSectionChange={props.onSectionChange}/>
      <div className="settings-content" ref={content}>{sections.map(id => <div key={scope + ":" + id} hidden={section !== id} inert={section !== id} className="settings-section" data-settings-section={id}>
        <TabRequest.Provider value={props.tabRequest?.section === id ? props.tabRequest : null}><SettingsSectionContent {...props} section={id}/></TabRequest.Provider>
      </div>)}</div>
    </div>
  </section>;
}

function SettingsTabs({tabs}: {tabs: {id: string; label: string; content: ReactNode}[]}) {
  const request = useContext(TabRequest);
  const initial = request && tabs.some(tab => tab.id === request.tab) ? request.tab : tabs[0].id;
  const [selected, setSelected] = useState(initial), [visited, setVisited] = useState([initial]); const prefix = useId();
  function select(id: string) {setSelected(id); setVisited(previous => previous.includes(id) ? previous : [...previous, id]);}
  useEffect(() => {if (request && tabs.some(tab => tab.id === request.tab)) select(request.tab);}, [request?.nonce]);
  function navigate(event: React.KeyboardEvent<HTMLButtonElement>, index: number) {
    const next = event.key === "ArrowRight" ? (index + 1) % tabs.length : event.key === "ArrowLeft" ? (index + tabs.length - 1) % tabs.length : event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : -1;
    if (next < 0) return; event.preventDefault(); select(tabs[next].id); event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("button")[next]?.focus();
  }
  const shown = tabs.some(tab => tab.id === selected) ? selected : tabs[0].id;
  return <><div className="settings-subtabs" role="tablist">{tabs.map((tab, index) => <button type="button" key={tab.id} id={prefix + tab.id} role="tab" aria-selected={shown === tab.id} aria-controls={prefix + tab.id + "-panel"} tabIndex={shown === tab.id ? 0 : -1} onKeyDown={event => navigate(event, index)} onClick={() => select(tab.id)}>{tab.label}</button>)}</div>
    {tabs.filter(tab => tab.id === shown || visited.includes(tab.id)).map(tab => <div key={tab.id} id={prefix + tab.id + "-panel"} role="tabpanel" aria-labelledby={prefix + tab.id} hidden={shown !== tab.id} inert={shown !== tab.id} className="settings-tab-panel">{tab.content}</div>)}</>;
}
function SectionHeading({title, description}: {title: string; description?: string}) {
  return <header className="settings-section-heading"><h2>{title}</h2>{description && <p>{description}</p>}</header>;
}
function SettingsSectionContent(props: SettingsWorkspaceProps & {section: SettingsSection}) {
  const {section, client, project, nativeAvailable} = props;
  const language = useView(value => value.language), tr = (de: string, en: string) => language === "de" ? de : en;
  const qc=useQueryClient();
  const t = (key: Key) => (language === "de" ? de : en)[key];
  if (section === "general") return <SettingsTabs tabs={[
    {id: "appearance", label: tr("Allgemein & Darstellung", "General & appearance"), content: <GeneralPreferences client={client}/>},
    {id: "desktop", label: tr("Start & Benachrichtigungen", "Startup & notifications"), content: <DesktopPreferencesPanel client={client}/>},
  ]}/>;
  if (section === "ai") return <SettingsTabs tabs={[
    {id: "models", label: tr("Modelle & Provider", "Models & providers"), content: <ProviderSettingsPanel client={client} providers={props.providers}/>},
    {id: "codex", label: "Codex", content: <CodexAccountsPanel client={client}/>},
    {id: "qualifications", label: tr("Modellqualifikationen", "Model qualifications"), content: <ProductHubPanel {...props} tab="models"/>},
  ]}/>;
  if (section === "teams") return <><SectionHeading title={tr("Teams & Server", "Teams & server")} description={tr("Teamkonten, Arbeitsbereiche, Mitglieder und Serverzugänge.", "Team accounts, workspaces, members and server access.")}/><ProductHubPanel {...props} tab="teams"/></>;
  if (section === "mcp") return <SettingsTabs tabs={[
    {id: "servers", label: tr("MCP-Server", "MCP servers"), content: <McpLifecycleSettings client={client}/>},
    {id: "tools", label: tr("Werkzeuge", "Tools"), content: project ? <McpToolManagement key={project} client={client} project={project}/> : <ProjectNotice/>},
    {id: "connectors", label: tr("Connectoren", "Connectors"), content: project ? <ConnectorPanel key={project} client={client} project={project}/> : <ProjectNotice/>},
    {id: "runtimes", label: tr("Werkzeuglaufzeiten", "Tool runtimes"), content: <ToolRuntimeSetup nativeAvailable={nativeAvailable}/>},
  ]}/>;
  if (section === "agents") return <SettingsTabs tabs={[
    {id: "defaults", label: tr("Agent-Standards", "Agent defaults"), content: props.agentsPanel},
    {id: "library", label: tr("Persönlichkeiten & Vorlagen", "Personalities & templates"), content: <section className="settings-card"><h2>{tr("Agentenbibliothek", "Agent library")}</h2><p>{tr("Persönlichkeiten und Agentenvorlagen lassen sich in Chats und Workflows wiederverwenden.", "Reuse personalities and agent templates in chats and workflows.")}</p>{props.onLibrary && <div className="actions"><button type="button" onClick={() => props.onLibrary?.("personalities")}>{tr("Persönlichkeiten verwalten", "Manage personalities")}</button><button type="button" onClick={() => props.onLibrary?.("agents")}>{tr("Agentenvorlagen verwalten", "Manage agent templates")}</button><button type="button" onClick={() => props.onLibrary?.("workflows")}>{tr("Workflows verwalten", "Manage workflows")}</button></div>}</section>},
  ]}/>;
  if (section === "knowledge") return <SettingsTabs tabs={[
    {id: "knowledge", label: tr("Wissen & Memory", "Knowledge & memory"), content: <ProductHubPanel {...props} tab="knowledge"/>},
    {id: "external", label: tr("Externes Memory", "External memory"), content: <ExternalMemoryPanel client={client} project={project||undefined} onImported={()=>{void qc.invalidateQueries({queryKey:["product"]});}}/>},
  ]}/>;
  if (section === "automation") return <SettingsTabs tabs={[{id:"skills",label:"Skills",content:<ProductHubPanel {...props} tab="skills"/>},{id:"schedules",label:tr("Zeitpläne","Schedules"),content:<ProductHubPanel {...props} tab="schedules"/>}]}/>;
  if (section === "terminal") return <SettingsTabs tabs={[
    {id: "terminal", label: "Terminal", content: props.order?<TerminalDock key={client.base+":"+props.order} client={client} order={props.order}/>:<p className="settings-empty">{tr("Öffne einen Chat, um dessen Terminal zu verwenden.", "Open a chat to use its terminal.")}</p>},
    {id: "tools", label: tr("Werkzeugausführung", "Tool execution"), content: <ProductHubPanel {...props} tab="tools"/>},
  ]}/>;
  if (section === "sandbox") return <SettingsTabs tabs={[
    {id: "defaults", label: tr("Standard-Ausführung", "Default execution"), content: <ExecutionDefaultsPanel client={client}/>},
    {id: "environments", label: tr("Ausführungsumgebungen", "Execution environments"), content: <ProductHubPanel {...props} tab="environments"/>},
    {id: "offline", label: tr("Offline-Ausführung", "Offline execution"), content: <ProductHubPanel {...props} tab="offline"/>},
  ]}/>;
  if (section === "security") return nativeAvailable ? <SettingsTabs tabs={[
    {id: "secrets", label: tr("Zugangsdaten", "Credentials"), content: <NativeSecrets language={language} t={t}/>},
    {id: "folders", label: tr("Ordnerfreigaben", "Folder access"), content: project ? <NativeFolders key={project} project={project} language={language} t={t}/> : <ProjectNotice/>},
  ]}/> : <><SectionHeading title={tr("Sicherheit & Zugänge", "Security & access")}/><section className="settings-card"><p>{tr("Konten, Anmeldung und Teamrechte verwaltest du unter Teams & Server.", "Manage accounts, sign-in and team permissions under Teams & server.")}</p><button type="button" onClick={() => props.onSectionChange("teams")}>{tr("Teamverwaltung öffnen", "Open team management")}</button></section></>;
  if (section === "usage") return <SettingsTabs tabs={[
    {id: "usage", label: tr("Verbrauch", "Usage"), content: <><SectionHeading title={tr("Nutzung & Insights", "Usage & insights")} description={tr("Nutzung und Kosten des verbundenen Arbeitsbereichs.", "Usage and costs for the connected workspace.")}/><AuditExecutionSummary client={client} token={client.token} path="/insights"/></>},
    {id: "budget", label: tr("Budget", "Budget"), content: <PersonalMoneyStatus load={client.personalMoney} language={language} t={t}/>},
    {id: "audit", label: tr("Audit & Diagnose", "Audit & diagnostics"), content: <ProductHubPanel {...props} tab="audit"/>},
  ]}/>;
  if (section === "data") return <SettingsTabs tabs={[
    {id: "maintenance", label: tr("Sicherung & Wartung", "Backup & maintenance"), content: <MaintenancePanel client={client}/>},
    {id: "transfer", label: tr("Import & Export", "Import & export"), content: <ProductHubPanel {...props} tab="transfer"/>},
    {id: "devices", label: tr("Geräte & Synchronisation", "Devices & synchronization"), content: <ProductHubPanel {...props} tab="devices"/>},
  ]}/>;
  return <SettingsTabs tabs={[
    {id: "updates", label: tr("Updates", "Updates"), content: <DesktopUpdates/>},
    {id: "support", label: tr("Support & Diagnose", "Support & diagnostics"), content: <SupportBundlePanel client={client}/>},
  ]}/>;
}
function ProjectNotice() {const language = useView(value => value.language); return <p className="settings-empty">{language === "de" ? "Wähle ein Projekt, um diese Einstellungen zu verwalten." : "Select a project to manage these settings."}</p>;}
function McpToolManagement({client, project}: {client: Client; project: string}) {
  const [selected, setSelected] = useState<McpToolSelection[]>([]);
  const language = useView(value => value.language), t = (key: Key) => (language === "de" ? de : en)[key];
  return <div className="settings-card"><McpTools client={client} project={project} selected={selected} setSelected={setSelected} t={t} mode="manage"/></div>;
}

export function GeneralPreferences({client}: {client: Pick<Client, "base">}) {
  const {language, setLanguage, theme, setTheme} = useView();
  const tr = (de: string, en: string) => language === "de" ? de : en;
  const {preferences, setPreferences} = useAppearancePreferences(appearanceScope(client));
  const [error, setError] = useState("");
  function save(patch: Partial<AppearancePreferences>) {try {setPreferences(patch); setError("");} catch {setError(tr("Die Anzeigeeinstellungen konnten nicht gespeichert werden.", "Appearance settings could not be saved."));}}
  const toggles: {key: "compactMode" | "focusMode" | "showTimestamps" | "reduceMotion"|"followSystemTheme"|"showThinking"|"confirmOnClose"; label: string; hint: string}[] = [
    {key:"followSystemTheme",label:tr("Systemdarstellung übernehmen","Follow system theme"),hint:tr("Passe Hell/Dunkel automatisch an das Betriebssystem an.","Automatically follow the operating system's light/dark theme.")},
    {key: "compactMode", label: tr("Kompakte Darstellung", "Compact layout"), hint: tr("Weniger Abstand in Chats und Navigation.", "Reduce spacing in chats and navigation.")},
    {key: "focusMode", label: tr("Fokusmodus", "Focus mode"), hint: tr("Blende die Ergebnisleiste und sekundäre Laufdetails aus.", "Hide the results rail and secondary run details.")},
    {key: "showTimestamps", label: tr("Zeitstempel anzeigen", "Show timestamps"), hint: tr("Zeige die Uhrzeit neben Chatnachrichten.", "Display the time beside chat messages.")},
    {key:"showThinking",label:tr("Thinking-Abschnitte anzeigen","Show thinking sections"),hint:tr("Zeige vorhandene Thinking-Abschnitte separat zum Aufklappen.","Display available thinking sections as separate collapsible content.")},
    {key:"confirmOnClose",label:tr("Schließen bei aktiven Läufen oder Terminals bestätigen","Confirm closing with active runs or terminals"),hint:tr("Frage vor dem Schließen des Fensters nach. Angenommene Läufe und Terminalsitzungen bleiben aktiv.","Ask before closing the window. Accepted runs and terminal sessions keep running.")},
    {key: "reduceMotion", label: tr("Bewegung reduzieren", "Reduce motion"), hint: tr("Deaktiviere Animationen und weiches Scrollen.", "Disable animations and smooth scrolling.")},
  ];
  return <section className="settings-card settings-general"><header className="settings-card-heading"><div><h2>{tr("Allgemein", "General")}</h2><p>{tr("Änderungen werden direkt auf diesem Gerät gespeichert.", "Changes are saved immediately on this device.")}</p></div></header>
    <div className="settings-field-grid"><label>{tr("Sprache", "Language")}<select value={language} onChange={event => setLanguage(event.target.value as "de" | "en")}><option value="de">Deutsch</option><option value="en">English</option></select></label><label>{tr("Darstellung", "Appearance")}<select value={theme} onChange={event => {save({followSystemTheme:false});setTheme(event.target.value as "light" | "dark");}}><option value="light">{tr("Hell", "Light")}</option><option value="dark">{tr("Dunkel", "Dark")}</option></select></label>
      <label>{tr("Chatschriftgröße", "Chat font size")}<select value={preferences.fontSize} onChange={event => save({fontSize: Number(event.target.value)})}>{[12, 13, 14, 15, 16, 18, 20, 22].map(size => <option value={size} key={size}>{size} px</option>)}</select></label><label>{tr("Nachricht senden", "Send message")}<select value={preferences.sendShortcut} onChange={event => save({sendShortcut: event.target.value as AppearancePreferences["sendShortcut"]})}><option value="enter">{tr("Enter · neue Zeile mit Umschalt+Enter", "Enter · new line with Shift+Enter")}</option><option value="ctrl_enter">{tr("Strg/⌘+Enter · neue Zeile mit Enter", "Ctrl/⌘+Enter · new line with Enter")}</option></select></label></div>
    <div className="settings-preference-list">{toggles.map(item => <label className="settings-preference" key={item.key}><span><strong>{item.label}</strong><small>{item.hint}</small></span><input type="checkbox" checked={preferences[item.key]} onChange={event => save({[item.key]: event.target.checked})}/></label>)}</div>
    {error && <p className="error" role="alert">{error}</p>}<div className="actions"><button type="button" onClick={() => save({...defaultAppearancePreferences})}>{tr("Anzeigeeinstellungen zurücksetzen", "Reset appearance settings")}</button></div>
  </section>;
}
