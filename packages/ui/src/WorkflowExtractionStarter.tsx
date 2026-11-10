import {useState} from "react";
import {defaultRunBudgets,type ProviderProfile,type WorkflowDefinition,type WorkflowReviewForm} from "@cowork/client";
import {WorkflowReviewFormEditor} from "./WorkflowReviewForm";
import {completeWorkflowBudgets} from "./workflowUi";
import {createExtractionWorkflow,ExtractionDraftError,extractionSourceId,invoiceExtractionForm,type ExtractionModelSelection,type ExtractionSourceKind} from "./workflowExtraction";

export function WorkflowExtractionStarter({providers,language,disabled,onCreate}:{providers:ProviderProfile[];language:"de"|"en";disabled:boolean;onCreate:(definition:WorkflowDefinition)=>void}){
  const tr=(de:string,en:string)=>language==="de"?de:en;
  const [name,setName]=useState(()=>tr("Rechnung auslesen und prüfen","Extract and review an invoice")),[source,setSource]=useState<ExtractionSourceKind>("ocr"),[selected,setSelected]=useState("");
  const [form,setForm]=useState<WorkflowReviewForm>(()=>invoiceExtractionForm(language)),[error,setError]=useState("");
  const modelAvailable=providers.some(profile=>selected===JSON.stringify({id:profile.id,revision:profile.revision}));
  function create(){
    if(disabled)return;
    try{
      let model:ExtractionModelSelection;
      try{model=JSON.parse(selected) as ExtractionModelSelection;}catch{throw new ExtractionDraftError("model");}
      onCreate(createExtractionWorkflow({name,language,source,form,model,providers,budgets:completeWorkflowBudgets(defaultRunBudgets)}));setError("");
    }catch(reason){
      setError(reason instanceof ExtractionDraftError&&reason.code==="model"?tr("Wähle eine verfügbare Modellrevision ausdrücklich aus.","Explicitly select an available model revision."):tr("Prüfe Name, Feldtypen, Quellen und Summenregeln des Entwurfs.","Check the draft name, field types, sources and total rules."));
    }
  }
  return <details className="workflow-editor-card"><summary>{tr("Dokument auslesen und prüfen","Extract and review a document")}</summary>
    <p>{tr("Dokument lesen → typisierte Modellvorschläge → menschliche Prüfung. Die Quelle stammt aus der bestätigten Arbeitskopie. Erst die ausdrücklich bestätigten Angaben werden zum Ergebnis des Prüfschritts.","Read document → typed model proposals → human review. The source comes from the confirmed working copy. Only explicitly confirmed values become the review step's result.")}</p>
    <fieldset disabled={disabled}>
      <label>{tr("Name des Dokumentablaufs","Document workflow name")}<input required maxLength={200} value={name} onChange={e=>setName(e.target.value)}/></label>
      <label>{tr("Dokument lesen mit","Read the document using")}<select value={source} onChange={e=>setSource(e.target.value as ExtractionSourceKind)}><option value="ocr">{tr("OCR für Scan oder Bild (Deutsch/Englisch)","OCR for a scan or image (German/English)")}</option><option value="read_text">{tr("Vorhandenen Dokumenttext auslesen","Read existing document text")}</option></select></label>
      <p className="muted">{tr("OCR liefert einen begrenzten Ausschnitt mit bis zu 100 Wörtern. Weitere Wörter und Seiten müssen im Dokumentenschritt ausdrücklich gelesen werden. Unvollständige Erkennung und fehlende Werte bleiben sichtbar.","OCR returns a bounded excerpt of up to 100 words. Read further words and pages explicitly in the document step. Incomplete recognition and missing values remain visible.")}</p>
      <label>{tr("Modell für die Extraktion","Model for extraction")}<select required value={selected} onChange={e=>{setSelected(e.target.value);setError("");}}><option value="">{tr("Modell ausdrücklich auswählen","Explicitly select a model")}</option>{selected&&!modelAvailable&&<option value={selected} disabled>{tr("Gewählte Modellrevision nicht mehr verfügbar","Selected model revision is no longer available")}</option>}{providers.map(profile=><option key={profile.id+":"+profile.revision} value={JSON.stringify({id:profile.id,revision:profile.revision})}>{profile.name} · {profile.model} · {tr("Revision","revision")} {profile.revision}</option>)}</select></label>
      {!providers.length&&<p role="status">{tr("Richte zuerst ein Modell unter Einstellungen → Teams & Tools ein.","Configure a model in Settings → Teams & Tools first.")}</p>}
      <p className="muted">{tr("Das Modell bekommt die ausdrücklich gebundenen Textfelder und besitzt in diesem Entwurf keine Werkzeug- oder Ordnerrechte. Fehlende oder unklare Angaben bleiben null, bis eine Person sie ergänzt.","The model receives explicitly bound text fields and has no tool or folder permissions in this draft. Missing or ambiguous values remain null until a person completes them.")}</p>
      <p>{tr("Zu prüfende Angaben: ","Values to review: ")}{Object.values(form.fields).map(field=>field.label).join(" · ")}</p>
      <details className="extraction-field-settings"><summary>{tr("Felder und Prüfregeln anpassen","Customize fields and review rules")}</summary><WorkflowReviewFormEditor form={form} onChange={setForm} sources={[{id:extractionSourceId,name:tr("Originalquelle lesen","Read original source")}]} /></details>
      <p>{tr("Im Ablaufeditor kannst du Schritte, Felder und Datenverbindungen anpassen. Änderungen am Formular und am Extraktionsergebnis müssen zusammenpassen. Die Quelldatei muss beim Start oder über einen Dateiauslöser verbunden werden. Veröffentlichen und Starten bleiben eigene Schritte.","Adjust steps, fields and data bindings in the workflow editor. Changes to the form and extraction result must match. Connect the source file at start or through a file trigger. Publication and execution remain separate steps.")}</p>
      <button type="button" disabled={!name.trim()||!modelAvailable} onClick={create}>{tr("Dokumentablauf als Entwurf öffnen","Open document workflow as a draft")}</button>
    </fieldset>{error&&<p role="alert">{error}</p>}
  </details>;
}
