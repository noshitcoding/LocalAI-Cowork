//! Bounded, non-streaming OpenAI-compatible managed-model gateway. The instance
//! receives a scoped proxy token; provider credentials stay in management.
use super::*;
use async_trait::async_trait;
use axum::response::{IntoResponse, Response};
use std::time::Duration;

const MAX_RESPONSE: usize = 16 * 1024 * 1024;

#[derive(Clone)]
struct Quote {
    currency: String,
    maximum_micros: i64,
}
struct Plan {
    url: String,
    secret_reference: Option<String>,
    body: Value,
    pricing: Option<ModelPricing>,
    quote: Option<Quote>,
    input_bound: u64,
    output_bound: u64,
}
#[async_trait]
trait Budget: Send + Sync {
    async fn reserve(&self, quote: Option<&Quote>) -> Result<Option<Uuid>, ApiError>;
    async fn settle(
        &self,
        reservation: Option<Uuid>,
        actual_micros: Option<i64>,
        usage: Value,
    ) -> Result<(), ApiError>;
}
fn cost(pricing: &ModelPricing, input: u64, output: u64) -> Result<i64, ApiError> {
    let micros = ((input as u128 * pricing.input_micros_per_million as u128)
        + (output as u128 * pricing.output_micros_per_million as u128)
        + 999_999)
        / 1_000_000;
    i64::try_from(micros).map_err(|_| invalid("model price exceeds supported accounting bounds"))
}
fn known_keys(value: &serde_json::Map<String, Value>, allowed: &[&str]) -> bool {
    value.keys().all(|key| allowed.contains(&key.as_str()))
}
fn text_messages(body: &serde_json::Map<String, Value>) -> bool {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return false;
    };
    messages.iter().all(|message| {
        let Some(message) = message.as_object() else {
            return false;
        };
        let Some(role) = message.get("role").and_then(Value::as_str) else {
            return false;
        };
        if !["system", "developer", "user", "assistant", "tool"].contains(&role)
            || !known_keys(
                message,
                &["role", "content", "name", "tool_calls", "tool_call_id"],
            )
            || message
                .get("name")
                .is_some_and(|v| v.as_str().is_none_or(|name| name.len() > 128))
            || message
                .get("tool_call_id")
                .is_some_and(|v| v.as_str().is_none_or(|name| name.len() > 200))
        {
            return false;
        }
        let calls = message.get("tool_calls");
        if let Some(calls) = calls {
            if role != "assistant"
                || calls.as_array().is_none_or(|calls| {
                    calls.is_empty()
                        || calls.len() > 64
                        || calls.iter().any(|call| {
                            let Some(call) = call.as_object() else {
                                return true;
                            };
                            if !known_keys(call, &["id", "type", "function"])
                                || call.get("type").and_then(Value::as_str) != Some("function")
                                || call.get("id").and_then(Value::as_str).is_none()
                            {
                                return true;
                            }
                            let Some(function) = call.get("function").and_then(Value::as_object)
                            else {
                                return true;
                            };
                            !known_keys(function, &["name", "arguments"])
                                || function.get("name").and_then(Value::as_str).is_none()
                                || function.get("arguments").and_then(Value::as_str).is_none()
                        })
                })
            {
                return false;
            }
        }
        match message.get("content") {
            Some(Value::String(_)) => true,
            None | Some(Value::Null) => role == "assistant" && calls.is_some(),
            _ => false,
        }
    })
}
fn function_tools(body: &serde_json::Map<String, Value>) -> bool {
    if let Some(choice) = body.get("tool_choice") {
        let supported = match choice {
            Value::String(value) => ["auto", "none", "required"].contains(&value.as_str()),
            Value::Object(value) => {
                known_keys(value, &["type", "function"])
                    && value.get("type").and_then(Value::as_str) == Some("function")
                    && value
                        .get("function")
                        .and_then(Value::as_object)
                        .is_some_and(|function| {
                            known_keys(function, &["name"])
                                && function.get("name").and_then(Value::as_str).is_some()
                        })
            }
            _ => false,
        };
        if !supported {
            return false;
        }
    }
    let Some(tools) = body.get("tools") else {
        return true;
    };
    tools.as_array().is_some_and(|tools| {
        tools.len() <= 128
            && tools.iter().all(|tool| {
                let Some(tool) = tool.as_object() else {
                    return false;
                };
                let Some(function) = tool.get("function").and_then(Value::as_object) else {
                    return false;
                };
                known_keys(tool, &["type", "function"])
                    && tool.get("type").and_then(Value::as_str) == Some("function")
                    && known_keys(function, &["name", "description", "parameters", "strict"])
                    && function
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|v| !v.is_empty() && v.len() <= 128)
                    && function.get("description").is_none_or(Value::is_string)
                    && function.get("parameters").is_some_and(Value::is_object)
                    && function.get("strict").is_none_or(Value::is_boolean)
            })
    })
}
fn plan(
    model: ModelConfig,
    endpoint: EndpointConfig,
    reference: Option<String>,
    mut body: Value,
) -> Result<Plan, ApiError> {
    if endpoint.protocol != "openai_compatible" {
        return Err(invalid(
            "the managed model proxy supports OpenAI-compatible endpoints only",
        ));
    }
    address(&endpoint.base_url)?;
    let object = body
        .as_object_mut()
        .ok_or_else(|| invalid("chat completion body must be an object"))?;
    const ALLOWED: &[&str] = &[
        "model",
        "messages",
        "tools",
        "tool_choice",
        "max_tokens",
        "max_completion_tokens",
        "stream",
        "n",
        "temperature",
        "top_p",
        "stop",
        "seed",
        "presence_penalty",
        "frequency_penalty",
        "parallel_tool_calls",
    ];
    if object.keys().any(|key| !ALLOWED.contains(&key.as_str()))
        || object.get("model").and_then(Value::as_str) != Some(model.model_id.as_str())
        || object.get("stream").is_some_and(|v| v != false)
        || object.get("n").is_some_and(|v| v != 1)
        || object
            .get("messages")
            .and_then(Value::as_array)
            .is_none_or(|v| v.is_empty() || v.len() > 128)
        || !text_messages(object)
        || !function_tools(object)
    {
        return Err(invalid(
            "use the assigned model with bounded text-only non-stream messages, function tools and n=1",
        ));
    }
    let configured_output = model.max_output_tokens.unwrap_or(1024) as u64;
    let requested = object
        .get("max_tokens")
        .or_else(|| object.get("max_completion_tokens"));
    let output_bound = match requested {
        Some(v) => v
            .as_u64()
            .filter(|v| *v > 0 && *v <= configured_output)
            .ok_or_else(|| invalid("requested output exceeds the configured model limit"))?,
        None => configured_output,
    };
    if object.contains_key("max_tokens") && object.contains_key("max_completion_tokens") {
        return Err(invalid("choose one output-token limit"));
    }
    object.remove("max_completion_tokens");
    object.insert("max_tokens".into(), json!(output_bound));
    object.insert("stream".into(), json!(false));
    object.insert("n".into(), json!(1));
    let context = model.context_window.unwrap_or(32768) as u64;
    let input_bound = context
        .checked_sub(output_bound)
        .ok_or_else(|| invalid("output must fit within the configured context"))?;
    let encoded = serde_json::to_vec(&body)?;
    // Reserve the entire configured input allowance. Byte-size plus a fixed
    // protocol allowance is deliberately conservative for byte-token providers;
    // untrusted provider usage is never allowed to expand a committed hold.
    if encoded.len() > 256 * 1024 || (encoded.len() as u64).saturating_add(4096) > input_bound {
        return Err(invalid(
            "chat input exceeds the conservative configured context bound",
        ));
    }
    let quote = match (
        &model.pricing,
        model.context_window,
        model.max_output_tokens,
    ) {
        (Some(pricing), Some(_), Some(_)) => Some(Quote {
            currency: pricing.currency.clone(),
            maximum_micros: cost(pricing, input_bound, output_bound)?,
        }),
        _ => None,
    };
    Ok(Plan {
        url: format!(
            "{}/chat/completions",
            endpoint.base_url.trim_end_matches('/')
        ),
        secret_reference: reference,
        body,
        pricing: model.pricing,
        quote,
        input_bound,
        output_bound,
    })
}

