import {useEffect, useRef, useState, type FormEvent} from "react";
import {useQuery, useQueryClient} from "@tanstack/react-query";
import type {Client, CreateProvider, ProviderProfile} from "@cowork/client";
import {Plus, RefreshCw} from "lucide-react";
import {productRequest} from "./productApi";
import {useView} from "./view";

export type ProviderGeneration = {temperature?: number; timeout_ms?: number};
type ProviderSettings = {profile: ProviderProfile; generation: ProviderGeneration};
export type ProviderDraft = CreateProvider & {temperature: string; timeout: string; apiKey: string; allowNoKey: boolean; removeKey: boolean};
type ProviderUpdate = {idempotency_key: string; expected_revision: number; profile: CreateProvider; generation: ProviderGeneration; api_key?: string; remove_key?: boolean};
export type PendingProviderSave = {
  create?: {idempotency_key: string; profile: CreateProvider; api_key: string | null; explicitly_allow_no_key: boolean};
  target?: ProviderProfile;
  update?: ProviderUpdate;
  generation: ProviderGeneration;
};
type ProviderApi = <T>(path: string, method: string, body?: unknown) => Promise<T>;

/** The caller retains this operation across failures, including create followed by generation save. */
export async function completeProviderSave(api: ProviderApi, operation: PendingProviderSave): Promise<ProviderProfile> {
  if (!operation.target) {
    if (!operation.create) throw new Error("provider_save_missing_request");
    operation.target = await api<ProviderProfile>("/providers/with-credential", "POST", operation.create);
  }
  if (!operation.update && operation.create && Object.keys(operation.generation).length) {
    operation.update = {idempotency_key: crypto.randomUUID(), expected_revision: operation.target.revision,
      profile: operation.create.profile, generation: operation.generation};
  }
  if (operation.update) operation.target = (await api<ProviderSettings>(`/providers/${encodeURIComponent(operation.target.id)}`, "PUT", operation.update)).profile;
  return operation.target;
}

export async function discoverProviderModels(api: ProviderApi, draft: ProviderDraft, source?: ProviderProfile): Promise<string[]> {
  const endpoint = draft.endpoint.trim().replace(/\/+$/, "");
  const unchanged = source && source.kind === draft.kind && source.endpoint.replace(/\/+$/, "") === endpoint;
  let value: {models: string[]};
  if (unchanged && !draft.apiKey && !draft.removeKey) {
    value = await api(`/providers/${encodeURIComponent(source.id)}/models`, "GET");
  } else {
    // Never send a stored credential to a new endpoint during a draft lookup.
    const noKey = source ? draft.removeKey || !source.credential_required : draft.allowNoKey;
    if (!draft.apiKey && !noKey) throw new Error("provider_discovery_key_required");
    if (!source && draft.apiKey && draft.allowNoKey) throw new Error("provider_discovery_auth_conflict");
    value = await api("/providers/discover-models", "POST", {kind: draft.kind, endpoint,
      api_key: draft.apiKey || null, explicitly_allow_no_key: !draft.apiKey && noKey});
  }
  if (!Array.isArray(value.models) || !value.models.every(model => typeof model === "string")) throw new Error("provider_catalog_invalid");
  return value.models;
}

