import {useEffect,useState} from "react";
import type {Client,WorkflowReviewEvidence,WorkflowReviewRequest} from "@cowork/client";
import {productDownload} from "./productApi";
import {verifiedPreviewUrl} from "./ToolPreviews";
import {reviewSourceOutput,sourceWordMatches,validReviewEvidence,type ReviewSourcePage,type ReviewSourceWord} from "./workflowReviewEvidence";
import {useView} from "./view";
import "./workflowReviewSources.css";

function SourcePage({client,run,page,word,name}:{client:Client;run:string;page:ReviewSourcePage;word:ReviewSourceWord|undefined;name:string}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [url,setUrl]=useState<string>(),[failed,setFailed]=useState(false);
  useEffect(()=>{let active=true,current:string|undefined;
    if(!page.preview_sha256)return;
    productDownload(client,`/runs/${encodeURIComponent(run)}/previews/${page.preview_sha256}`).then(blob=>verifiedPreviewUrl(blob,page.preview_sha256!)).then(value=>{current=value;if(active)setUrl(value);else URL.revokeObjectURL(value);},()=>{if(active)setFailed(true);});
    return()=>{active=false;if(current)URL.revokeObjectURL(current);};
  },[client,run,page.preview_sha256]);
  if(!page.preview_sha256)return <p>{tr("Für diese Seite ist keine gespeicherte Bildvorschau vorhanden. Der erkannte Text bleibt unten lesbar.","This page has no saved image preview. The recognized text remains available below.")}</p>;
  if(failed)return <p role="alert">{tr("Die gespeicherte Seitenvorschau konnte nicht geprüft oder angezeigt werden. Der Originaltext bleibt unten lesbar.","The saved page preview could not be verified or displayed. The original text remains available below.")}</p>;
  if(!url)return <p role="status">{tr(failed?"Die geprüfte Seitenvorschau ist nicht verfügbar.":"Seitenvorschau wird geprüft …",failed?"The verified page preview is unavailable.":"Verifying page preview …")}</p>;
  return <figure className="review-source-page"><div className="review-source-raster"><img src={url} alt={`${name} · ${tr("Seite","Page")} ${page.page}`} onError={()=>setFailed(true)}/>{word&&<svg viewBox={`0 0 ${page.width} ${page.height}`} preserveAspectRatio="none" aria-hidden="true"><rect x={word.x} y={word.y} width={word.width} height={word.height} vectorEffect="non-scaling-stroke"/></svg>}</div><figcaption>{tr("Seite","Page")} {page.page}{word?` · ${word.text}`:""} · <a href={url} target="_blank" rel="noreferrer">{tr("Geprüftes Bild öffnen","Open verified image")}</a></figcaption>{failed&&<p role="alert">{tr("Die gespeicherten Bilddaten lassen sich nicht anzeigen.","The saved image data cannot be displayed.")}</p>}</figure>;
}

