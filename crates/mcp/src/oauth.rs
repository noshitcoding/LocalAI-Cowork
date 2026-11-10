//! OAuth wire primitives. Token POSTs have no retry or authorization here: the
//! product must commit and recheck an exact durable exchange before dispatch.
use crate::{Error, Result};
use futures_util::StreamExt;
use reqwest::{Client, Url};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, net::IpAddr, time::Duration};
use zeroize::{Zeroize, Zeroizing};
#[derive(Clone, Copy)]
pub enum OAuthNetwork {
    NativeOwner,
    PublicServer,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub scopes_supported: Vec<String>,
    pub token_endpoint_auth_methods_supported: Vec<String>,
    pub original: Value,
    pub sha256: String,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProtectedResourceMetadata {
    pub resource: String,
    pub metadata_uri: String,
    pub authorization_servers: Vec<AuthorizationMetadata>,
    pub scopes_supported: Vec<String>,
    pub suggested_scopes: Vec<String>,
    pub original: Value,
    pub sha256: String,
}
fn hash(v: &Value) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(v).expect("JSON")))
}
fn endpoint(raw: &str, http_loopback: bool) -> Result<Url> {
    url(raw, http_loopback, false)
}
fn url(raw: &str, http_loopback: bool, query: bool) -> Result<Url> {
    let u = Url::parse(raw).map_err(|_| Error::Configuration)?;
    let loopback = u.host_str().is_some_and(|h| {
        h.trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    // Url inserts a root slash for a valid bare authority. Preserve the issuer
    // string in metadata/hashes; allow only this particular parser normalization.
    let original = u.as_str() == raw
        || u.path() == "/" && u.query().is_none() && u.as_str().strip_suffix('/') == Some(raw);
    if !(u.scheme() == "https" || http_loopback && u.scheme() == "http" && loopback)
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
        || !query && u.query().is_some()
        || !original
        || raw.len() > 2048
    {
        return Err(Error::Configuration);
    }
    Ok(u)
}
fn scopes(value: Option<&Value>) -> Result<Vec<String>> {
    let Some(v) = value else {
        return Ok(vec![]);
    };
    let a = v
        .as_array()
        .filter(|a| a.len() <= 64)
        .ok_or(Error::Protocol)?;
    let mut result = Vec::new();
    for v in a {
        let s = v
            .as_str()
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 128
                    && s.bytes()
                        .all(|b| (0x21..=0x7e).contains(&b) && b != b'"' && b != b'\\')
            })
            .ok_or(Error::Protocol)?;
        if result.iter().any(|r| r == s) {
            return Err(Error::Protocol);
        }
        result.push(s.to_owned());
    }
    Ok(result)
}
async fn client(url: &Url, network: OAuthNetwork) -> Result<Client> {
    let mut b = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .pool_max_idle_per_host(0)
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20));
    if matches!(network, OAuthNetwork::PublicServer) {
        let h = url.host_str().ok_or(Error::Configuration)?;
        let port = url.port_or_known_default().ok_or(Error::Configuration)?;
        let addresses =
            tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((h, port)))
                .await
                .map_err(|_| Error::Deadline)?
                .map_err(|_| Error::Transport)?
                .collect::<Vec<_>>();
        if addresses.is_empty()
            || addresses.len() > 64
            || addresses
                .iter()
                .any(|a| !super::transport::public_address(a.ip()))
        {
            return Err(Error::Configuration);
        }
        b = b.resolve_to_addrs(h, &addresses);
    }
    b.build().map_err(|_| Error::Configuration)
}
async fn bytes(response: reqwest::Response) -> Result<(u16, Vec<u8>)> {
    let status = response.status().as_u16();
    if response.content_length().is_some_and(|n| n > 131_072) {
        return Err(Error::Limit);
    }
    let mut result = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(c) = stream.next().await {
        let c = c.map_err(|_| Error::Transport)?;
        if result.len() + c.len() > 131_072 {
            return Err(Error::Limit);
        }
        result.extend_from_slice(&c);
    }
    Ok((status, result))
}
async fn document(uri: &str, network: OAuthNetwork, resource: bool) -> Result<Value> {
    let url = url(
        uri,
        resource && matches!(network, OAuthNetwork::NativeOwner),
        true,
    )?;
    let (status, b) = bytes(
        client(&url, network)
            .await?
            .get(url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|_| Error::Transport)?,
    )
    .await?;
    if status != 200 {
        return Err(Error::Remote);
    }
    crate::wire::parse(&b)
}
fn challenge(headers: &reqwest::header::HeaderMap) -> Result<Option<BTreeMap<String, String>>> {
    let mut found = None;
    for h in headers.get_all("www-authenticate") {
        let raw = h.to_str().map_err(|_| Error::Protocol)?;
        if raw.len() > 8192 {
            return Err(Error::Limit);
        }
        let Some((scheme, mut input)) = raw.split_once(' ') else {
            continue;
        };
        if !scheme.eq_ignore_ascii_case("bearer") {
            continue;
        }
        let mut fields = BTreeMap::new();
        while !input.trim().is_empty() {
            input = input.trim_start_matches([' ', ',']);
            let (name, rest) = input.split_once('=').ok_or(Error::Protocol)?;
            let name = name.trim().to_ascii_lowercase();
            if name.is_empty() || name.len() > 128 || fields.contains_key(&name) {
                return Err(Error::Protocol);
            }
            let rest = rest.trim_start();
            let (value, tail) = if let Some(rest) = rest.strip_prefix('"') {
                let mut value = String::new();
                let mut escape = false;
                let mut end = None;
                for (i, c) in rest.char_indices() {
                    if escape {
                        value.push(c);
                        escape = false;
                    } else if c == '\\' {
                        escape = true;
                    } else if c == '"' {
                        end = Some(i);
                        break;
                    } else {
                        value.push(c);
                    }
                }
                let i = end.ok_or(Error::Protocol)?;
                (value, &rest[i + 1..])
            } else {
                let i = rest.find(',').unwrap_or(rest.len());
                (rest[..i].trim().to_owned(), &rest[i..])
            };
            if value.chars().any(char::is_control) {
                return Err(Error::Protocol);
            }
            fields.insert(name, value);
            input = tail;
        }
        if found.replace(fields).is_some() {
            return Err(Error::Protocol);
        }
    }
    Ok(found)
}
pub async fn discover(resource: &str, network: OAuthNetwork) -> Result<ProtectedResourceMetadata> {
    let uri = endpoint(resource, matches!(network, OAuthNetwork::NativeOwner))?;
    // Read-only challenge observation; no credentials or initialize/tool request.
    let response = client(&uri, network)
        .await?
        .get(uri.clone())
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| Error::Transport)?;
    let challenge = challenge(response.headers())?;
    drop(response);
    let mut candidates = Vec::new();
    if let Some(metadata) = challenge.as_ref().and_then(|v| v.get("resource_metadata")) {
        url(metadata, matches!(network, OAuthNetwork::NativeOwner), true)?;
        candidates.push(metadata.clone());
    } else {
        let mut at = uri.clone();
        at.set_path(&format!(
            "/.well-known/oauth-protected-resource{}",
            uri.path().trim_end_matches('/')
        ));
        candidates.push(at.to_string());
        let mut base = uri.clone();
        base.set_path("/.well-known/oauth-protected-resource");
        if !candidates.contains(&base.to_string()) {
            candidates.push(base.to_string());
        }
    }
    let mut selected = None;
    for candidate in candidates {
        match document(&candidate, network, true).await {
            Ok(v) => {
                selected = Some((candidate, v));
                break;
            }
            Err(Error::Remote) => continue,
            Err(e) => return Err(e),
        }
    }
    let (metadata_uri, original) = selected.ok_or(Error::Remote)?;
    if original.get("resource").and_then(Value::as_str) != Some(resource) {
        return Err(Error::Protocol);
    }
    let servers = original
        .get("authorization_servers")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty() && a.len() <= 4)
        .ok_or(Error::Protocol)?;
    let mut authorization_servers = Vec::new();
    for issuer in servers {
        let issuer = issuer.as_str().ok_or(Error::Protocol)?;
        authorization_servers.push(discover_authority(issuer, network).await?);
    }
    let supported = scopes(original.get("scopes_supported"))?;
    let suggested = if let Some(scope) = challenge.as_ref().and_then(|c| c.get("scope")) {
        scopes(Some(&json!(
            scope.split_ascii_whitespace().collect::<Vec<_>>()
        )))?
    } else {
        supported.clone()
    };
    let sha256 = hash(&json!([
        1,
        resource,
        metadata_uri,
        original,
        authorization_servers
    ]));
    Ok(ProtectedResourceMetadata {
        resource: resource.into(),
        metadata_uri,
        authorization_servers,
        scopes_supported: supported,
        suggested_scopes: suggested,
        original,
        sha256,
    })
}
async fn discover_authority(issuer: &str, network: OAuthNetwork) -> Result<AuthorizationMetadata> {
    let u = endpoint(issuer, false)?;
    let suffix = u.path().trim_end_matches('/');
    let mut candidates = Vec::new();
    for path in [
        format!("/.well-known/oauth-authorization-server{suffix}"),
        format!("/.well-known/openid-configuration{suffix}"),
        format!("{suffix}/.well-known/openid-configuration"),
    ] {
        let mut v = u.clone();
        v.set_path(&path);
        if !candidates.contains(&v.to_string()) {
            candidates.push(v.to_string());
        }
    }
    let mut result = None;
    for uri in candidates {
        match document(&uri, network, false).await {
            Ok(v) => {
                result = Some(v);
                break;
            }
            Err(Error::Remote) => continue,
            Err(e) => return Err(e),
        }
    }
    let original = result.ok_or(Error::Remote)?;
    if original.get("issuer").and_then(Value::as_str) != Some(issuer)
        || !original
            .get("code_challenge_methods_supported")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|v| v == "S256"))
        || !original
            .get("response_types_supported")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|v| v == "code"))
    {
        return Err(Error::Protocol);
    }
    if original.get("grant_types_supported").is_some_and(|v| {
        !v.as_array()
            .is_some_and(|a| a.iter().any(|v| v == "authorization_code"))
    }) {
        return Err(Error::Protocol);
    }
    let auth = original
        .get("authorization_endpoint")
        .and_then(Value::as_str)
        .ok_or(Error::Protocol)?
        .to_owned();
    let token = original
        .get("token_endpoint")
        .and_then(Value::as_str)
        .ok_or(Error::Protocol)?
        .to_owned();
    url(&auth, false, true)?;
    url(&token, false, true)?;
    let methods = match original.get("token_endpoint_auth_methods_supported") {
        None => vec!["client_secret_basic".into()],
        Some(v) => scopes(Some(v))?,
    };
    Ok(AuthorizationMetadata {
        issuer: issuer.into(),
        authorization_endpoint: auth,
        token_endpoint: token,
        scopes_supported: scopes(original.get("scopes_supported"))?,
        token_endpoint_auth_methods_supported: methods,
        sha256: hash(&original),
        original,
    })
}
pub struct AuthorizationStart {
    pub authorization_url: String,
    pub state: Zeroizing<String>,
    pub verifier: Zeroizing<String>,
}
fn random() -> Result<Zeroizing<String>> {
    let mut b = [0u8; 32];
    SystemRandom::new()
        .fill(&mut b)
        .map_err(|_| Error::Configuration)?;
    Ok(Zeroizing::new(
        b.iter().map(|b| format!("{b:02x}")).collect(),
    ))
}
fn base64url(bytes: &[u8]) -> String {
    const C: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut bits = 0u32;
    let mut n = 0;
    for b in bytes {
        bits = (bits << 8) | *b as u32;
        n += 8;
        while n >= 6 {
            n -= 6;
            out.push(C[((bits >> n) & 63) as usize] as char);
        }
    }
    if n > 0 {
        out.push(C[((bits << (6 - n)) & 63) as usize] as char);
    }
    out
}
pub fn start(
    metadata: &AuthorizationMetadata,
    client_id: &str,
    redirect: &str,
    resource: &str,
    selected_scopes: &[String],
) -> Result<AuthorizationStart> {
    if client_id.is_empty()
        || client_id.len() > 512
        || scopes(Some(&json!(selected_scopes)))? != selected_scopes
    {
        return Err(Error::Configuration);
    }
    let r = endpoint(redirect, true)?;
    let loopback = r.host_str().is_some_and(|h| {
        h.trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if r.scheme() != "https" && !loopback {
        return Err(Error::Configuration);
    }
    endpoint(resource, true)?;
    let state = random()?;
    let verifier = random()?;
    let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
    let mut auth = url(&metadata.authorization_endpoint, false, true)?;
    if auth.query_pairs().any(|(k, _)| {
        [
            "response_type",
            "client_id",
            "redirect_uri",
            "state",
            "code_challenge",
            "code_challenge_method",
            "resource",
            "scope",
        ]
        .contains(&k.as_ref())
    }) {
        return Err(Error::Configuration);
    }
    auth.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("resource", resource);
    if !selected_scopes.is_empty() {
        auth.query_pairs_mut()
            .append_pair("scope", &selected_scopes.join(" "));
    }
    Ok(AuthorizationStart {
        authorization_url: auth.to_string(),
        state,
        verifier,
    })
}
pub struct TokenRequest {
    endpoint: String,
    client_id: String,
    client_secret: Option<Zeroizing<String>>,
    form: BTreeMap<String, Zeroizing<String>>,
    sha256: String,
}
impl TokenRequest {
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn authorization_code(
        metadata: &AuthorizationMetadata,
        client: &str,
        secret: Option<Zeroizing<String>>,
        redirect: &str,
        resource: &str,
        code: Zeroizing<String>,
        verifier: Zeroizing<String>,
    ) -> Result<Self> {
        Self::new(
            metadata,
            client,
            secret,
            [
                ("grant_type", Zeroizing::new("authorization_code".into())),
                ("redirect_uri", Zeroizing::new(redirect.into())),
                ("resource", Zeroizing::new(resource.into())),
                ("code", code),
                ("code_verifier", verifier),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
        )
    }
    pub fn refresh(
        metadata: &AuthorizationMetadata,
        client: &str,
        secret: Option<Zeroizing<String>>,
        resource: &str,
        refresh: Zeroizing<String>,
        scope: &[String],
    ) -> Result<Self> {
        Self::new(
            metadata,
            client,
            secret,
            [
                ("grant_type", Zeroizing::new("refresh_token".into())),
                ("resource", Zeroizing::new(resource.into())),
                ("refresh_token", refresh),
                ("scope", Zeroizing::new(scope.join(" "))),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
        )
    }
    fn new(
        m: &AuthorizationMetadata,
        client: &str,
        secret: Option<Zeroizing<String>>,
        form: BTreeMap<String, Zeroizing<String>>,
    ) -> Result<Self> {
        let token = url(&m.token_endpoint, false, true)?;
        if token.query_pairs().any(|(k, _)| {
            [
                "client_id",
                "client_secret",
                "grant_type",
                "code",
                "code_verifier",
                "refresh_token",
                "resource",
                "scope",
            ]
            .contains(&k.as_ref())
        }) {
            return Err(Error::Configuration);
        }
        let method = if secret.is_some() {
            "client_secret_basic"
        } else {
            "none"
        };
        if !m
            .token_endpoint_auth_methods_supported
            .iter()
            .any(|v| v == method)
            || client.is_empty()
            || client.len() > 512
            || secret.as_ref().is_some_and(|s| s.len() > 8192)
            || form.values().any(|v| {
                v.is_empty()
                    && form
                        .get("grant_type")
                        .is_some_and(|g| g.as_str() == "authorization_code")
                    || v.len() > 8192
                    || v.contains('\0')
            })
        {
            return Err(Error::Configuration);
        }
        let sha256 = hash(&json!([
            1,
            m.issuer,
            m.sha256,
            m.token_endpoint,
            client,
            secret.as_ref().map(|s| hash(&json!(s.as_str()))),
            form.iter()
                .map(|(k, v)| (k, hash(&json!(v.as_str()))))
                .collect::<Vec<_>>()
        ]));
        Ok(Self {
            endpoint: m.token_endpoint.clone(),
            client_id: client.into(),
            client_secret: secret,
            form,
            sha256,
        })
    }
    /// Exactly one POST consumes this in-memory request. HTTP/body errors have
    /// unknown remote token effects; callers preserve the already committed intent.
    pub async fn dispatch(self, network: OAuthNetwork) -> Result<OAuthResponse> {
        let url = url(&self.endpoint, false, true)?;
        let client = client(&url, network).await?;
        let fields = self
            .form
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .chain(std::iter::once(("client_id", self.client_id.as_str())))
            .collect::<Vec<_>>();
        let mut request = client
            .post(url)
            .header("Accept", "application/json")
            .form(&fields);
        if let Some(secret) = &self.client_secret {
            // RFC6749 2.3.1 encodes both values before HTTP Basic base64.
            let encoded_client = form_encoded(&self.client_id)?;
            let encoded_secret = form_encoded(secret)?;
            request = request.basic_auth(encoded_client.as_str(), Some(encoded_secret.as_str()));
        }
        let (status, body) = bytes(request.send().await.map_err(|_| Error::Transport)?).await?;
        let body = Zeroizing::new(body);
        let value = crate::wire::parse(&body)?;
        Ok(OAuthResponse {
            status,
            body: value,
        })
    }
}
pub struct OAuthResponse {
    pub status: u16,
    pub body: Value,
}
fn form_encoded(value: &str) -> Result<Zeroizing<String>> {
    let mut u = Url::parse("https://encode.invalid/").map_err(|_| Error::Configuration)?;
    u.query_pairs_mut().append_pair("v", value);
    Ok(Zeroizing::new(
        u.query()
            .and_then(|s| s.strip_prefix("v="))
            .ok_or(Error::Configuration)?
            .into(),
    ))
}
impl Drop for OAuthResponse {
    fn drop(&mut self) {
        fn wipe(v: &mut Value) {
            match v {
                Value::String(s) => s.zeroize(),
                Value::Array(a) => a.iter_mut().for_each(wipe),
                Value::Object(o) => o.values_mut().for_each(wipe),
                _ => {}
            }
        }
        wipe(&mut self.body);
    }
}
pub struct TokenValues {
    pub access: Zeroizing<String>,
    pub refresh: Option<Zeroizing<String>>,
    pub client_use_ms: u64,
    pub scopes: Vec<String>,
}
impl OAuthResponse {
    pub fn token_values(&self, requested: &[String]) -> Result<TokenValues> {
        if self.status != 200
            || !self
                .body
                .get("token_type")
                .and_then(Value::as_str)
                .is_some_and(|s| s.eq_ignore_ascii_case("bearer"))
        {
            return Err(Error::Remote);
        }
        let token = |name: &str| -> Result<Zeroizing<String>> {
            let s = self
                .body
                .get(name)
                .and_then(Value::as_str)
                .filter(|s| {
                    !s.is_empty()
                        && s.len() <= 4096
                        && s.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
                })
                .ok_or(Error::Protocol)?;
            Ok(Zeroizing::new(s.into()))
        };
        let access = token("access_token")?;
        let refresh = self
            .body
            .get("refresh_token")
            .map(|_| token("refresh_token"))
            .transpose()?;
        let client_use_ms = match self.body.get("expires_in") {
            None => 300_000,
            Some(v) => {
                v.as_u64()
                    .filter(|s| *s > 0)
                    .ok_or(Error::Protocol)?
                    .min(3600)
                    * 1000
            }
        };
        let granted = match self.body.get("scope") {
            None => requested.to_vec(),
            Some(v) => {
                let raw = v
                    .as_str()
                    .filter(|s| s.len() <= 8192)
                    .ok_or(Error::Protocol)?;
                scopes(Some(&json!(
                    raw.split_ascii_whitespace().collect::<Vec<_>>()
                )))?
            }
        };
        if granted.iter().any(|s| !requested.contains(s)) {
            return Err(Error::Protocol);
        }
        Ok(TokenValues {
            access,
            refresh,
            client_use_ms,
            scopes: granted,
        })
    }
}
