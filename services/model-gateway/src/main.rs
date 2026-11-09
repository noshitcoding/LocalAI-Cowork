//! One explicitly pinned owned backend. No remote backend or model fallback.
mod config;
mod owner;
#[cfg(test)]
mod tests;
use anyhow::{Result, bail};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
};
use cowork_model_accounting::{digest, sha256};
use futures_util::StreamExt;
use serde_json::{Value, json};
use sqlx::{Row, sqlite::SqlitePoolOptions};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone)]
struct Gateway {
    loaded: Arc<config::Loaded>,
    http: reqwest::Client,
    upstream: String,
    backend_key: String,
    backend_owner: Arc<owner::BackendOwner>,
    backend_address: std::net::SocketAddr,
    pool: sqlx::SqlitePool,
    alive: Arc<AtomicBool>,
    serial: Arc<tokio::sync::Semaphore>,
}
type Answer = std::result::Result<Response, (StatusCode, &'static str)>;
struct StopOnDrop(Arc<AtomicBool>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
fn authorized(g: &Gateway, h: &HeaderMap) -> bool {
    let Some(token) = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    else {
        return false;
    };
    let a = token.as_bytes();
    let b = g.loaded.token.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}
fn response(bytes: Vec<u8>, g: &Gateway, request: Option<&str>) -> Response {
    let mut r = Response::new(Body::from(bytes));
    r.headers_mut()
        .insert("content-type", "text/event-stream".parse().unwrap());
    r.headers_mut().insert(
        "x-cowork-deployment-sha256",
        g.loaded
            .config
            .bundle
            .evidence
            .deployment_sha256
            .parse()
            .unwrap(),
    );
    if let Some(id) = request {
        r.headers_mut()
            .insert("x-cowork-model-attempt", id.parse().unwrap());
    }
    r
}
async fn models(State(g): State<Gateway>, h: HeaderMap) -> Answer {
    if !authorized(&g, &h) {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    if !g.alive.load(Ordering::Acquire) {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "owned_backend_stopped"));
    }
    g.backend_owner
        .assert_listener(g.backend_address)
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "original_backend_listener_unconfirmed",
            )
        })?;
    Ok(Json(json!({"data":[{"id":g.loaded.config.profile.model}]})).into_response())
}
use axum::response::IntoResponse;
async fn descriptor(State(g): State<Gateway>, h: HeaderMap) -> Answer {
    if !authorized(&g, &h) {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    if !g.alive.load(Ordering::Acquire) {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "owned_backend_stopped"));
    }
    g.backend_owner
        .assert_listener(g.backend_address)
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "original_backend_listener_unconfirmed",
            )
        })?;
    let b = &g.loaded.config.bundle;
    let m = &b.qualification.model;
    Ok(Json(json!({"format":1,"provider_sha256":b.provider_sha256,"deployment_sha256":b.evidence.deployment_sha256,"model_sha256":m.model_sha256,"tokenizer_sha256":m.tokenizer_sha256,"chat_template_sha256":m.chat_template_sha256,"prefill":"literal_token_ids_no_implicit_bos_v1","tokenize":{"add_special":false,"parse_special":true},"supervisor":"cowork-executor-owned-v1"})).into_response())
}
async fn bounded_json(response: reqwest::Response, limit: usize) -> Result<Value> {
    if !response.status().is_success() {
        bail!("Owned backend refused request")
    };
    let mut stream = response.bytes_stream();
    let mut bytes = vec![];
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len() + chunk.len() > limit {
            bail!("Backend metadata limit")
        };
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}
async fn actual_ids(g: &Gateway, text: &str) -> Result<Vec<u32>> {
    g.loaded.assert_original_files()?;
    g.backend_owner.assert_listener(g.backend_address)?;
    let ids = g.loaded.model.tokenizer.encode_complete(text)?;
    let body=bounded_json(g.http.post(format!("{}/tokenize",g.upstream)).bearer_auth(&g.backend_key).json(&json!({"content":text,"add_special":false,"parse_special":true,"with_pieces":false})).send().await?,32*1024*1024).await?;
    if body["tokens"] != json!(ids) {
        bail!("Actual owned backend token IDs differ from qualification")
    };
    g.backend_owner.assert_listener(g.backend_address)?;
    Ok(ids)
}
async fn receipt(State(g): State<Gateway>, Path(id): Path<String>, h: HeaderMap) -> Answer {
    if !authorized(&g, &h) {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let row = sqlx::query("SELECT request_sha256,state,receipt,input_ids,length(raw_response) AS received_bytes FROM requests WHERE id=?")
        .bind(&id)
        .fetch_optional(&g.pool)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "journal_unavailable"))?
        .ok_or((StatusCode::NOT_FOUND, "not_found"))?;
    let payload: Option<String> = row
        .try_get("receipt")
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "journal_corrupt"))?;
    let facts: Option<Value> = payload
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "journal_corrupt"))?;
    let ids: Value = serde_json::from_str(&row.get::<String, _>("input_ids"))
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "journal_corrupt"))?;
    Ok(Json(json!({"id":id,"request_sha256":row.get::<String,_>("request_sha256"),"state":row.get::<String,_>("state"),"input_token_ids":ids,"receipt":facts,"received_bytes":row.get::<i64,_>("received_bytes"),"raw_response_path":format!("/requests/{id}/response"),"replay_authority":false})).into_response())
}
async fn retained_response(
    State(g): State<Gateway>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Answer {
    if !authorized(&g, &h) {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let row = sqlx::query("SELECT state,raw_response FROM requests WHERE id=?")
        .bind(&id)
        .fetch_optional(&g.pool)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "journal_unavailable"))?
        .ok_or((StatusCode::NOT_FOUND, "not_found"))?;
    let mut r = response(row.get::<Vec<u8>, _>("raw_response"), &g, Some(&id));
    r.headers_mut()
        .insert("content-type", "application/octet-stream".parse().unwrap());
    r.headers_mut().insert(
        "x-cowork-retained-state",
        row.get::<String, _>("state")
            .parse()
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "journal_corrupt"))?,
    );
    Ok(r)
}
fn completed_stream(bytes: &[u8], input: u64, output: u64) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let (mut done, mut finished, mut usage) = (false, false, None);
    for line in text
        .lines()
        .filter(|s| !s.is_empty() && !s.starts_with(':'))
    {
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            return false;
        };
        if data == "[DONE]" {
            if done || !finished || usage.is_none() {
                return false;
            }
            done = true;
            continue;
        }
        if done {
            return false;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return false;
        };
        if v.get("error").is_some() {
            return false;
        }
        let Some(choices) = v["choices"].as_array() else {
            return false;
        };
        if !choices.is_empty() {
            if finished
                || choices.len() != 1
                || choices[0]["index"] != 0
                || !choices[0]["text"].is_string()
            {
                return false;
            }
            if !choices[0]["finish_reason"].is_null() {
                if !matches!(
                    choices[0]["finish_reason"].as_str(),
                    Some("stop" | "length")
                ) {
                    return false;
                }
                finished = true;
            }
        }
        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            if usage.is_some()
                || !finished
                || u["prompt_tokens"].as_u64() != Some(input)
                || u["completion_tokens"].as_u64().is_none_or(|n| n > output)
                || u["total_tokens"].as_u64()
                    != u["completion_tokens"]
                        .as_u64()
                        .and_then(|n| n.checked_add(input))
            {
                return false;
            }
            usage = Some(u.clone());
        } else {
            if choices.is_empty() {
                return false;
            }
        }
    }
    done && finished && usage.is_some()
}
async fn completion(State(g): State<Gateway>, h: HeaderMap, bytes: Bytes) -> Answer {
    if !authorized(&g, &h) {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let id = h
        .get("x-cowork-model-attempt")
        .and_then(|v| v.to_str().ok())
        .filter(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
        .ok_or((StatusCode::BAD_REQUEST, "exact_attempt_required"))?
        .to_string();
    if h.get("x-cowork-deployment-sha256")
        .and_then(|v| v.to_str().ok())
        != Some(g.loaded.config.bundle.evidence.deployment_sha256.as_str())
    {
        return Err((StatusCode::CONFLICT, "deployment_changed"));
    }
    let hash = sha256(&bytes);
    let body: Value =
        serde_json::from_slice(&bytes).map_err(|_| (StatusCode::BAD_REQUEST, "invalid_json"))?;
    // Serial caller slots do not admit requests. Durable deduplication precedes
    // every backend contact; previous pending/unknown intents never execute.
    let permit = g
        .serial
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "shutdown"))?;
    let mut tx = g
        .pool
        .begin()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "journal_unavailable"))?;
    sqlx::query("UPDATE guard SET revision=revision WHERE id=1")
        .execute(&mut *tx)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "journal_unavailable"))?;
    if let Some(row) = sqlx::query("SELECT request_sha256,state,response FROM requests WHERE id=?")
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "journal_unavailable"))?
    {
        if row.get::<String, _>("request_sha256") != hash {
            return Err((StatusCode::CONFLICT, "attempt_changed"));
        };
        let old: Option<Vec<u8>> = row.get("response");
        return match old {
            Some(bytes) => Ok(response(bytes, &g, Some(&id))),
            None => Err((StatusCode::CONFLICT, "retained_unconfirmed_request")),
        };
    }
    if !g.alive.load(Ordering::Acquire) {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "owned_backend_stopped"));
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM requests")
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "journal_unavailable"))?;
    if count >= 1024 {
        return Err((StatusCode::INSUFFICIENT_STORAGE, "retained_journal_limit"));
    }
    if body["model"] != g.loaded.config.profile.model
        || body["stream"] != true
        || body["stream_options"] != json!({"include_usage":true})
        || body.as_object().is_none_or(|o| {
            o.keys().any(|k| {
                !["model", "prompt", "stream", "stream_options", "max_tokens"].contains(&k.as_str())
            })
        })
    {
        return Err((StatusCode::BAD_REQUEST, "exact_raw_completion_required"));
    }
    let text = body["prompt"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or((StatusCode::BAD_REQUEST, "complete_prompt_required"))?;
    let output = body["max_tokens"]
        .as_u64()
        .filter(|n| *n > 0 && *n < g.loaded.model.identity().context_tokens)
        .ok_or((StatusCode::BAD_REQUEST, "output_allowance_required"))?;
    let ids = actual_ids(&g, text)
        .await
        .map_err(|_| (StatusCode::CONFLICT, "actual_tokenizer_mismatch"))?;
    if ids.len() as u64 + output > g.loaded.model.identity().context_tokens {
        return Err((StatusCode::BAD_REQUEST, "context_limit"));
    }
    let ids_sha = digest(&ids).map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "digest_failed"))?;
    let mut forwarded = body.clone();
    forwarded["prompt"] = json!(ids);
    forwarded["cache_prompt"] = json!(false);
    let forwarded_bytes =
        serde_json::to_vec(&forwarded).map_err(|_| (StatusCode::BAD_REQUEST, "invalid_json"))?;
    sqlx::query("INSERT INTO requests(id,request_sha256,state,input_ids,receipt,response,raw_response) VALUES(?,?,'pending',?,NULL,NULL,X'')").bind(&id).bind(&hash).bind(serde_json::to_string(&ids).unwrap()).execute(&mut *tx).await.map_err(|_|(StatusCode::INTERNAL_SERVER_ERROR,"journal_unavailable"))?;
    tx.commit()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "journal_unavailable"))?;
    let rendered_sha = sha256(text.as_bytes());
    // A disconnected caller cannot cancel an already admitted original send.
    // The owned task keeps its serial slot until receive evidence is durable.
    tokio::spawn(async move {
        let _permit = permit;
        execute_completion(
            g,
            id,
            hash,
            forwarded_bytes,
            CompletionContext {
                input_tokens: ids.len() as u64,
                ids_sha,
                rendered_sha,
                output,
            },
        )
        .await
    })
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "retained_unconfirmed_request",
        )
    })?
}
struct CompletionContext {
    input_tokens: u64,
    ids_sha: String,
    rendered_sha: String,
    output: u64,
}