export function providerSettingsError(reason: unknown, language: string): string {
  const text = reason instanceof Error ? reason.message : String(reason);
  const messages: Record<string, [string, string]> = {
    provider_endpoint_not_found: ["API-Pfad nicht gefunden (HTTP 404). Prüfe die Basisadresse; OpenAI-kompatible PCAI-/SGLang-Endpunkte benötigen häufig /v1.", "API path not found (HTTP 404). Check the base URL; OpenAI-compatible PCAI/SGLang endpoints commonly require /v1."],
    provider_authentication_failed: ["API-Schlüssel abgelehnt (HTTP 401). Prüfe den Schlüssel für diesen Endpunkt.", "API key rejected (HTTP 401). Check the key for this endpoint."],
    provider_access_denied: ["Zugriff verweigert (HTTP 403). Prüfe Schlüssel, Berechtigungen und den Netzwerkzugang.", "Access denied (HTTP 403). Check the key, permissions and network access."],
    provider_model_missing: ["Der Server ist erreichbar, bietet diese Modell-ID aber nicht an. Lade die Modelle und wähle die gemeldete ID.", "The server is reachable but does not offer this model ID. Load the models and select a reported ID."],
    provider_connection_timeout: ["Zeitüberschreitung beim Modellserver. Prüfe Erreichbarkeit und VPN.", "Model server request timed out. Check connectivity and VPN."],
    provider_connection_failed: ["Keine Verbindung zum Modellserver. Prüfe Adresse, DNS, VPN und Netzwerk.", "Cannot connect to the model server. Check the address, DNS, VPN and network."],
    provider_tls_failed: ["TLS-Zertifikatsprüfung fehlgeschlagen. Prüfe die Zertifikatskette des Modellservers.", "TLS certificate verification failed. Check the model server's certificate chain."],
    provider_credential_missing: ["Für diese Verbindung fehlt der API-Schlüssel.", "This connection is missing its API key."],
    provider_rate_limited: ["Der Modellserver begrenzt Anfragen (HTTP 429). Versuche es später erneut.", "The model server is rate limiting requests (HTTP 429). Try again later."],
    provider_catalog_invalid: ["Die Antwort enthält keinen gültigen Modellkatalog. Prüfe Provider und API-Basisadresse.", "The response does not contain a valid model catalogue. Check the provider and API base URL."],
    provider_catalog_interrupted: ["Der Modellabruf wurde unterbrochen. Versuche es erneut.", "The model catalogue request was interrupted. Try again."],
    provider_catalog_too_large: ["Der Modellkatalog überschreitet die erlaubte Antwortgröße.", "The model catalogue exceeds the allowed response size."],
    provider_discovery_key_required: ["Gib für den Abruf an der neuen Adresse einen API-Schlüssel ein. Der gespeicherte Schlüssel wird nur an der gespeicherten Adresse verwendet.", "Enter an API key to query the new address. The stored key is only used at the saved address."],
    provider_discovery_auth_conflict: ["API-Schlüssel entfernen oder den Betrieb ohne Schlüssel deaktivieren.", "Remove the API key or disable the no-key option."],
  };
  return messages[text.split(" (")[0]]?.[language === "de" ? 0 : 1] ?? text;
}
const emptyDraft = (): ProviderDraft => ({name: "", kind: "ollama", endpoint: "http://127.0.0.1:11434", model: "", context_tokens: 32768,
  temperature: "", timeout: "", apiKey: "", allowNoKey: true, removeKey: false});
function fromSettings(value: ProviderSettings): ProviderDraft {
  return {...value.profile, temperature: value.generation.temperature === undefined ? "" : String(value.generation.temperature),
    timeout: value.generation.timeout_ms === undefined ? "" : String(value.generation.timeout_ms / 1000), apiKey: "", allowNoKey: !value.profile.credential_required, removeKey: false};
}
export function providerGeneration(draft: Pick<ProviderDraft, "temperature" | "timeout">): ProviderGeneration {
  const generation: ProviderGeneration = {};
  if (draft.temperature !== "") {
    const temperature = Number(draft.temperature);
    if (!Number.isFinite(temperature) || temperature < 0 || temperature > 2) throw new Error("invalid_temperature");
    generation.temperature = temperature;
  }
  if (draft.timeout !== "") {
    const timeout = Number(draft.timeout) * 1000;
    if (!Number.isSafeInteger(timeout) || timeout < 1000 || timeout > 1_200_000) throw new Error("invalid_timeout");
    generation.timeout_ms = timeout;
  }
  return generation;
}

