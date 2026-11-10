import {useEffect, useState} from "react";
import {useQueryClient} from "@tanstack/react-query";
import {useView} from "./view";
import {BrowserHostSetup} from "./BrowserHostSetup";
import {documentToolsState, isNativeDesktop, latexState, useCachedToolboxAvailability, type DocumentAvailability, type SetupStatus} from "./documentAutoSetup";

type Status = SetupStatus;
type Translate = (de: string, en: string) => string;
const tools = [{name: "python", label: "Python + Office/PDF"}, {name: "tectonic", label: "Tectonic (LaTeX)"}, {name: "chromium", label: "Chromium + Playwright"}, {name: "chrome", label: "Chrome for Testing"}, {name: "firefox", label: "Firefox + Playwright"}, {name: "browser_host", label: "Browser Extension Host"}, {name: "git", label: "Git"}, {name: "gh", label: "GitHub CLI"}, {name: "libreoffice", label: "LibreOffice"}, {name: "docker", label: "Docker"}, {name: "node", label: "Node.js"}, {name: "powershell", label: "PowerShell"}, {name: "bash", label: "Bash"}];
const time = (value: number | null | undefined, language: string) => value ? new Date(value).toLocaleString(language === "de" ? "de-DE" : "en-US", {dateStyle: "short", timeStyle: "short"}) : "";