async fn forward<B: Budget>(plan: Plan, budget: &B) -> Result<Response, ApiError> {
    // Local preflight cannot incur provider cost. Do not create a wallet hold
    // for a missing binding or invalid transport configuration.
    let key = crate::managed_resources::resolve_secret(plan.secret_reference.as_deref())?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|_| ApiError::Unavailable("model transport unavailable".into()))?;
    let hold = budget.reserve(plan.quote.as_ref()).await?;
    let result = async {
        let mut request = client.post(&plan.url).json(&plan.body);
        if let Some(key) = key { request = request.bearer_auth(key); }
        let mut response = request.send().await.map_err(|_|ApiError::Unavailable("model provider did not complete; reservation retained conservatively".into()))?;
        if response.content_length().is_some_and(|v| v>MAX_RESPONSE as u64) { return Err(ApiError::Unavailable("model response exceeds supported bounds".into())); }
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_|ApiError::Unavailable("model response interrupted; reservation retained conservatively".into()))? {
            if bytes.len().saturating_add(chunk.len())>MAX_RESPONSE { return Err(ApiError::Unavailable("model response exceeds supported bounds".into())); }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() { return Err(ApiError::Unavailable("model provider rejected the request; reservation retained conservatively".into())); }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_|ApiError::Unavailable("model provider returned an invalid response".into()))?;
        let input = value["usage"]["prompt_tokens"].as_u64();
        let output = value["usage"]["completion_tokens"].as_u64();
        let actual = match (&plan.pricing,input,output) {
            (Some(pricing),Some(input),Some(output)) if input<=plan.input_bound && output<=plan.output_bound => Some(cost(pricing,input,output)?),
            _ => None,
        };
        if matches!((input,output),(Some(input),Some(output)) if input>plan.input_bound || output>plan.output_bound) {
            return Err(ApiError::Unavailable("provider usage exceeded the configured reservation bounds".into()));
        }
        let usage=json!({"prompt_tokens":input,"completion_tokens":output,"response_sha256":hex_digest(&bytes)});
        Ok((value,actual,usage))
    }.await;
    match result {
        Ok((value, actual, usage)) => {
            budget.settle(hold, actual, usage).await?;
            Ok((
                [(axum::http::header::CACHE_CONTROL, "no-store")],
                Json(value),
            )
                .into_response())
        }
        Err(error) => {
            // Never refund an ambiguous/billable upstream outcome. A crash leaves
            // its durable hold active until explicit accounting reconciliation.
            budget.settle(hold, None, Value::Null).await?;
            Err(error)
        }
    }
}

