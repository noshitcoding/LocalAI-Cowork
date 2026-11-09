import {describe,it,expect} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {QueryClient,QueryClientProvider} from "@tanstack/react-query";
import {Client,decode} from "@cowork/client";
import fixture from "../../../tests/fixtures/workflow-iteration.json";
import {WorkflowIterationsEditor} from "./WorkflowIterationsEditor";
import {WorkflowDataflowEditor} from "./WorkflowDataflowEditor";
import {WorkflowDefinitionSummary} from "./WorkflowLibrary";
import {de} from "./i18n";
import {restoreWorkflowEditor,workflowEditorFormat} from "./workflowJournal";

describe("list workflow editing",()=>{
  it("renders frozen entry mappings, active item limit and shared budgets",()=>{
    const value=decode("WorkflowDefinition",structuredClone(fixture));
    const client=new Client("http://local.invalid","owned-fixture"),query=new QueryClient({defaultOptions:{queries:{retry:false}}});
    const html=renderToStaticMarkup(<QueryClientProvider client={query}><WorkflowIterationsEditor client={client} project="project" value={value} onChange={()=>{}}/></QueryClientProvider>);
    expect(html).toContain("Listen verarbeiten");expect(html).toContain("Vorlage für Listeneinträge");
    expect(html).toContain("Eingaben pro Eintrag");expect(html).toContain("Aktueller Listeneintrag");
    expect(html).toContain("Gleichzeitig aktive Einträge");expect(html).toContain("Gemeinsame Obergrenzen dieser Liste");
    expect(html).toContain("Amount body");expect(html).toContain("Listenablauf entfernen");
    query.clear();
  });
  it("keeps the list output computed in the data editor and locks removal of typed data",()=>{
    const value=decode("WorkflowDefinition",structuredClone(fixture));
    const html=renderToStaticMarkup(<WorkflowDataflowEditor value={value} onChange={()=>{}}/>);
    expect(html).toContain("Each value");expect(html).toContain("Ergebnistyp des Kontrollbausteins");
    expect(html).toContain("disabled=\"\">Typisierte Bindungen aus diesem Entwurf entfernen");
    expect(restoreWorkflowEditor({editor_format:workflowEditorFormat,data:value,origin:null,archived:false})).toEqual(value);
    expect(restoreWorkflowEditor({editor_format:workflowEditorFormat,data:{...value,format_version:6},origin:null,archived:false})).toBeNull();
    const summary=renderToStaticMarkup(<WorkflowDefinitionSummary definition={value} providers={[]} t={key=>de[key]}/>);
    expect(summary).toContain("Listenablauf");expect(summary).toContain("Rückgabe pro Eintrag");
    expect(summary).toContain("Gleichzeitig aktive Einträge");expect(summary).toContain("Amount body");
  });
});