async fn execute_completion(
    g: Gateway,
    id: String,
    hash: String,
    forwarded_bytes: Vec<u8>,
    context: CompletionContext,
) -> Answer {
    let CompletionContext {
        input_tokens,
        ids_sha,
        rendered_sha,
        output,
    } = context;
    let mut actual = vec![];
    let mut backend_status = None;
    let execution = async {
        g.loaded.assert_original_files()?;
        g.backend_owner.assert_listener(g.backend_address)?;
        let r = g
            .http
            .post(format!("{}/v1/completions", g.upstream))
            .bearer_auth(&g.backend_key)
            .header("content-type", "application/json")
            .body(forwarded_bytes.clone())
            .send()
            .await?;
        let success = r.status().is_success();
        backend_status = Some(r.status().as_u16());
        let mut stream = r.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if actual.len() + chunk.len() > 4_000_000 {
                bail!("Received stream limit")
            };
            actual.extend_from_slice(&chunk);
            if sqlx::query("UPDATE requests SET raw_response=? WHERE id=? AND request_sha256=? AND state='pending'")
                .bind(&actual).bind(&id).bind(&hash).execute(&g.pool).await?.rows_affected()!=1 {
                bail!("Original receive journal changed")
            }
        }
        if !success { bail!("Backend rejected completion") }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let state = if execution.is_ok() && completed_stream(&actual, input_tokens, output) {
        "completed"
    } else {
        "unknown"
    };
    let evidence = json!({"format":1,"deployment_sha256":g.loaded.config.bundle.evidence.deployment_sha256,"request_sha256":hash,"forwarded_payload_sha256":sha256(&forwarded_bytes),"rendered_sha256":rendered_sha,"input_token_ids_sha256":ids_sha,"input_tokens":input_tokens,"literal_token_ids":true,"no_implicit_bos":true,"raw_response_sha256":sha256(&actual),"received_bytes":actual.len(),"backend_http_status":backend_status,"completed_stream":state=="completed","replay_authority":false});
    if sqlx::query("UPDATE requests SET state=?,receipt=?,response=? WHERE id=? AND request_sha256=? AND state='pending'").bind(state).bind(evidence.to_string()).bind(if state=="completed"{Some(actual.clone())}else{None}).bind(&id).bind(&hash).execute(&g.pool).await.map_err(|_|(StatusCode::INTERNAL_SERVER_ERROR,"journal_unavailable"))?.rows_affected()!=1{return Err((StatusCode::INTERNAL_SERVER_ERROR,"journal_changed"))}
    if state != "completed" {
        return Err((StatusCode::BAD_GATEWAY, "retained_unconfirmed_request"));
    }
    Ok(response(actual, &g, Some(&id)))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 2 && args[1] == "--version" {
        println!("cowork-model-gateway {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.len() != 3 || (args[1] != "--config" && args[1] != "--inspect-config") {
        bail!(
            "Usage: cowork-model-gateway --config ABSOLUTE_PRIVATE_CONFIG | --inspect-config ABSOLUTE_PRIVATE_CONFIG"
        )
    }
    let loaded = Arc::new(config::load(std::path::Path::new(&args[2]))?);
    if args[1] == "--inspect-config" {
        println!(
            "{}",
            json!({"format":1,"configuration_valid":true,"deployment_sha256":loaded.config.bundle.evidence.deployment_sha256,"provider_sha256":loaded.config.bundle.provider_sha256,"qualification_sha256":loaded.model.sha256()?,"started":false,"actual_backend_tokenizer_checked":false})
        );
        return Ok(());
    }
    let _ownership = config::ownership_lock(
        &loaded
            .config
            .data_directory
            .join("model-gateway-owner.lock"),
    )?;
    let listener = tokio::net::TcpListener::bind(loaded.config.listen).await?;
    let reservation = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = reservation.local_addr()?;
    drop(reservation);
    let backend_key = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let key_file = loaded
        .config
        .data_directory
        .join(format!("backend-key-{}.txt", uuid::Uuid::new_v4()));
    config::create_private(&key_file, backend_key.as_bytes())?;
    let mut environment = std::collections::BTreeMap::new();
    environment.insert(
        "PATH".into(),
        loaded.config.backend_directory.to_string_lossy().into(),
    );
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        environment.insert("SystemRoot".into(), root.to_string_lossy().into());
    }
    let launch = cowork_executor::TrustedLaunch {
        program: loaded
            .config
            .backend_directory
            .join(&loaded.config.deployment.server),
        directory: loaded.config.backend_directory.clone(),
        arguments: vec![
            "--model".into(),
            loaded.config.model_file.to_string_lossy().into(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            address.port().to_string(),
            "--ctx-size".into(),
            loaded.config.deployment.context_tokens.to_string(),
            "--parallel".into(),
            "1".into(),
            "--alias".into(),
            loaded.config.profile.model.clone(),
            "--api-key-file".into(),
            key_file.to_string_lossy().into(),
            "--no-webui".into(),
            "--no-context-shift".into(),
            "--offline".into(),
        ],
        environment,
        timeout: Duration::from_secs(86400),
        capture_bytes_per_stream: 65536,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let _cancel_on_error = StopOnDrop(cancel.clone());
    let alive = Arc::new(AtomicBool::new(true));
    let stop = cancel.clone();
    let running = alive.clone();
    let (owner_send, owner_receive) = tokio::sync::oneshot::channel();
    let backend = tokio::task::spawn_blocking(move || {
        let result = cowork_executor::supervise_started(&launch, &stop, |pid| {
            let owner = owner::BackendOwner::capture(pid).map_err(|_| {
                cowork_executor::Error::Io(std::io::Error::other(
                    "Cannot pin original owned backend",
                ))
            })?;
            owner_send.send(owner).map_err(|_| {
                cowork_executor::Error::Invalid("Original process owner receiver closed")
            })
        });
        running.store(false, Ordering::Release);
        result
    });
    let backend_owner = Arc::new(owner_receive.await?);
    let db = loaded.config.data_directory.join("model-gateway.sqlite");
    if !db.exists() {
        config::create_private(&db, b"")?
    }
    drop(config::open(&db, true)?);
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Full)
        .busy_timeout(Duration::from_secs(10));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    sqlx::raw_sql("CREATE TABLE IF NOT EXISTS guard(id INTEGER PRIMARY KEY,revision INTEGER NOT NULL);INSERT OR IGNORE INTO guard VALUES(1,0);CREATE TABLE IF NOT EXISTS deployment(id INTEGER PRIMARY KEY,sha256 TEXT NOT NULL);CREATE TABLE IF NOT EXISTS requests(id TEXT PRIMARY KEY,request_sha256 TEXT NOT NULL,state TEXT NOT NULL,input_ids TEXT NOT NULL,receipt TEXT,response BLOB,raw_response BLOB NOT NULL DEFAULT X'');CREATE TABLE IF NOT EXISTS binding(id INTEGER PRIMARY KEY,sha256 TEXT NOT NULL)").execute(&pool).await?;
    let columns = sqlx::query("PRAGMA table_info(requests)")
        .fetch_all(&pool)
        .await?;
    if !columns
        .iter()
        .any(|r| r.get::<String, _>("name") == "raw_response")
    {
        sqlx::query("ALTER TABLE requests ADD COLUMN raw_response BLOB NOT NULL DEFAULT X''")
            .execute(&pool)
            .await?;
        sqlx::query("UPDATE requests SET raw_response=response WHERE response IS NOT NULL")
            .execute(&pool)
            .await?;
    }
    let binding = digest(
        &json!({"format":1,"qualification_sha256":loaded.model.sha256()?,"provider_sha256":loaded.config.bundle.provider_sha256,"deployment_sha256":loaded.config.bundle.evidence.deployment_sha256}),
    )?;
    let original_binding: Option<String> =
        sqlx::query_scalar("SELECT sha256 FROM binding WHERE id=1")
            .fetch_optional(&pool)
            .await?;
    if original_binding.as_ref().is_some_and(|v| v != &binding)
        || original_binding.is_none()
            && sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM requests")
                .fetch_one(&pool)
                .await?
                > 0
    {
        bail!("State directory does not belong to this original qualification/profile")
    }
    sqlx::query("INSERT OR IGNORE INTO binding(id,sha256) VALUES(1,?)")
        .bind(binding)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT OR IGNORE INTO deployment(id,sha256) VALUES(1,?)")
        .bind(&loaded.config.bundle.evidence.deployment_sha256)
        .execute(&pool)
        .await?;
    let retained: String = sqlx::query_scalar("SELECT sha256 FROM deployment WHERE id=1")
        .fetch_one(&pool)
        .await?;
    if retained != loaded.config.bundle.evidence.deployment_sha256 {
        bail!("State directory belongs to a different immutable deployment")
    }
    let gateway = Gateway {
        loaded,
        http: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(1800))
            .build()?,
        upstream: format!("http://{address}"),
        backend_key,
        backend_owner,
        backend_address: address,
        pool,
        alive: alive.clone(),
        serial: Arc::new(tokio::sync::Semaphore::new(1)),
    };
    let ready = async {
        for _ in 0..600 {
            if !alive.load(Ordering::Acquire) {
                bail!("Owned backend exited")
            };
            if gateway
                .http
                .get(format!("{}/health", gateway.upstream))
                .bearer_auth(&gateway.backend_key)
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                for sample in &gateway.loaded.config.bundle.evidence.reference_requests {
                    let actual = actual_ids(&gateway, &sample.rendered_text).await?;
                    if actual != sample.token_ids {
                        bail!("External reference vectors disagree with actual backend")
                    }
                }
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        bail!("Owned backend readiness timeout")
    }
    .await;
    if let Err(error) = ready {
        cancel.store(true, Ordering::Release);
        backend.await??;
        return Err(error);
    }
    let state = gateway.clone();
    let app = Router::new()
        .route("/models", get(models))
        .route("/.well-known/cowork-model-qualification", get(descriptor))
        .route("/completions", post(completion))
        .route("/requests/{id}", get(receipt))
        .route("/requests/{id}/response", get(retained_response))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state);
    let stop_cancel = cancel.clone();
    let shutdown = async move {
        loop {
            tokio::select! {_=tokio::signal::ctrl_c()=>break,_=tokio::time::sleep(Duration::from_millis(100))=>if !alive.load(Ordering::Acquire){break}}
        }
        stop_cancel.store(true, Ordering::Release);
    };
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;
    cancel.store(true, Ordering::Release);
    let outcome = backend.await??;
    // Detached original sends settle after backend shutdown before closing SQLite.
    let _settled = gateway.serial.clone().acquire_owned().await?;
    gateway.pool.close().await;
    served?;
    if !outcome.output_complete {
        bail!("Backend output handles remain unconfirmed")
    };
    Ok(())
}