/** Document runtime (Office/PDF/images) and offline LaTeX, installed automatically on first start. */
export function DocumentToolsStatus({status, availability, tr, language, locked, onInstall}: {status: Status | null; availability?: DocumentAvailability; tr: Translate; language: string; locked: boolean; onInstall: () => void}) {
  const state = documentToolsState(status, availability), latex = latexState(status, availability);
  const office = state === "path_too_long" ? tr("Ausführungsordner zu lang", "Execution directory too long") : state === "ready" ? availability?.documents_available === false ? tr("Installiert; wird nach dem Neuladen der Konfiguration aktiv", "Installed; active after the configuration reloads") : tr("Bereit", "Ready") : state === "installing" ? tr("Wird eingerichtet", "Being set up") : tr("Noch nicht eingerichtet", "Not set up yet");
  const tex = latex === "active" ? tr("Bereit, ohne Internet", "Ready, works offline")
    : latex === "data_path_too_long" ? tr("Der interne LaTeX-Arbeitsordner ist für Windows zu lang. Verwende einen kürzeren privaten V2-Datenpfad.", "The internal LaTeX working directory is too long for Windows. Use a shorter private V2 data path.")
    : latex === "probing" ? tr("Laufzeit wird geprüft", "Checking runtime")
    : latex === "cache_incomplete" ? tr("LaTeX-Paketcache fehlt oder passt nicht zur Laufzeit", "LaTeX package cache is missing or mismatched")
    : latex === "isolation_unavailable" ? tr("Prozessisolation ist auf diesem Ausführungsziel nicht verfügbar", "Process isolation is unavailable on this executor")
    : latex === "probe_failed" ? tr("Die Prüfung der Dokument-Laufzeit ist fehlgeschlagen", "Document runtime verification failed")
    : latex === "installed_inactive" ? tr("Installiert, vom Ausführungsziel noch nicht als bereit bestätigt", "Installed, not yet confirmed ready by the executor")
    : latex === "installing" ? tr("Wird eingerichtet", "Being set up") : tr("Noch nicht eingerichtet", "Not set up yet");
  const auto = status?.auto;
  const ocr = state === "path_too_long" ? tr("Ausführungsordner zu lang", "Execution directory too long") : availability?.ocr_available ? tr("Bereit, Deutsch und Englisch lokal", "Ready, German and English locally")
    : availability?.ocr_status === "probing" ? tr("Texterkennung wird geprüft", "Checking text recognition")
    : tr("Noch nicht bereit; Dokumentwerkzeuge einrichten oder aktualisieren", "Not ready; set up or update the document tools");
  return <section className="product-item document-tools-status"><h3>{tr("Dokumentwerkzeuge", "Document tools")}</h3>
    <p>{tr("Word, Excel, PowerPoint, PDF und Bilder sowie LaTeX werden beim ersten Start automatisch in den privaten V2-Datenordner geladen (etwa 19 MB Python-Pakete, 21 MB Tectonic und 115 MB LaTeX-Pakete und Schriften). Browser werden dabei nicht heruntergeladen.", "Word, Excel, PowerPoint, PDF, images and LaTeX are downloaded automatically into the private V2 data folder on first start (about 19 MB of Python packages, 21 MB of Tectonic and 115 MB of LaTeX packages and fonts). No browsers are downloaded.")}</p>
    <ul className="native-folder-list"><li><strong>{tr("Office, PDF und Bilder", "Office, PDF and images")}</strong><span data-state={state}>{office}</span></li><li><strong>LaTeX (Tectonic)</strong><span data-state={latex}>{tex}</span></li><li><strong>{tr("Texterkennung (OCR)","Text recognition (OCR)")}</strong><span data-state={availability?.ocr_available?"ready":"missing"}>{ocr}</span></li></ul>
    <p>{tr("Für Scans werden zusätzlich lokale OCR-Pakete und etwa 6 MB Sprachmodelle für Deutsch und Englisch eingerichtet. Die Erkennung benötigt anschließend keine Internetverbindung.","Scans additionally use local OCR packages and about 6 MB of German and English language models. Recognition then works without an internet connection.")}</p>
    {state === "installing" && <><p role="status">{status?.automatic ? tr("Automatische Einrichtung", "Automatic setup") + ": " : ""}{status?.stage}: {status?.message}</p><progress aria-label={tr("Dokumentwerkzeuge werden eingerichtet", "Setting up document tools")}/></>}
    {state === "paused" && <p role="status">{tr("Die automatische Einrichtung wurde abgebrochen und startet erst wieder, wenn du sie hier anstößt.", "The automatic setup was canceled and only resumes when you start it here.")}</p>}
    {state === "path_too_long" && <p role="alert" className="error">{tr("Der interne Dokument-Arbeitsordner ist für Windows zu lang. Verwende einen kürzeren privaten V2-Datenpfad und starte den Arbeitsbereich neu. Bei einer Serververbindung muss der Betreiber den Datenpfad anpassen.", "The internal document working directory is too long for Windows. Use a shorter private V2 data path and restart the workspace. For a server connection, the operator must adjust the data path.")}</p>}
    {state === "failed" && <p role="alert" className="error">{tr("Die Einrichtung der Dokumentwerkzeuge ist fehlgeschlagen", "Setting up the document tools failed")}{auto?.next_attempt_at ? " · " + tr("nächster automatischer Versuch", "next automatic attempt") + " " + time(auto.next_attempt_at, language) : ""}: {status?.error ?? auto?.last_error}</p>}
    {state === "waiting" && <p role="status">{tr("Nächster automatischer Versuch", "Next automatic attempt")}: {time(auto?.next_attempt_at, language)}</p>}
    {(state === "missing" || state === "failed" || state === "paused" || state === "waiting" || state === "ready" && availability?.ocr_available === false) && <button className="primary" disabled={locked} onClick={onInstall}>{state === "failed" ? tr("Dokumentwerkzeuge erneut einrichten", "Retry document tools setup") : tr("Dokumentwerkzeuge jetzt einrichten", "Set up document tools now")}</button>}
  </section>;
}

