import {useTeamText} from "./teamStrings";
import {useState,type FormEvent} from "react";
import {Client} from "@cowork/client";
import {downloadToolboxFile} from "./toolboxWorkbench";
import {deliverDownload} from "./download";
import {useView} from "./view";

export function ToolboxFiles({client,order}:{client:Client,order:string}){
  const tr=useTeamText();
  const [busy,setBusy]=useState(false);const [error,setError]=useState<unknown>();const [status,setStatus]=useState("");const language=useView(v=>v.language);
  async function upload(event:FormEvent<HTMLFormElement>){event.preventDefault();const data=new FormData(event.currentTarget);const file=data.get("file");if(!(file instanceof File))return;setBusy(true);setError(undefined);try{if(file.size>16*1024*1024)throw new Error(tr("Dateien dürfen höchstens 16 MiB groß sein."));const path=String(data.get("path")||file.name);const response=await fetch(client.base+`/api/v2/orders/${order}/toolbox/files?path=${encodeURIComponent(path)}`,{method:"PUT",headers:{Authorization:"Bearer "+client.token,"Content-Type":"application/octet-stream"},body:file});if(!response.ok)throw new Error("upload_failed ("+response.status+")");setStatus(tr("Gespeichert: ")+path);}catch(e){setError(e);}finally{setBusy(false);}}
  async function download(event:FormEvent<HTMLFormElement>){event.preventDefault();const path=String(new FormData(event.currentTarget).get("path")??"");setBusy(true);setError(undefined);try{const file=await downloadToolboxFile(client,order,path);await deliverDownload(file.blob,file.file_name,language,file);setStatus(tr("Datei bereitgestellt."));}catch(e){setError(e);}finally{setBusy(false);}}
  return <section><h3>{tr("Arbeitsdateien der Werkzeuge")}</h3><p className="muted">{tr("Office-Eingaben hochladen und erzeugte Dateien oder Browseraufnahmen über ihren relativen Pfad herunterladen.")}</p><form onSubmit={upload}><label>{tr("Datei")}<input type="file" name="file" required/></label><label>{tr("Relativer Zielpfad")}<input name="path" placeholder={tr("eingabe.docx")}/></label><button disabled={busy}>{tr("Hochladen")}</button></form><form onSubmit={download}><label>{tr("Relativer Dateipfad")}<input name="path" required placeholder={tr("ergebnis.pdf")}/></label><button disabled={busy}>{tr("Herunterladen")}</button></form>{status&&<p role="status">{status}</p>}{!!error&&<p className="error" role="alert">{error instanceof Error?error.message:String(error)}</p>}</section>;
}
