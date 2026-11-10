import {useId,useState} from "react";
import type {Client} from "@cowork/client";
import {useDefaultExecutionMode,type DefaultExecutionMode} from "./executionDefaults";
import {useView} from "./view";

export function ExecutionDefaultsPanel({client}:{client:Pick<Client,"base">}){
  const language=useView(value=>value.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const {mode,setMode}=useDefaultExecutionMode(client),[error,setError]=useState("");
  const description=useId();
  return <section className="settings-card">
    <header className="settings-card-heading"><div><h2>{tr("Standard-Ausführung","Default execution")}</h2><p>{tr("Lege fest, wie neue Aufträge ihre Arbeitskopie verarbeiten. Die Auswahl wird auf diesem Gerät für den verbundenen Arbeitsbereich gespeichert.","Choose how new tasks process their working copy. This choice is saved on this device for the connected workspace.")}</p></div></header>
    <label>{tr("Standard-Ausführungsart","Default execution mode")}<select value={mode} aria-describedby={description} onChange={event=>{try{setMode(event.target.value as DefaultExecutionMode);setError("");}catch{setError(tr("Die Standard-Ausführungsart konnte nicht gespeichert werden.","The default execution mode could not be saved."));}}}>
      <option value="broker_controller">{tr("Dateien und Text","Files and text")}</option>
      <option value="document_engine">{tr("Dokumente und Bilder","Documents and images")}</option>
      <option value="ask">{tr("Bei jedem Auftrag fragen","Ask for each task")}</option>
    </select></label>
    <div id={description} className="muted">
      <p>{mode==="document_engine"?tr("Die Dokumentengine verarbeitet die gewählten Dateikopien mit Dokumentprogrammen unter deinen Betriebssystem- und Netzwerkrechten. Die Ordnerauswahl ist keine Betriebssystemisolation.","The document engine processes the selected file copies using document programs under your operating system and network permissions. Folder selection is not operating system isolation."):mode==="ask"?tr("Wähle die Ausführungsart vor jeder Planprüfung im Auftrag.","Choose the execution mode in each task before reviewing its plan."):tr("Begrenzte Datei- und Artefaktwerkzeuge ohne allgemeine Programme. Benötigt ein Workflow die Dokumentengine, wählst du sie für diesen Auftrag gesondert.","Bounded file and artifact tools without general programs. If a workflow requires the document engine, select it separately for that task.")}</p>
      <p>{tr("Vor jedem Start prüfst und bestätigst du weiterhin den genauen Plan, die Modellanbieter und Rechte. Bereits vorbereitete Aufträge behalten ihre Ausführungsart.","Before each start, you still review and confirm the exact plan, model providers and permissions. Already prepared tasks retain their execution mode.")}</p>
    </div>
    {error&&<p role="alert" className="error">{error}</p>}
  </section>;
}
