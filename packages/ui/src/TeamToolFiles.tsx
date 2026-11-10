import {useTeamText} from "./teamStrings";
import {useState, type FormEvent} from "react";
import {Client} from "@cowork/client";
import {deliverDownload} from "./download";
import {useView} from "./view";

export function TeamToolFiles({client, session, team, project, order}:{client:Client, session:string, team:string, project:string, order:string}) {
  const tr=useTeamText();
  const [busy,setBusy]=useState(false); const [status,setStatus]=useState(""); const [error,setError]=useState<unknown>();
  const language=useView(v=>v.language);
  const url=(path:string)=>client.base+`/api/v2/team-tool-files/${team}/${project}/${order}?path=${encodeURIComponent(path)}`;
  async function transfer(event:FormEvent<HTMLFormElement>, upload:boolean) {
    event.preventDefault(); const data=new FormData(event.currentTarget); const file=data.get("file");
    const path=String(data.get("path")||(file instanceof File?file.name:""));
    setBusy(true);setError(undefined);setStatus("");
    try {
      if(upload&&(!(file instanceof File)||file.size>16*1024*1024))throw new Error(tr("Bitte eine Datei bis 16 MiB auswählen."));
      const response=await fetch(url(path),{method:upload?"PUT":"GET",headers:{Authorization:"Bearer "+session,...(upload?{"Content-Type":"application/octet-stream"}:{})},body:upload&&file instanceof File?file:undefined,cache:"no-store"});
      if(!response.ok){let code=tr("Dateiübertragung fehlgeschlagen");try{const body=await response.json();code=String(body.error??code);}catch{}throw new Error(`${code} (${response.status})`);}
      if(upload)setStatus(tr("Gespeichert: ")+path);
      else {await deliverDownload(await response.blob(),path.split(/[\\/]/).at(-1)||"ergebnis",language);setStatus(tr("Datei bereitgestellt."));}
    } catch(value){setError(value);} finally {setBusy(false);}
  }
  return <details><summary>{tr("Arbeitsdateien des Teamauftrags")}</summary><p className="muted">{tr("Office-Eingaben hochladen und erzeugte Dateien oder Browseraufnahmen herunterladen. Während einer Werkzeugausführung sind die Arbeitsdateien gesperrt.")}</p><form onSubmit={event=>void transfer(event,true)}><label>{tr("Datei")}<input name="file" type="file" required/></label><label>{tr("Relativer Zielpfad")}<input name="path" placeholder={tr("eingabe.docx")}/></label><button disabled={busy||!order}>{tr("Hochladen")}</button></form><form onSubmit={event=>void transfer(event,false)}><label>{tr("Relativer Dateipfad")}<input name="path" required placeholder={tr("ergebnis.pdf")}/></label><button disabled={busy||!order}>{tr("Herunterladen")}</button></form>{status&&<p role="status">{status}</p>}{!!error&&<p className="error" role="alert">{error instanceof Error?error.message:String(error)}</p>}</details>;
}