/** Compact progress line for the automatic first-start setup; renders nothing when idle. */
export function DocumentSetupBanner() {
  const language = useView(value => value.language), tr: Translate = (de, en) => language === "de" ? de : en;
  const qc = useQueryClient(); const [status, setStatus] = useState<Status | null>(null);
  useEffect(() => {
    if (!isNativeDesktop()) return; let mounted = true, wasRunning = false;
    const poll = async () => {try {const {invoke} = await import("@tauri-apps/api/core"); const next = await invoke<Status>("toolbox_setup_status"); if (!mounted) return; setStatus(next); if (wasRunning && !next.running) await qc.invalidateQueries({queryKey: ["toolbox"]}); wasRunning = next.running;} catch {/* the setup panel reports errors */}};
    void poll(); const timer = setInterval(() => void poll(), 2000); return () => {mounted = false; clearInterval(timer);};
  }, [qc]);
  if (!status?.running || status.profile !== "documents") return null;
  return <p role="status" className="document-setup-banner">{tr("Dokumentwerkzeuge werden eingerichtet", "Setting up document tools")} · {status.message}</p>;
}

export function ToolRuntimeSetup({nativeAvailable}: {nativeAvailable?: boolean} = {}) {
  const native = (nativeAvailable ?? true) && isNativeDesktop();
  const language = useView(value => value.language), tr = (de: string, en: string) => language === "de" ? de : en;
  const qc = useQueryClient(); const [status, setStatus] = useState<Status | null>(null), [error, setError] = useState<string | null>(null), [busy, setBusy] = useState(false);
  const [git, setGit] = useState(true), [gh, setGh] = useState(true), [office, setOffice] = useState(true);
  const availability = useCachedToolboxAvailability();
  async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {const {invoke} = await import("@tauri-apps/api/core"); return invoke<T>(command, args);}
  async function refresh() {setStatus(await invoke<Status>("toolbox_setup_status")); await qc.invalidateQueries({queryKey: ["toolbox"]});}
  useEffect(() => {setStatus(null); setError(null); if (!native) return; let mounted = true; void invoke<Status>("toolbox_setup_status").then(value => {if (mounted) setStatus(value);}).catch(reason => {if (mounted) setError(String(reason));}); return () => {mounted = false;};}, [native]);
  useEffect(() => {if (!native || !status?.running) return; const timer = setInterval(() => void refresh().catch(reason => setError(String(reason))), 1000); return () => clearInterval(timer);}, [native, status?.running]);
  async function action(callback: () => Promise<void>) {if (busy || !native) return; setBusy(true); setError(null); try {await invoke("connection_info"); await callback(); await refresh();} catch (reason) {setError(String(reason));} finally {setBusy(false);}}
  const locked = busy || status?.running === true;
  return <section className="product-card tool-runtime-setup"><h2>{tr("Werkzeuge einrichten", "Configure tool runtimes")}</h2>
    <p>{tr("Terminal, Browser, Office und PDF verwenden einen eingerichteten Werkzeugzugang für diesen Arbeitsbereich.", "Terminal, browser, Office and PDF use a configured tool runtime for this workspace.")}</p>
    {!native ? <><p>{tr("Die Server-Laufzeiten werden vom Betreiber bereitgestellt. Das mitgelieferte Dockerimage enthält Python mit den Dokumentpaketen, Tectonic mit vorab geladenen LaTeX-Paketen, Playwright, Chromium, LibreOffice, Git und GitHub CLI.", "The server operator provisions tool runtimes. The supplied Docker image includes Python with the document packages, Tectonic with pre-loaded LaTeX packages, Playwright, Chromium, LibreOffice, Git and GitHub CLI.")}</p><p>{tr("Die Verbindung zu den vorhandenen Serverwerkzeugen verwendest du über die Werkzeugausführung. MCP-Server verbindest du direkt im Bereich MCP & Werkzeuge.", "Use the tool execution panel to access existing server runtimes. Connect MCP servers directly in MCP & Tools.")}</p></> : <>
      <DocumentToolsStatus status={status} availability={availability} tr={tr} language={language} locked={locked} onInstall={() => void action(async () => setStatus(await invoke<Status>("toolbox_setup_start", {options: {git: false, github_cli: false, libreoffice: false, profile: "documents"}})))}/>
      <div className="actions"><button disabled={locked} onClick={() => void action(async () => {await invoke("toolbox_setup_reload");})}>{tr("Konfiguration neu laden", "Reload configuration")}</button><button disabled={locked} onClick={() => void refresh().catch(reason => setError(String(reason)))}>{tr("Status aktualisieren", "Refresh status")}</button></div>
      <ul className="native-folder-list">{tools.map(tool => <li key={tool.name}><strong>{tool.label}</strong><span className="native-folder-path">{status?.programs[tool.name] ?? tr("Noch nicht eingerichtet", "Not configured")}</span><button disabled={locked} onClick={() => void action(async () => {await invoke("toolbox_setup_select", {name: tool.name});})}>{tr("Vorhandenes Programm wählen", "Choose installed program")}</button></li>)}</ul>
      <h3>{tr("Private Laufzeit installieren", "Install a private runtime")}</h3><p>{tr("Lade zusätzlich Playwright-Pakete sowie Chromium, Chrome for Testing und Firefox in den privaten V2-Datenordner; die Dokumentwerkzeuge werden dabei mit eingerichtet. Vorhandene ausgewählte Programme werden weiterverwendet. Je nach Auswahl sind etwa 0,5–1,5 GB Speicher und eine Internetverbindung erforderlich.", "Additionally download Playwright packages plus Chromium, Chrome for Testing and Firefox into the private V2 data folder; the document tools are set up as well. Selected existing programs are reused. Depending on your selection, this requires about 0.5–1.5 GB of storage and an internet connection.")}</p>
      <label className="product-check"><input type="checkbox" checked={git} disabled={locked} onChange={event => setGit(event.target.checked)}/>{tr("Fehlendes Git privat einrichten", "Install missing Git privately")}</label>
      <label className="product-check"><input type="checkbox" checked={gh} disabled={locked} onChange={event => setGh(event.target.checked)}/>{tr("Fehlende GitHub CLI privat einrichten", "Install missing GitHub CLI privately")}</label>
      <label className="product-check"><input type="checkbox" checked={office} disabled={locked} onChange={event => setOffice(event.target.checked)}/>{tr("Fehlendes LibreOffice einschließlich Konvertierung einrichten", "Install missing LibreOffice including conversion")}</label>
      <p className="muted">{tr("Docker wird als vorhandener Betreiberzugang ausgewählt. Das Teamimage wird auf dem eigenen Server gebaut.", "Select an existing Docker installation. The team image is built on your own server.")}</p>
      <button className="primary" disabled={locked} onClick={() => void action(async () => setStatus(await invoke<Status>("toolbox_setup_start", {options: {git, github_cli: gh, libreoffice: office, profile: "full"}})))}>{status?.error && status.profile === "full" ? tr("Einrichtung erneut ausführen", "Retry setup") : tr("Private Laufzeit installieren", "Install private runtime")}</button>
      {status?.running && <><p role="status">{status.stage}: {status.message}</p><progress aria-label={tr("Werkzeuge werden eingerichtet", "Setting up tools")}/><button disabled={busy} onClick={() => void invoke("toolbox_setup_cancel").catch(reason => setError(String(reason)))}>{tr("Einrichtung abbrechen", "Cancel setup")}</button><p className="muted">{tr("Bereits heruntergeladene Dateien bleiben für die Fehlerdiagnose erhalten.", "Downloaded files are retained for diagnostics.")}</p></>}
      {status?.browser_ready && !status.running && <p role="status">{tr("Die privaten Browserwerkzeuge sind eingerichtet.", "The private browser tools are configured.")}</p>}
    </>}
    {native&&<BrowserHostSetup/>}
    {(error || status?.error) && <p role="alert" className="error">{error ?? status?.error}</p>}
  </section>;
}
