// Disposable real-daemon/browser probe. Pass a local Playwright module entry as
// COWORK_PLAYWRIGHT_MODULE when it is not installed in this checkout.
import assert from "node:assert/strict";
import {createHash} from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import net from "node:net";
import {spawn,execFileSync} from "node:child_process";
import {pathToFileURL} from "node:url";

const root=path.resolve(import.meta.dirname,"../.."),output=path.resolve(process.argv[2]??path.join(root,".local","knowledge-topics-browser-"+Date.now()));
await fs.mkdir(output,{recursive:true});
const data=await fs.mkdtemp(path.join(os.tmpdir(),"cowork-topics-"));
if(process.platform==="win32"){
  assert.deepEqual(await fs.readdir(data),[]);
  const sid=execFileSync("whoami",["/user","/fo","csv","/nh"],{encoding:"utf8",windowsHide:true}).match(/S-1-5-[0-9-]+/)?.[0];
  assert.ok(sid);
  execFileSync("icacls",[data,"/inheritance:r","/grant:r",`*${sid}:(OI)(CI)F`,"*S-1-5-18:(OI)(CI)F","*S-1-5-32-544:(OI)(CI)F"],{windowsHide:true,stdio:"pipe"});
  execFileSync("icacls",[data,"/setowner","*"+sid],{windowsHide:true,stdio:"pipe"});
}
const listener=net.createServer();await new Promise(r=>listener.listen(0,"127.0.0.1",r));const port=listener.address().port;await new Promise(r=>listener.close(r));
const base=`http://127.0.0.1:${port}`;
const env={...process.env,COWORK_DATA_DIR:data,COWORK_PORT:String(port),COWORK_WEB_DIR:path.join(root,"apps/web/dist"),RUST_LOG:"error"};
delete env.COWORK_DATABASE_URL;
const daemon=spawn(path.join(root,"target/debug",process.platform==="win32"?"cowork-daemon.exe":"cowork-daemon"),[],{cwd:root,env,windowsHide:true,stdio:["ignore","pipe","pipe"]});
let logs="",browser,token,page;
daemon.stderr.on("data",chunk=>{logs+=chunk;});
daemon.stdout.on("data",chunk=>{logs+=chunk;});
async function call(method,endpoint,body){const response=await fetch(base+"/api/v2"+endpoint,{method,headers:{Authorization:"Bearer "+token,"Content-Type":"application/json"},body:body===undefined?undefined:JSON.stringify(body)});assert.equal(response.status,200,endpoint+": "+await response.clone().text());return response.json();}
try{
  for(let i=0;i<300;i++){
    assert.equal(daemon.exitCode,null,logs);
    try{token=(await fs.readFile(path.join(data,"daemon.token"),"utf8")).trim();await call("GET","/health");break;}catch{await new Promise(r=>setTimeout(r,100));}
  }
  assert.ok(token,"Daemon did not start");
  const p=await call("POST","/projects",{name:"Wissensprüfung"});
  const {chromium}=await import(process.env.COWORK_PLAYWRIGHT_MODULE?pathToFileURL(process.env.COWORK_PLAYWRIGHT_MODULE).href:"playwright");
  browser=await chromium.launch({headless:true,channel:"chrome"});
  const context=await browser.newContext({viewport:{width:1440,height:1000}});
  page=await context.newPage();const errors=[];
  page.on("pageerror",error=>errors.push(error.message));
  await page.addInitScript(({token,base})=>{sessionStorage.setItem("cowork-v2-token",token);sessionStorage.setItem("cowork-v2-endpoint",base);sessionStorage.setItem("cowork-v2-connection-kind","server");},{token,base});
  await page.goto(base);
  await page.getByRole("button",{name:p.name,exact:true}).click();
  await page.getByRole("button",{name:"Projekt konfigurieren",exact:true}).click();
  await page.getByRole("tab",{name:"Wissensbasis",exact:true}).click();
  const panel=page.getByRole("tabpanel",{name:"Wissensbasis",exact:true});
  await panel.getByLabel("Themenordner anlegen",{exact:true}).fill("Forschung");
  await panel.getByRole("button",{name:"Ordner anlegen",exact:true}).click();
  await panel.getByLabel("Unterordner anlegen",{exact:true}).fill("Quantisierung");
  await panel.getByRole("button",{name:"Ordner anlegen",exact:true}).click();
  await panel.getByRole("heading",{name:"Forschung / Quantisierung",exact:true}).waitFor();
  const form=panel.locator(".knowledge-entry-form");
  await form.getByLabel("Titel",{exact:true}).fill("Ungespeicherter Entwurf");
  await form.getByLabel(/^Inhalt/).fill("Dieser Entwurf bleibt beim Ordnerwechsel erhalten.");
  const navigation=panel.getByRole("navigation",{name:"Wissensordner",exact:true});
  await navigation.getByRole("button",{name:/^Forschung/}).click();
  await navigation.getByRole("button",{name:/^Quantisierung/}).click();
  assert.equal(await form.getByLabel("Titel",{exact:true}).inputValue(),"Ungespeicherter Entwurf");
  assert.equal(await form.getByLabel(/^Inhalt/).inputValue(),"Dieser Entwurf bleibt beim Ordnerwechsel erhalten.");
  for(const title of ["NVFP4 Ergebnisse","Vergleichswerte"]){
    await form.getByLabel("Titel",{exact:true}).fill(title);
    await form.getByLabel(/^Inhalt/).fill("Messwerte aus der lokalen Untersuchung.");
    await form.getByLabel("Tags, durch Kommas getrennt",{exact:true}).fill("fp4");
    await form.getByRole("button",{name:"Speichern",exact:true}).click();
    await panel.getByRole("status").filter({hasText:"Eintrag gespeichert."}).waitFor();
    await panel.locator(".knowledge-entries-columns .knowledge-entry-list").getByText(title,{exact:true}).waitFor();
  }
  const initialFolders=await call("GET",`/projects/${p.id}/knowledge-folders`);
  const childFolder=initialFolders.folders.find(f=>f.name==="Quantisierung");
  const importer=panel.locator(".knowledge-import");
  await importer.locator("summary").first().click();
  await importer.getByLabel(/^Zielordner für den Import/).selectOption(childFolder.id);
  await importer.getByLabel("Titel für eingefügten Text",{exact:true}).fill("Importquelle");
  await importer.getByLabel("Text einfügen",{exact:true}).fill("NVFP4 aus einem eingefügten Quelldokument.");
  await importer.getByRole("button",{name:"Text zu den Quellen hinzufügen",exact:true}).click();
  await importer.getByLabel("Tags, durch Kommas getrennt",{exact:true}).fill("fp4");
  await importer.getByRole("button",{name:"Vorschau erstellen",exact:true}).click();
  await importer.getByRole("button",{name:"Alle Abschnitte importieren",exact:true}).click();
  await importer.getByRole("status").filter({hasText:"1 Abschnitte"}).waitFor();
  await importer.locator("summary").first().click();
  await panel.getByRole("navigation",{name:"Wissensordner",exact:true}).getByRole("button",{name:/^Forschung/}).click();
  await panel.getByRole("button",{name:"Themengraph aufbauen",exact:true}).click();
  const graph=panel.locator(".knowledge-graph");
  await graph.getByRole("status").filter({hasText:"Aktuell · 3 Einträge"}).waitFor();
  await graph.getByLabel("Graphsuche ausprobieren",{exact:true}).fill("NVFP4");
  await graph.getByRole("button",{name:"Graph durchsuchen",exact:true}).click();
  await graph.getByText("Vergleichswerte",{exact:true}).waitFor();
  assert.ok((await graph.innerText()).includes("Über Verbindung: fp4"));
  const entry=panel.locator(".knowledge-entries-columns .knowledge-entry-list>li").filter({has:page.locator("strong",{hasText:"Vergleichswerte"})});
  await entry.getByRole("button",{name:"Bearbeiten",exact:true}).click();
  await form.getByLabel(/^Inhalt/).fill("Überarbeitete Messwerte.");
  await form.getByRole("button",{name:"Speichern",exact:true}).click();
  await graph.getByRole("status").filter({hasText:"Veraltet"}).waitFor();
  await graph.getByRole("button",{name:"Graph neu aufbauen",exact:true}).click();
  await graph.getByRole("status").filter({hasText:"Aktuell"}).waitFor();
  const folders=await call("GET",`/projects/${p.id}/knowledge-folders`);
  const child=folders.folders.find(f=>f.name==="Quantisierung");
  const docs=await call("GET",`/projects/${p.id}/knowledge?limit=2000`);
  assert.equal(docs.length,3);assert.ok(docs.every(d=>d.document.folder_id===child.id));
  await page.screenshot({path:path.join(output,"desktop.png"),fullPage:true,animations:"disabled"});
  await page.setViewportSize({width:390,height:844});
  assert.ok(await panel.isVisible());
  await page.waitForFunction(()=>document.querySelector("#workspace-sidebar")?.getAttribute("aria-hidden")==="true");
  const topicBounds=await panel.locator(".knowledge-topics").boundingBox();
  assert.ok(topicBounds&&topicBounds.x>=0&&topicBounds.x+topicBounds.width<=391,"Topic panel fits mobile viewport");
  const overflow=await page.evaluate(()=>document.documentElement.scrollWidth>window.innerWidth+1);
  assert.equal(overflow,false,"Mobile horizontal overflow");
  await page.screenshot({path:path.join(output,"mobile.png"),fullPage:true,animations:"disabled"});
  await page.reload();
  await page.setViewportSize({width:1440,height:1000});
  await page.getByRole("button",{name:p.name,exact:true}).click();
  await page.getByRole("button",{name:"Projekt konfigurieren",exact:true}).click();
  await page.getByRole("tab",{name:"Wissensbasis",exact:true}).click();
  await page.getByRole("navigation",{name:"Wissensordner",exact:true}).getByRole("button",{name:/^Forschung/}).click();
  await page.locator(".knowledge-graph").getByRole("status").filter({hasText:"Aktuell · 3 Einträge"}).waitFor();
  assert.deepEqual(errors,[]);
  const sourceFiles=["crates/contracts/src/knowledge_topics.rs","crates/contracts/src/knowledge_import.rs","crates/storage/src/knowledge_topics.rs","crates/storage/src/product_context.rs","crates/storage/src/productivity.rs","crates/storage/src/productivity_archive.rs","services/daemon/src/knowledge_topic_routes.rs","services/daemon/src/lib.rs","packages/ui/src/KnowledgeTopicOrganizer.tsx","packages/ui/src/ProjectKnowledgeBase.tsx","packages/ui/src/KnowledgeImportPanel.tsx","packages/ui/src/knowledgeBase.css","packages/ui/src/knowledgeTopics.ts","contracts/api.schema.json"];
  const source_sha256=Object.fromEntries(await Promise.all(sourceFiles.map(async file=>[file,createHash("sha256").update(await fs.readFile(path.join(root,file))).digest("hex")])));
  const daemon_sha256=createHash("sha256").update(await fs.readFile(path.join(root,"target/debug",process.platform==="win32"?"cowork-daemon.exe":"cowork-daemon"))).digest("hex");
  await fs.writeFile(path.join(output,"result.json"),JSON.stringify({passed:true,source_sha256,daemon_sha256,checks:["create folder","create subfolder","retain draft while browsing folders","save notes in folder","import text into subfolder","build subtree graph","retrieve tag neighbor","invalidate edited graph","rebuild graph","verify stored locations","desktop screenshot","390px layout","reload restores tree and graph","no browser exceptions"]},null,2));
  console.log(JSON.stringify({passed:true,output}));
}catch(error){await page?.screenshot({path:path.join(output,"failure.png"),fullPage:true}).catch(()=>{});await fs.writeFile(path.join(output,"failure.txt"),String(error)+"\n"+logs+"\n"+(await page?.locator("body").innerText().catch(()=>"")??""));throw error;}
finally{
  await browser?.close();
  if(daemon.exitCode===null){daemon.kill();await new Promise(r=>daemon.once("exit",r));}
  // Retain the disposable database with the proof for investigation; no live profile is used.
  await fs.writeFile(path.join(output,"fixture-path.txt"),data);
}
