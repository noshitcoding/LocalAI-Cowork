//! Actor-session metadata is transient; only reviewed native source and immutable requests persist.
use super::model_registry::{
    RegistryGate, bundle_identity, confirm, journal_named, publish, read_selected,
};
use super::native_client::{NativeClient, failure};
use cowork_contracts::{
    ActorModelContext, ActorModelRegistryView, ApiError, ImportActorModelQualification,
    RevokeActorModelQualification, UiLanguage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use zeroize::Zeroizing;

const MAX_SOURCE: u64 = 60 * 1024 * 1024;
const MAX_RECORD: u64 = 64 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorImportSelection {
    idempotency_key: String,
    project_id: String,
    profile_id: String,
    actor_session: Zeroizing<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorRevokeSelection {
    request: RevokeActorModelQualification,
    actor_session: Zeroizing<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenImport {
    format: u32,
    request_sha256: String,
    request: ImportActorModelQualification,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptedImport {
    format: u32,
    request_sha256: String,
    response: ActorModelRegistryView,
}
fn uuid(value: &str) -> Result<(), ApiError> {
    if uuid::Uuid::parse_str(value).is_ok_and(|v| !v.is_nil() && v.to_string() == value) {
        Ok(())
    } else {
        Err(failure("invalid_request"))
    }
}
fn session(value: &str) -> Result<reqwest::header::HeaderValue, ApiError> {
    if value.is_empty() || value.len() > 4096 {
        return Err(failure("forbidden"));
    }
    let mut header =
        reqwest::header::HeaderValue::from_str(value).map_err(|_| failure("forbidden"))?;
    header.set_sensitive(true);
    Ok(header)
}
fn validate_context(value: &ActorModelContext) -> Result<(), ApiError> {
    uuid(&value.actor_id)?;
    uuid(&value.project_id)?;
    uuid(&value.provider.id)?;
    if value.project_revision <= 0
        || value.provider.revision <= 0
        || value.provider_binding["id"] != value.provider.id
        || value.provider_binding["owner_user_id"] != value.actor_id
        || value.provider_binding["revision"] != value.provider.revision
        || value.provider_binding["endpoint"] != value.provider.endpoint
        || !super::update_journal::valid_sha(
            value.provider_binding["sha256"].as_str().unwrap_or(""),
        )
    {
        return Err(failure("model_registry_receipt_invalid"));
    }
    Ok(())
}
fn validate_frozen(
    value: &FrozenImport,
    input: &ActorImportSelection,
    actor: &str,
) -> Result<(), ApiError> {
    validate_context(&value.request.context)?;
    if value.format != 1
        || value.request.idempotency_key != input.idempotency_key
        || value.request.context.actor_id != actor
        || value.request.context.project_id != input.project_id
        || value.request.context.provider.id != input.profile_id
        || !super::update_journal::valid_sha(&value.request.source_sha256)
        || cowork_domain::canonical_hash(&value.request) != value.request_sha256
    {
        return Err(failure("model_registry_receipt_invalid"));
    }
    bundle_identity(&value.request.qualification_bundle)?;
    Ok(())
}
fn validate_reply(frozen: &FrozenImport, value: &ActorModelRegistryView) -> Result<(), ApiError> {
    let b = &frozen.request.qualification_bundle;
    let (id, version) = bundle_identity(b)?;
    let q = &value.qualification;
    if value.request_sha256 != frozen.request_sha256
        || value.source_sha256 != frozen.request.source_sha256
        || cowork_domain::canonical_hash(&value.context)
            != cowork_domain::canonical_hash(&frozen.request.context)
        || q.binding.id != id
        || q.binding.version != version
        || !super::update_journal::valid_sha(&q.binding.sha256)
        || q.provider_id != value.context.provider.id
        || q.provider_revision != value.context.provider.revision
        || q.provider_sha256 != b["provider_sha256"].as_str().unwrap_or("")
        || q.provider_sha256 != cowork_domain::canonical_hash(&value.context.provider)
        || q.deployment_sha256 != b["evidence"]["deployment_sha256"].as_str().unwrap_or("")
        || q.model_sha256
            != b["qualification"]["model"]["model_sha256"]
                .as_str()
                .unwrap_or("")
        || q.tokenizer_sha256
            != b["qualification"]["model"]["tokenizer_sha256"]
                .as_str()
                .unwrap_or("")
        || q.chat_template_sha256
            != b["qualification"]["model"]["chat_template_sha256"]
                .as_str()
                .unwrap_or("")
        || q.evidence_sha256
            != b["qualification"]["parity_evidence_sha256"]
                .as_str()
                .unwrap_or("")
        || q.context_tokens != u64::from(value.context.provider.context_tokens)
        || q.wire_mode != "openai_completions"
        || q.reference_implementation
            != b["evidence"]["reference_implementation"]
                .as_str()
                .unwrap_or("")
    {
        return Err(failure("model_registry_operation_uncertain"));
    }
    Ok(())
}
async fn connection(window: &tauri::WebviewWindow) -> Result<NativeClient, ApiError> {
    if !super::application_window(window) {
        return Err(failure("forbidden"));
    }
    super::connection_info(
        window.app_handle().clone(),
        window.state::<tokio::sync::Mutex<()>>(),
    )
    .await
    .map_err(|_| failure("model_registry_connection_failed"))?;
    NativeClient::new(
        window,
        "model_registry_connection_failed",
        "model_registry_operation_uncertain",
    )
}
#[tauri::command]
pub async fn native_actor_model_registry_import(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, RegistryGate>,
    input: ActorImportSelection,
    language: Option<UiLanguage>,
) -> Result<Option<ActorModelRegistryView>, ApiError> {
    uuid(&input.idempotency_key)?;
    uuid(&input.project_id)?;
    uuid(&input.profile_id)?;
    let credential = session(&input.actor_session)?;
    let _gate = gate
        .0
        .try_lock()
        .map_err(|_| failure("model_registry_busy"))?;
    let client = connection(&window).await?;
    // Even a locally cached original receipt remains private to the currently
    // authenticated actor and the actual private project's live authority.
    let authority: Value = client
        .result(
            client
                .request(
                    reqwest::Method::GET,
                    &format!(
                        "/native-actor-model-registry/authority?project_id={}",
                        input.project_id
                    ),
                )
                .header("x-cowork-actor-session", credential.clone()),
            false,
        )
        .await?;
    let actor = authority["actor_id"]
        .as_str()
        .ok_or_else(|| failure("forbidden"))?;
    uuid(actor)?;
    if authority["project_id"] != input.project_id {
        return Err(failure("forbidden"));
    }
    let data = super::private_data_dir(window.app_handle())
        .map_err(|_| failure("model_registry_connection_failed"))?;
    let (folder, _pins, _lock) = journal_named(&data, "actor-model-registry-imports")?;
    let path = folder.join(format!("{actor}-{}.request.json", input.idempotency_key));
    let reply_path = folder.join(format!("{actor}-{}.result.json", input.idempotency_key));
    let replay = path
        .try_exists()
        .map_err(|_| failure("model_registry_connection_failed"))?;
    let german = matches!(language, Some(UiLanguage::De));
    let frozen: FrozenImport = if replay {
        serde_json::from_slice(&read_selected(&path, MAX_RECORD, true)?)
            .map_err(|_| failure("model_registry_receipt_invalid"))?
    } else {
        let context: ActorModelContext = client
            .result(
                client
                    .request(
                        reqwest::Method::GET,
                        &format!(
                            "/native-actor-model-registry/context?project_id={}&profile_id={}",
                            input.project_id, input.profile_id
                        ),
                    )
                    .header("x-cowork-actor-session", credential.clone()),
                false,
            )
            .await?;
        validate_context(&context)?;
        if context.actor_id != actor
            || context.project_id != input.project_id
            || context.provider.id != input.profile_id
        {
            return Err(failure("forbidden"));
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        window
            .app_handle()
            .dialog()
            .file()
            .set_parent(&window)
            .set_title(if german {
                "Geprüftes Qualifikationspaket für das private Profil auswählen"
            } else {
                "Select reviewed qualification bundle for the private profile"
            })
            .add_filter("Cowork model qualification", &["json"])
            .pick_file(move |file| {
                let _ = sender.send(file);
            });
        let Some(selected) = receiver
            .await
            .map_err(|_| failure("model_registry_connection_failed"))?
        else {
            return Ok(None);
        };
        let selected = selected
            .into_path()
            .map_err(|_| failure("invalid_request"))?;
        let bytes = tauri::async_runtime::spawn_blocking(move || {
            read_selected(&selected, MAX_SOURCE, false)
        })
        .await
        .map_err(|_| failure("model_registry_connection_failed"))??;
        let bundle: Value =
            serde_json::from_slice(&bytes).map_err(|_| failure("invalid_request"))?;
        bundle_identity(&bundle)?;
        if bundle["provider_sha256"] != cowork_domain::canonical_hash(&context.provider) {
            return Err(failure("actor_model_registry_profile_mismatch"));
        }
        let request = ImportActorModelQualification {
            idempotency_key: input.idempotency_key.clone(),
            context,
            source_sha256: cowork_domain::hash(&bytes),
            qualification_bundle: bundle,
        };
        FrozenImport {
            format: 1,
            request_sha256: cowork_domain::canonical_hash(&request),
            request,
        }
    };
    validate_frozen(&frozen, &input, actor)?;
    if reply_path
        .try_exists()
        .map_err(|_| failure("model_registry_connection_failed"))?
    {
        let receipt: AcceptedImport =
            serde_json::from_slice(&read_selected(&reply_path, 64 * 1024, true)?)
                .map_err(|_| failure("model_registry_receipt_invalid"))?;
        if receipt.format != 1 || receipt.request_sha256 != frozen.request_sha256 {
            return Err(failure("model_registry_receipt_invalid"));
        }
        validate_reply(&frozen, &receipt.response)?;
        return Ok(Some(receipt.response));
    }
    let (id, version) = bundle_identity(&frozen.request.qualification_bundle)?;
    let c = &frozen.request.context;
    let text = if german {
        format!(
            "Private Modellqualifikation {id} · v{version}\nActor: {}\nProjekt: {} · Revision {}\nProfil: {} · Revision {}\nProfil-SHA256: {}\nDatei-SHA256: {}\n\nBestätige die unabhängig geprüften Gateway-, Modell-, Tokenizer-, Template- und Referenzbelege für genau dieses private Profil. Jeder Modellversand prüft das aktuelle Deployment erneut.{}",
            c.actor_id,
            c.project_id,
            c.project_revision,
            c.provider.id,
            c.provider.revision,
            cowork_domain::canonical_hash(&c.provider),
            frozen.request.source_sha256,
            if replay {
                "\n\nFortsetzung ausschließlich mit dem gespeicherten Originalauftrag; ein früherer Import wird nicht neu interpretiert."
            } else {
                ""
            }
        )
    } else {
        format!(
            "Private model qualification {id} · v{version}\nActor: {}\nProject: {} · revision {}\nProfile: {} · revision {}\nProfile SHA256: {}\nFile SHA256: {}\n\nConfirm independently reviewed gateway, model, tokenizer, template and reference evidence for this exact private profile. Every model dispatch verifies the current deployment again.{}",
            c.actor_id,
            c.project_id,
            c.project_revision,
            c.provider.id,
            c.provider.revision,
            cowork_domain::canonical_hash(&c.provider),
            frozen.request.source_sha256,
            if replay {
                "\n\nContinue only the recorded original request; an earlier import is never reinterpreted."
            } else {
                ""
            }
        )
    };
    if !confirm(&window, german, text).await? {
        return if replay {
            Err(failure("model_registry_retry_cancelled"))
        } else {
            Ok(None)
        };
    }
    if !replay {
        publish(&folder, &path, &frozen, MAX_RECORD)?;
    }
    let response: ActorModelRegistryView = client
        .result(
            client
                .request(reqwest::Method::POST, "/native-actor-model-registry/import")
                .timeout(std::time::Duration::from_secs(60))
                .header("x-cowork-actor-session", credential)
                .header("content-type", "application/json")
                .body(serde_json::to_vec(&frozen.request).map_err(|_| failure("invalid_request"))?),
            true,
        )
        .await?;
    validate_reply(&frozen, &response)?;
    publish(
        &folder,
        &reply_path,
        &AcceptedImport {
            format: 1,
            request_sha256: frozen.request_sha256,
            response: response.clone(),
        },
        64 * 1024,
    )?;
    Ok(Some(response))
}

#[tauri::command]
pub async fn native_actor_model_registry_revoke(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, RegistryGate>,
    input: ActorRevokeSelection,
    language: Option<UiLanguage>,
) -> Result<Value, ApiError> {
    uuid(&input.request.actor_id)?;
    uuid(&input.request.project_id)?;
    let binding = &input.request.binding;
    if binding.id.is_empty()
        || binding.id.len() > 512
        || binding.id.chars().any(char::is_control)
        || binding.version == 0
        || binding.version > i64::MAX as u64
        || !super::update_journal::valid_sha(&binding.sha256)
    {
        return Err(failure("invalid_request"));
    }
    let credential = session(&input.actor_session)?;
    let _gate = gate
        .0
        .try_lock()
        .map_err(|_| failure("model_registry_busy"))?;
    let client = connection(&window).await?;
    let authority: Value = client
        .result(
            client
                .request(
                    reqwest::Method::GET,
                    &format!(
                        "/native-actor-model-registry/authority?project_id={}",
                        input.request.project_id
                    ),
                )
                .header("x-cowork-actor-session", credential.clone()),
            false,
        )
        .await?;
    if authority["actor_id"] != input.request.actor_id
        || authority["project_id"] != input.request.project_id
    {
        return Err(failure("forbidden"));
    }
    let german = matches!(language, Some(UiLanguage::De));
    let text = if german {
        format!(
            "Private Qualifikation {} · v{}\nActor: {}\nProjekt: {}\nSHA256: {}\n\nDiese genaue Version für neue Modellanfragen widerrufen? Ursprüngliche Import- und Laufbelege bleiben erhalten.",
            binding.id,
            binding.version,
            input.request.actor_id,
            input.request.project_id,
            binding.sha256
        )
    } else {
        format!(
            "Private qualification {} · v{}\nActor: {}\nProject: {}\nSHA256: {}\n\nRevoke this exact version for new model requests? Original import and run receipts remain retained.",
            binding.id,
            binding.version,
            input.request.actor_id,
            input.request.project_id,
            binding.sha256
        )
    };
    if !confirm(&window, german, text).await? {
        return Err(failure("model_registry_retry_cancelled"));
    }
    client
        .result(
            client
                .request(reqwest::Method::POST, "/native-actor-model-registry/revoke")
                .header("x-cowork-actor-session", credential)
                .header("content-type", "application/json")
                .body(serde_json::to_vec(&input.request).map_err(|_| failure("invalid_request"))?),
            true,
        )
        .await
}