export function WorkflowReviewSource({client,run,evidence}:{client:Client;run:string;evidence:WorkflowReviewEvidence}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const output=reviewSourceOutput(evidence),[query,setQuery]=useState(""),[pageNumber,setPageNumber]=useState<number>(),[selected,setSelected]=useState<string>();
  if(!output)return <article className="product-item"><h5>{evidence.name}</h5><p role="alert">{tr("Die gespeicherte Quelle kann nicht als Text- oder OCR-Ergebnis angezeigt werden. Sie bleibt im Nachweis erhalten.","The saved source cannot be displayed as a text or OCR result. It remains in the evidence.")}</p><details><summary>{tr("Gespeicherte Quelldaten","Saved source data")}</summary><pre>{JSON.stringify(evidence.output,null,2)}</pre></details></article>;
  const pages=output.pages??[],words=output.words??[],page=pages.find(p=>p.page===pageNumber)??pages[0],word=words.find(w=>`${w.page}:${w.index}`===selected&&w.page===page?.page),matches=sourceWordMatches(words,query);
  const incomplete=output.truncated||output.next_offset!==null||output.pages_truncated||output.next_page!=null||output.diagnostics_truncated;
  return <article className="product-item review-source-document"><h5>{evidence.name}</h5><p className="review-source-path">{output.source.path}</p>
    <p className="muted">{tr("Gespeichertes Originalergebnis dieses Laufs. Die Quelle wird durch eine Korrektur der Antwort nicht verändert.","Saved original result of this run. Correcting the answer does not change the source.")}</p>
    {incomplete&&<p role="status">{tr("Diese Erkennung ist ein Ausschnitt. Weitere Seiten oder Wörter müssen im Dokumentenschritt ausdrücklich gelesen werden.","This recognition is an excerpt. Read further pages or words explicitly in the document step.")}</p>}
    {!!pages.length&&<><label>{tr("Quellseite","Source page")}<select aria-label={tr("Quellseite","Source page")} value={page?.page??""} onChange={e=>{setPageNumber(Number(e.target.value));setSelected(undefined);}}>{pages.map(p=><option value={p.page} key={p.page}>{tr("Seite","Page")} {p.page}</option>)}</select></label>{page&&<SourcePage key={`${evidence.step_id}:${page.page}:${page.preview_sha256}`} client={client} run={run} page={page} word={word} name={evidence.name}/>}</>}
    <details open={evidence.kind==="text"}><summary>{tr("Erkannter Originaltext","Recognized original text")}</summary><pre className="review-source-text">{output.text}</pre></details>
    {!!words.length&&<details><summary>{tr("Wörter und Fundstellen","Words and locations")} ({words.length})</summary><label>{tr("In der Erkennung suchen","Search recognition")}<input type="search" maxLength={200} value={query} onChange={e=>setQuery(e.target.value)}/></label><p className="muted">{tr("Der OCR-Rohwert ist keine Wahrscheinlichkeit. Wähle ein Wort, um seine Fundstelle auf der gespeicherten Seite zu markieren.","The raw OCR score is not a probability. Select a word to mark its location on the saved page.")}</p>
      <div className="review-source-words"><table><thead><tr><th>{tr("Seite","Page")}</th><th>{tr("Erkanntes Wort","Recognized word")}</th><th>{tr("OCR-Rohwert","Raw OCR score")}</th></tr></thead><tbody>{matches.map(w=><tr key={`${w.page}:${w.index}`}><td>{w.page}</td><td><button type="button" aria-pressed={selected===`${w.page}:${w.index}`} onClick={()=>{setSelected(`${w.page}:${w.index}`);setPageNumber(w.page);}}>{w.text}{w.text_truncated?" …":""}</button></td><td>{w.score.toLocaleString(language,{maximumFractionDigits:2})}</td></tr>)}</tbody></table></div>{!matches.length&&<p role="status">{tr("Keine passende Fundstelle im gespeicherten Ausschnitt.","No matching location in the saved excerpt.")}</p>}
    </details>}
    {!!output.diagnostics.length&&<details><summary>{tr("Hinweise zur Erkennung","Recognition notes")}</summary><ul>{output.diagnostics.map((note,i)=><li key={i}>{note.message}</li>)}</ul></details>}
    <details><summary>{tr("Quellnachweis","Source evidence")}</summary><dl><dt>{tr("Dateiinhalt SHA-256","File content SHA-256")}</dt><dd><code>{output.source.sha256}</code></dd>{page&&<><dt>{tr("Erkennungsraster SHA-256","Recognition raster SHA-256")}</dt><dd><code>{page.raster_sha256}</code></dd></>}</dl></details>
  </article>;
}

export function WorkflowReviewSources({client,run,review}:{client:Client;run:string;review:WorkflowReviewRequest}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en,evidence=review.evidence??[];
  if(!validReviewEvidence(review))return <p role="alert">{tr("Die gespeicherten Quellen fehlen oder passen nicht zur ursprünglichen Formularauswahl. Gleiche den gespeicherten Vorgang ab, bevor du die Antwort bestätigst.","The saved sources are missing or do not match the original form selection. Reconcile the saved operation before confirming the answer.")}</p>;
  if(!evidence.length)return null;
  return <section className="workflow-review-sources" aria-label={tr("Quellen zum Vergleichen","Sources to compare")}><h4>{tr("Quellen zum Vergleichen","Sources to compare")}</h4>{evidence.map(item=><WorkflowReviewSource key={item.step_id} client={client} run={run} evidence={item}/>)}</section>;
}
