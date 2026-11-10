import {describe,expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client} from "@cowork/client";
import {LibraryDefinitionEditor} from "./AgentLibrary";
import {emptyAgentTemplate,emptyPersonality} from "./libraryApi";
import {en} from "./i18n";
describe("agent library editors",()=>{
  it("renders the complete personality authoring fields and default control",()=>{
    const markup=renderToStaticMarkup(<LibraryDefinitionEditor kind="personalities" value={emptyPersonality()} setValue={()=>{}} providers={[]} personalities={[]} client={new Client("owner")} project="" disabled={false} t={key=>en[key]}/>);
    for(const field of ["Name","Symbol","Beschreibung","Rolle","Ziel / Prompt-Fokus","Systemprompt / Anweisungen","skills.md","Modell &amp; Anbieter","Temperatur","Zeitlimit","Als Standardpersönlichkeit speichern"])expect(markup).toContain(field);
    expect(markup).toContain('min="0"');expect(markup).toContain('min="1000"');expect(markup).not.toContain("Agent templates are coming soon");
  });
  it("renders runtime settings, permission modes and bindings for a full agent template",()=>{
    const value=emptyAgentTemplate();value.name="Editor";
    const markup=renderToStaticMarkup(<LibraryDefinitionEditor kind="agent-templates" value={value} setValue={()=>{}} providers={[]} personalities={[]} client={new Client("owner")} project="" actor={true} disabled={false} t={key=>en[key]}/>);
    for(const field of ["Gespeicherte Persönlichkeit","Aktionsfreigaben","Delegation erlauben","MCP-Zugriff","Skill-IDs","Memory-IDs","read_only","confirm_mutations","confirm_all","autonomous","execute_program","browser","office","github"])expect(markup).toContain(field);
    expect(markup).toContain(en.runBudgets);expect(markup).not.toContain("Save as default personality");
  });
});