pub(crate) async fn complete(
    State(state): State<AppState>,
    Path((org, instance, user, model)): Path<(Uuid, Uuid, Uuid, Uuid)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(denied)?;
    let mut tx = state.pool.begin().await?;
    let key = crate::fleet::current_instance_key(&mut tx, org, instance).await?;
    crate::managed_resources::verify_model_proxy_token(
        bearer,
        &key,
        org,
        instance,
        user,
        model,
        chrono::Utc::now().timestamp(),
    )?;
    member(&mut tx, org, user).await?;
    scope(&mut tx, org, user, instance, None).await?;
    let row=sqlx::query("SELECT m.configuration AS model,m.secret_reference AS model_secret,e.configuration AS endpoint,e.secret_reference AS endpoint_secret,m.revision AS model_revision,e.revision AS endpoint_revision FROM management_resource_catalog m JOIN management_resource_dependencies d ON d.resource_id=m.id JOIN management_resource_catalog e ON e.id=d.depends_on AND e.kind='endpoint' WHERE m.id=$4 AND m.kind='model' AND management_resource_available(m.id) AND EXISTS(SELECT 1 FROM management_resource_assignments a WHERE a.organisation_id=$1 AND a.user_id=$2 AND a.resource_id=m.id AND (a.instance_id IS NULL OR a.instance_id=$3) AND a.environment_id IS NULL AND management_resource_scope_available(a.organisation_id,a.user_id,a.instance_id,a.environment_id,a.resource_id)) FOR SHARE OF m,e")
        .bind(org).bind(user).bind(instance).bind(model).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    let model_config: ModelConfig = serde_json::from_value(row.get("model"))
        .map_err(|_| invalid("managed model configuration is invalid"))?;
    let endpoint: EndpointConfig = serde_json::from_value(row.get("endpoint"))
        .map_err(|_| invalid("managed endpoint configuration is invalid"))?;
    let reference = row
        .get::<Option<String>, _>("model_secret")
        .or(row.get::<Option<String>, _>("endpoint_secret"));
    let plan = plan(model_config, endpoint, reference, body)?;
    let request_id = Uuid::new_v4();
    let snapshot = json!({"organisation_id":org,"instance_id":instance,"user_id":user,"model_resource_id":model,"model_revision":row.get::<i64,_>("model_revision"),"endpoint_revision":row.get::<i64,_>("endpoint_revision"),"pricing":plan.pricing,"input_bound":plan.input_bound,"output_bound":plan.output_bound,"body":plan.body});
    let digest = hex_digest(&serde_json::to_vec(&snapshot)?);
    tx.commit().await?;
    let budget = WalletBudget {
        state: &state,
        org,
        instance,
        user,
        model,
        request_id,
        digest,
        metadata: json!({"model_revision":row.get::<i64,_>("model_revision"),"endpoint_revision":row.get::<i64,_>("endpoint_revision"),"pricing":plan.pricing,"input_bound":plan.input_bound,"output_bound":plan.output_bound}),
    };
    forward(plan, &budget).await
}
fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
struct WalletBudget<'a> {
    state: &'a AppState,
    org: Uuid,
    instance: Uuid,
    user: Uuid,
    model: Uuid,
    request_id: Uuid,
    digest: String,
    metadata: Value,
}
#[async_trait]
impl Budget for WalletBudget<'_> {
    async fn reserve(&self, quote: Option<&Quote>) -> Result<Option<Uuid>, ApiError> {
        let decision = super::super::wallet::reserve(
            self.state,
            super::super::wallet::WalletReservationRequest {
                organisation_id: self.org,
                instance_id: self.instance,
                user_id: self.user,
                model_resource_id: self.model,
                request_id: self.request_id,
                request_sha256: self.digest.clone(),
                maximum_micros: quote.map(|q| q.maximum_micros),
                currency: quote.map(|q| q.currency.clone()),
                metadata: self.metadata.clone(),
            },
        )
        .await?;
        if decision.replayed {
            return Err(ApiError::Conflict(
                "this proxy request reservation already exists; automatic replay is unavailable"
                    .into(),
            ));
        }
        Ok(decision.reservation.map(|r| r.id))
    }
    async fn settle(
        &self,
        reservation: Option<Uuid>,
        actual: Option<i64>,
        usage: Value,
    ) -> Result<(), ApiError> {
        let Some(id) = reservation else {
            return Ok(());
        };
        match actual {
            Some(amount) => {
                let receipt = hex_digest(&serde_json::to_vec(
                    &json!({"request_sha256":self.digest,"actual_micros":amount,"usage":usage}),
                )?);
                super::super::wallet::settle(self.state,id,amount,&receipt,json!({"source":"managed_openai_proxy","pricing":self.metadata["pricing"],"usage":usage})).await
            }
            None => super::super::wallet::hold(self.state, id, "unverifiable_provider_usage").await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Router};
    use std::sync::{
        atomic::{AtomicI64, AtomicUsize, Ordering},
        Arc,
    };
    struct FixtureBudget {
        used: AtomicI64,
        limit: i64,
    }
    #[async_trait]
    impl Budget for FixtureBudget {
        async fn reserve(&self, quote: Option<&Quote>) -> Result<Option<Uuid>, ApiError> {
            let amount = quote
                .ok_or_else(|| invalid("explicit pricing required"))?
                .maximum_micros;
            self.used
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
                    old.checked_add(amount).filter(|new| *new <= self.limit)
                })
                .map_err(|_| ApiError::Quota("member wallet limit reached".into()))?;
            Ok(Some(Uuid::new_v4()))
        }
        async fn settle(
            &self,
            _hold: Option<Uuid>,
            _actual: Option<i64>,
            _usage: Value,
        ) -> Result<(), ApiError> {
            Ok(())
        }
    }
    fn fixture(url: String) -> Plan {
        Plan {
            url,
            secret_reference: None,
            body: json!({"model":"fixture","messages":[{"role":"user","content":"Hello"}],"max_tokens":10,"stream":false}),
            pricing: Some(ModelPricing {
                currency: "USD".into(),
                input_micros_per_million: 1_000_000,
                output_micros_per_million: 1_000_000,
            }),
            quote: Some(Quote {
                currency: "USD".into(),
                maximum_micros: 100,
            }),
            input_bound: 90,
            output_bound: 10,
        }
    }
    #[tokio::test]
    async fn exhausted_and_concurrent_wallets_never_send_unreserved_provider_requests() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/chat/completions", listener.local_addr().unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let app=Router::new().route("/chat/completions",post(move||{let count=count.clone();async move {
            count.fetch_add(1,Ordering::SeqCst);tokio::time::sleep(Duration::from_millis(50)).await;
            Json(json!({"choices":[{"message":{"role":"assistant","content":"Fixture"}}],"usage":{"prompt_tokens":5,"completion_tokens":3}})) }}));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let exhausted = FixtureBudget {
            used: AtomicI64::new(100),
            limit: 100,
        };
        assert!(matches!(
            forward(fixture(url.clone()), &exhausted).await,
            Err(ApiError::Quota(_))
        ));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "over-limit requests must never contact provider"
        );
        let concurrent = FixtureBudget {
            used: AtomicI64::new(0),
            limit: 100,
        };
        let (a, b) = tokio::join!(
            forward(fixture(url.clone()), &concurrent),
            forward(fixture(url), &concurrent)
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
        let _ = server.await;
    }
    #[test]
    fn proxy_rejects_streaming_unbounded_input_and_mismatched_models() {
        let model = || ModelConfig {
            model_id: "fixture".into(),
            endpoint_resource_id: None,
            context_window: Some(8192),
            max_output_tokens: Some(1024),
            pricing: Some(ModelPricing {
                currency: "USD".into(),
                input_micros_per_million: 1000000,
                output_micros_per_million: 2000000,
            }),
        };
        let endpoint = || EndpointConfig {
            base_url: "https://models.example.invalid/v1".into(),
            protocol: "openai_compatible".into(),
        };
        let body = json!({"model":"fixture","messages":[{"role":"user","content":"Hi"}]});
        assert_eq!(
            plan(model(), endpoint(), None, body.clone()).unwrap().body["max_tokens"],
            1024
        );
        for changes in [
            json!({"stream":true}),
            json!({"n":2}),
            json!({"model":"other"}),
            json!({"max_tokens":1025}),
            json!({"messages":[{"role":"user","content":"X".repeat(9000)}]}),
            json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"https://remote.example/large-image"}}]}]}),
            json!({"messages":[{"role":"user","content":[{"type":"input_audio","input_audio":{"data":"encoded","format":"wav"}}]}]}),
            json!({"messages":[{"role":"user","content":"Hi","file_url":"https://remote.example/file"}]}),
            json!({"tools":[{"type":"web_search"}]}),
            json!({"tool_choice":{"type":"web_search"}}),
        ] {
            let mut value = body.clone();
            for (k, v) in changes.as_object().unwrap() {
                value[k] = v.clone();
            }
            assert!(plan(model(), endpoint(), None, value).is_err());
        }
    }
}
