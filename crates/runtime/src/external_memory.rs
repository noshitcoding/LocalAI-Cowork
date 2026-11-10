//! Explicit, once-only external retrieval. Responses become reviewed local copies.
use super::*;
use cowork_secrets::KeyStore;
use cowork_storage::library_config::LibraryOwner;
use futures_util::StreamExt;
const MAX_BYTES: usize = 1024 * 1024;
pub async fn retrieve(
    store: &Store,
    owner: LibraryOwner<'_>,
    id: &str,
    input: &RetrieveExternalMemory,
    keys: &dyn KeyStore,
) -> Result<ExternalMemoryRetrieval> {
    let (provider, secret) = match store
        .external_memory_retrieval_admit(owner, id, input, keys)
        .await?
    {
        Ok(v) => v,
        Err(receipt) => return Ok(receipt),
    };
    // A committed admission is never replayed, even after network/parse failure.
    cowork_storage::external_memory::validate(&provider.value)?;
    let url = reqwest::Url::parse(&provider.value.endpoint).map_err(|_| Error::Forbidden)?;
    let mut client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .pool_max_idle_per_host(0)
        .http1_only()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10));
    if matches!(owner, LibraryOwner::Actor(_)) {
        let addresses=cowork_mcp::public_https_addresses(&url).await.map_err(|_|Error::Validation("Server-account external memory requires a public HTTPS hostname on port 443 resolving only to public addresses".into()))?;
        client = client.resolve_to_addrs(url.host_str().ok_or(Error::Forbidden)?, &addresses);
    }
    let client = client
        .build()
        .map_err(|_| Error::Provider("external_memory_transport_unavailable".into()))?;
    let method = if provider.value.method == "GET" {
        reqwest::Method::GET
    } else {
        reqwest::Method::POST
    };
    let mut request = client
        .request(method, url)
        .header(reqwest::header::ACCEPT, "application/json");
    if provider.value.auth_scheme != "none" {
        let secret = secret.as_ref().ok_or(Error::NativeKeyUnavailable)?;
        request = request.header(
            reqwest::header::AUTHORIZATION,
            format!("{} {}", provider.value.auth_scheme, secret.as_str()),
        );
    }
    if let Some(body) = &provider.value.request_body {
        request = request.json(body);
    }
    let response = request
        .send()
        .await
        .map_err(|_| Error::Provider("external_memory_dispatch_uncertain_do_not_retry".into()))?;
    if !response.status().is_success() {
        return Err(Error::Provider(format!(
            "external_memory_http_{}_not_replayed",
            response.status().as_u16()
        )));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err(Error::BudgetExceeded);
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| Error::UncertainEffect)?;
        if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err(Error::BudgetExceeded);
        }
        bytes.extend_from_slice(&chunk);
    }
    let raw: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Error::Validation("External memory must return JSON".into()))?;
    let values = raw
        .pointer(&provider.value.items_pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::Validation("External memory items pointer must select an array".into())
        })?;
    if values.len() > 200 {
        return Err(Error::BudgetExceeded);
    }
    let mut ids = std::collections::HashSet::new();
    let mut items = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let content = value
            .get(&provider.value.content_field)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::Validation("External memory content field must be text".into())
            })?;
        valid_text(content, 64000)?;
        let item_id = if let Some(field) = &provider.value.id_field {
            value
                .get(field)
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Validation("External memory ID field must be text".into()))?
                .to_owned()
        } else {
            index.to_string()
        };
        valid_text(&item_id, 200)?;
        if !ids.insert(item_id.clone()) {
            return Err(Error::Validation(
                "External memory item IDs must be unique".into(),
            ));
        }
        items.push(ExternalMemoryItem {
            id: item_id,
            content: content.into(),
        });
    }
    let receipt = ExternalMemoryRetrieval {
        request_id: input.request_id,
        provider_id: id.into(),
        provider_revision: provider.revision,
        endpoint: provider.value.endpoint,
        response_sha256: hash(&bytes),
        retrieved_at: now(),
        items,
    };
    store
        .external_memory_received(&owner.namespace(), &receipt)
        .await?;
    Ok(receipt)
}
impl Runtime {
    fn external_memory_keys(&self) -> Result<&personal_provider_credentials::PersonalProviderKeys> {
        self.personal_provider_keys
            .as_ref()
            .ok_or(Error::NativeKeyUnavailable)
    }
    pub async fn write_external_memory_provider(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
        input: &WriteExternalMemoryProvider,
    ) -> Result<ExternalMemoryProvider> {
        let access = self.external_memory_keys()?;
        self.store
            .write_external_memory_provider(
                owner,
                id,
                input,
                access.keys.as_ref(),
                access.key_id,
                self.native_keys.is_none(),
            )
            .await
    }
    pub async fn retrieve_external_memory(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
        input: &RetrieveExternalMemory,
    ) -> Result<ExternalMemoryRetrieval> {
        retrieve(
            &self.store,
            owner,
            id,
            input,
            self.external_memory_keys()?.keys.as_ref(),
        )
        .await
    }
}