export function ProviderSettingsPanel({client, providers}: {client: Client; providers: ProviderProfile[]}) {
  const language = useView(value => value.language), tr = (de: string, en: string) => language === "de" ? de : en;
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState("");
  const [draft, setDraft] = useState<ProviderDraft>(emptyDraft);
  const [baseline, setBaseline] = useState<ProviderDraft>(emptyDraft);
  const [source, setSource] = useState<ProviderProfile>();
  const [models, setModels] = useState<string[]>([]), [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const operation = useRef<PendingProviderSave | undefined>(undefined);
  const deletion = useRef<{idempotency_key: string; expected_revision: number} | undefined>(undefined);
  const [pending, setPending] = useState(false), [pendingDelete, setPendingDelete] = useState(false);
  const details = useQuery({queryKey: ["provider-settings", client.base, selected], queryFn: () => productRequest<ProviderSettings>(client, `/providers/${encodeURIComponent(selected)}/settings`), enabled: !!selected, retry: false});
  const dirty = JSON.stringify(draft) !== JSON.stringify(baseline);
  useEffect(() => {
    if (!details.data || details.data.profile.id !== selected || pending || dirty) return;
    const next = fromSettings(details.data); setDraft(next); setBaseline(next); setSource(details.data.profile);
  }, [details.data, selected, pending]);
  useEffect(() => {
    if (!dirty && !pending && !pendingDelete) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty, pending, pendingDelete]);
  const change = (patch: Partial<ProviderDraft>) => {
    if (["kind", "endpoint", "apiKey", "allowNoKey", "removeKey"].some(key => key in patch)) setModels([]);
    setDraft(value => ({...value, ...patch})); setNotice(""); setError("");
  };
  async function refresh() {
    await queryClient.invalidateQueries({queryKey: ["providers"]});
    await queryClient.invalidateQueries({queryKey: ["provider-settings", client.base]});
  }
  function select(id: string) {
    if (busy || pending || pendingDelete) return;
    if (dirty && !window.confirm(tr("Ungespeicherte Änderungen an dieser Modellverbindung verwerfen?", "Discard unsaved changes to this model connection?"))) return;
    setSelected(id); setSource(undefined); const next = emptyDraft(); setDraft(next); setBaseline(next); setModels([]); setNotice(""); setError(""); setConfirmDelete(false);
  }
  async function save(event?: FormEvent) {
    event?.preventDefault(); if (busy) return; setBusy(true); setError(""); setNotice("");
    try {
      if (!operation.current) {
        const profile: CreateProvider = {name: draft.name.trim(), kind: draft.kind, endpoint: draft.endpoint.trim(), model: draft.model.trim(), context_tokens: draft.context_tokens, ...(draft.capabilities?.image_input ? {capabilities: {image_input: true}} : {})};
        const generation = providerGeneration(draft);
        if (selected) {
          if (!source || source.id !== selected) throw new Error(tr("Gespeicherte Verbindung zuerst laden.", "Load the saved connection first."));
          operation.current = {target: source, generation, update: {idempotency_key: crypto.randomUUID(), expected_revision: source.revision,
            profile, generation, ...(draft.apiKey ? {api_key: draft.apiKey} : {}), ...(draft.removeKey ? {remove_key: true} : {})}};
        } else {
          if (!draft.apiKey && !draft.allowNoKey) throw new Error(tr("API-Schlüssel angeben oder den Betrieb ohne Schlüssel ausdrücklich wählen.", "Enter an API key or explicitly choose to use no key."));
          if (draft.apiKey && draft.allowNoKey) throw new Error(tr("API-Schlüssel entfernen oder den Betrieb ohne Schlüssel deaktivieren.", "Remove the API key or disable the no-key option."));
          operation.current = {generation, create: {idempotency_key: crypto.randomUUID(), profile, api_key: draft.apiKey || null, explicitly_allow_no_key: draft.allowNoKey}};
        }
      }
      setPending(true);
      const result = await completeProviderSave((path, method, body) => productRequest(client, path, method, body), operation.current);
      operation.current = undefined; setPending(false); setSelected(result.id);
      const next = fromSettings({profile: result, generation: providerGeneration(draft)}); setDraft(next); setBaseline(next); setSource(result);
      await refresh(); setNotice(tr("Modellverbindung gespeichert.", "Model connection saved."));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      // An explicit rejected request can be edited. A partially completed create retains its target.
      if (/\((400|403|404|409|422)\)$/.test(String(reason)) && (!operation.current?.create || !operation.current.target)) {operation.current = undefined; setPending(false);}
    } finally {setBusy(false);}
  }
  async function inspect(kind: "models" | "test") {
    if (busy || pending || pendingDelete || kind === "test" && (!selected || dirty)) return; setBusy(true); setError(""); setNotice("");
    try {
      const path = `/providers/${encodeURIComponent(selected)}/${kind}`;
      if (kind === "models") {
        const models = await discoverProviderModels((path, method, body) => productRequest(client, path, method, body), draft, source);
        setModels(models);
        if (models.length === 1 && !draft.model.trim()) setDraft(value => ({...value, model: models[0]}));
        setNotice(models.length ? tr(`${models.length} ${models.length === 1 ? "Modell" : "Modelle"} gefunden. Wähle ein Modell und speichere die Verbindung.`, `${models.length} ${models.length === 1 ? "model" : "models"} discovered. Select a model and save the connection.`) : tr("Der Server ist erreichbar, meldet aber keine verfügbaren Modelle.", "The server is reachable but reports no available models."));
      } else {
        const value = await productRequest<{ready: boolean}>(client, path, "POST", {});
        if (value.ready !== true) throw new Error(tr("Die Verbindung ist nicht bereit.", "The connection is not ready."));
        setNotice(tr("Modellkatalog und Modell-ID erfolgreich geprüft. Eine Chat-Anfrage wurde dabei nicht ausgeführt.", "Model catalog and model ID checked successfully. No chat request was made."));
      }
    } catch (reason) {setError(providerSettingsError(reason, language));} finally {setBusy(false);}
  }
  async function reloadSaved() {
    if (!selected || busy || pending || pendingDelete) return;
    if (dirty && !window.confirm(tr("Ungespeicherte Änderungen verwerfen und die gespeicherten Werte laden?", "Discard unsaved changes and load saved values?"))) return;
    setBusy(true); setError("");
    try {
      const result = await details.refetch();
      if (result.error) throw result.error;
      if (!result.data || result.data.profile.id !== selected) throw new Error("provider_settings_unavailable");
      const next = fromSettings(result.data); setDraft(next); setBaseline(next); setSource(result.data.profile); setNotice("");
    } catch (reason) {setError(reason instanceof Error ? reason.message : String(reason));} finally {setBusy(false);}
  }
  async function remove() {
    if (!selected || !source || busy) return; setBusy(true); setError("");
    try {
      deletion.current ??= {idempotency_key: crypto.randomUUID(), expected_revision: source.revision}; setPendingDelete(true);
      await productRequest(client, `/providers/${encodeURIComponent(selected)}`, "DELETE", deletion.current);
      deletion.current = undefined; setPendingDelete(false); setConfirmDelete(false); setSelected(""); const next = emptyDraft(); setDraft(next); setBaseline(next); await refresh(); setNotice(tr("Modellverbindung gelöscht.", "Model connection deleted."));
    } catch (reason) {setError(reason instanceof Error ? reason.message : String(reason)); if (/\((400|403|404|409|422)\)$/.test(String(reason))) {deletion.current = undefined; setPendingDelete(false);}} finally {setBusy(false);}
  }
  const managedCodex=String(providers.find(profile=>profile.id===selected)?.kind)==="codex";
  const locked = managedCodex || busy || pending || pendingDelete || !!selected && !details.data;
  return <section className="provider-settings settings-card">
    <header className="settings-card-heading"><div><h2>{tr("Modelle & Provider", "Models & providers")}</h2><p>{tr("Verbinde lokale Modelle oder einen kompatiblen Modellserver.", "Connect local models or a compatible model server.")}</p></div><button type="button" onClick={() => select("")} disabled={busy || pending || pendingDelete}><Plus size={16}/>{tr("Neue Verbindung", "New connection")}</button></header>
    <div className="settings-provider-layout"><nav className="settings-provider-list" aria-label={tr("Modellverbindungen", "Model connections")}>
      {providers.map(profile => <button type="button" key={profile.id} aria-current={selected === profile.id ? "true" : undefined} disabled={busy || pending || pendingDelete} onClick={() => select(profile.id)}><strong>{profile.name}</strong><span>{profile.model}</span><small>{String(profile.kind)==="codex"?"Codex":profile.kind === "ollama" ? "Ollama" : "OpenAI-compatible"}</small></button>)}
      {!providers.length && <p className="muted">{tr("Noch keine Verbindung eingerichtet.", "No model connections configured.")}</p>}
      <button type="button" disabled={busy || pending || pendingDelete || dirty} onClick={() => void refresh().catch(reason => setError(String(reason)))}><RefreshCw size={14}/>{tr("Aktualisieren", "Refresh")}</button>
    </nav><form onSubmit={event => void save(event)} className="settings-provider-form">
      <h3>{selected ? tr("Verbindung bearbeiten", "Edit connection") : tr("Verbindung hinzufügen", "Add connection")}</h3>
      {selected && details.isPending && <p role="status">{tr("Verbindung wird geladen…", "Loading connection…")}</p>}
      {details.isError && <p role="alert" className="error">{String(details.error)}</p>}
      <fieldset disabled={locked} hidden={managedCodex}><div className="settings-field-grid">
        <label>{tr("Name", "Name")}<input value={draft.name} required maxLength={100} onChange={event => change({name: event.target.value})}/></label>
        <label>{tr("Provider", "Provider")}<select value={draft.kind} onChange={event => change({kind: event.target.value as CreateProvider["kind"], endpoint: event.target.value === "ollama" ? "http://127.0.0.1:11434" : "https://openrouter.ai/api/v1", allowNoKey: event.target.value === "ollama"})}><option value="ollama">Ollama</option><option value="openai_compatible">OpenAI / OpenRouter / vLLM</option></select></label>
      </div><label>{tr("Serveradresse", "Endpoint")}<input type="url" value={draft.endpoint} required onChange={event => change({endpoint: event.target.value})}/></label>
      {draft.kind === "openai_compatible" && <p className="muted">{tr("API-Basisadresse einschließlich Pfad, z. B. https://server/v1. Bei PCAI kann der direkte Modellserver eine andere Modell-ID als LiteLLM verwenden.", "API base URL including its path, e.g. https://server/v1. A direct PCAI model server may use a different model ID from LiteLLM.")}</p>}
      <label>{tr("Modell-ID", "Model ID")}<input list="settings-provider-models" value={draft.model} required maxLength={200} placeholder={tr("Über „Modelle laden“ auswählen oder manuell eingeben", "Discover models or enter an ID manually")} onChange={event => change({model: event.target.value})}/><datalist id="settings-provider-models">{models.map(model => <option key={model} value={model}/>)}</datalist></label>
      {models.length > 0 && <label>{tr("Modelle von der API", "Models from the API")}<select value={models.includes(draft.model) ? draft.model : ""} onChange={event => change({model: event.target.value})}><option value="" disabled>{tr("Modell auswählen…", "Select a model…")}</option>{models.map(model => <option key={model} value={model}>{model}</option>)}</select></label>}
      <div className="settings-field-grid"><label>{tr("Kontextfenster (Tokens)", "Context window (tokens)")}<input type="number" min={1024} max={2000000} value={draft.context_tokens} required onChange={event => change({context_tokens: Number(event.target.value)})}/></label>
        <label>{tr("Temperatur", "Temperature")}<input type="number" min={0} max={2} step={0.1} placeholder={tr("Providerstandard", "Provider default")} value={draft.temperature} onChange={event => change({temperature: event.target.value})}/></label>
        <label>{tr("Zeitlimit (Sekunden)", "Timeout (seconds)")}<input type="number" min={1} max={1200} step={1} placeholder={tr("Standard", "Default")} value={draft.timeout} onChange={event => change({timeout: event.target.value})}/></label></div>
      <label className="settings-checkbox"><input type="checkbox" checked={!!draft.capabilities?.image_input} onChange={event => change({capabilities: event.target.checked ? {image_input: true} : undefined})}/>{tr("Bildeingabe (Vision): Das Modell sieht Bilder und Vorschauen erzeugter Dokumente. Nur für Modelle mit Bildverständnis aktivieren.", "Image input (vision): the model sees images and previews of generated documents. Enable only for models that understand images.")}</label>
      <label>{selected ? tr("API-Schlüssel ersetzen", "Replace API key") : tr("API-Schlüssel", "API key")}<input type="password" autoComplete="new-password" value={draft.apiKey} disabled={draft.removeKey} onChange={event => change({apiKey: event.target.value})}/></label>
      {selected ? <><p className="muted">{details.data?.profile.credential_ready ? tr("Zugang bereit. Ein leeres Feld behält den gespeicherten Schlüssel.", "Credentials ready. Leave the field empty to keep the saved key.") : tr("Für diese Verbindung ist kein gültiger Schlüssel hinterlegt.", "This connection has no ready credential.")}</p><label className="settings-checkbox"><input type="checkbox" checked={draft.removeKey} onChange={event => change({removeKey: event.target.checked, apiKey: ""})}/>{tr("Gespeicherten Schlüssel entfernen", "Remove stored key")}</label></> : <label className="settings-checkbox"><input type="checkbox" checked={draft.allowNoKey} onChange={event => change({allowNoKey: event.target.checked})}/>{tr("Verbindung ausdrücklich ohne API-Schlüssel verwenden", "Explicitly use this connection without an API key")}</label>}
      </fieldset>
      {managedCodex&&<p role="status">{tr("Diese Verbindung ist eine feste Kopie von Codex-Konto, Modell und Denkaufwand. Weitere Modellprofile und den Kontostatus verwaltest du im Codex-Reiter. Entfernen sperrt dieses Profil für neue Läufe.","This connection is a frozen copy of its Codex account, model and reasoning effort. Manage additional model profiles and account status in the Codex tab. Removing it blocks this profile for new runs.")}</p>}
      {pending && <p role="status">{tr("Die Bestätigung dieser Speicherung steht noch aus. Ein erneuter Versuch verwendet denselben Vorgang.", "This save has not been confirmed. Retrying uses the same operation.")}</p>}
      {error && <p role="alert" className="error">{error}</p>}{notice && <p role="status" className="settings-notice">{notice}</p>}
      <div className="actions"><button type="button" disabled={locked || !draft.endpoint.trim()} onClick={() => void inspect("models")}>{busy ? tr("Verbindung wird geprüft…", "Checking connection…") : tr("Modelle laden", "Discover models")}</button><button type="submit" className="primary" disabled={managedCodex||busy || pendingDelete || !!selected && !details.data}>{pending ? tr("Speicherung erneut bestätigen", "Retry same save") : tr("Speichern", "Save")}</button>{selected && <><button type="button" disabled={locked || dirty} onClick={() => void inspect("test")}>{tr("Verbindung testen", "Test connection")}</button><button type="button" disabled={busy||pending||pendingDelete} onClick={() => setConfirmDelete(true)}>{tr("Löschen", "Delete")}</button></>}</div>
      <p className="muted">{tr("Modelle laden benötigt nur Serveradresse und Zugangsdaten. Wähle anschließend eine Modell-ID aus der API-Liste oder gib sie manuell ein.", "Discover models needs only the endpoint and credentials. Then select a model ID from the API list or enter it manually.")}</p>
      {selected && <><p className="muted">{tr("Der Verbindungstest prüft den Modellkatalog und die gespeicherte Modell-ID. Die Textgenerierung wird erst beim Start einer Anfrage geprüft.", "The connection test checks the catalog and saved model ID. Text generation is checked when you start a request.")}</p><button type="button" disabled={busy || pending || pendingDelete} onClick={() => void reloadSaved()}>{tr("Gespeicherte Werte laden", "Reload saved values")}</button></>}
      {confirmDelete && <section className="settings-confirm"><p>{pendingDelete ? tr("Die Löschung ist noch nicht bestätigt. Rufe denselben Vorgang erneut ab.", "Deletion is unconfirmed. Retrieve the same operation again.") : tr("Diese Modellverbindung für neue Läufe entfernen? Frühere Laufbelege bleiben erhalten.", "Remove this model connection from new runs? Historical run records remain retained.")}</p><div className="actions"><button type="button" disabled={busy || pendingDelete} onClick={() => setConfirmDelete(false)}>{tr("Abbrechen", "Cancel")}</button><button type="button" disabled={busy} onClick={() => void remove()}>{pendingDelete ? tr("Löschung erneut bestätigen", "Retry same deletion") : tr("Verbindung löschen", "Delete connection")}</button></div></section>}
    </form></div>
  </section>;
}
