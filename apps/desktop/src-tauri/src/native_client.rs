use cowork_contracts::ApiError;
use serde::{Serialize, de::DeserializeOwned};
use tauri::Manager;

pub fn failure(code: &str) -> ApiError {
    ApiError {
        code: code.into(),
        message: "The native operation was not confirmed".into(),
        retryable: false,
        correlation_id: uuid::Uuid::new_v4().to_string(),
    }
}

/// No renderer-supplied endpoint, credentials, headers or arbitrary request path.
pub struct NativeClient {
    client: reqwest::Client,
    owner: String,
    native: String,
    connection_error: &'static str,
    mutation_error: &'static str,
}
impl NativeClient {
    pub fn new(
        window: &tauri::WebviewWindow,
        connection_error: &'static str,
        mutation_error: &'static str,
    ) -> Result<Self, ApiError> {
        if !super::application_window(window) {
            return Err(failure("forbidden"));
        }
        let dir =
            super::private_data_dir(window.app_handle()).map_err(|_| failure(connection_error))?;
        let owner = std::fs::read_to_string(dir.join("daemon.token"))
            .map_err(|_| failure(connection_error))?
            .trim()
            .to_owned();
        let native = std::fs::read_to_string(dir.join("native.token"))
            .map_err(|_| failure(connection_error))?
            .trim()
            .to_owned();
        if !(48..=256).contains(&owner.len())
            || !(48..=256).contains(&native.len())
            || owner == native
        {
            return Err(failure("forbidden"));
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|_| failure(connection_error))?;
        Ok(Self {
            client,
            owner,
            native,
            connection_error,
            mutation_error,
        })
    }
    pub fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("http://127.0.0.1:43821/api/v2{path}"))
            .bearer_auth(&self.owner)
            .header("x-cowork-native", &self.native)
    }
    pub async fn result<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        mutation: bool,
    ) -> Result<T, ApiError> {
        let code = if mutation {
            self.mutation_error
        } else {
            self.connection_error
        };
        let mut response = request.send().await.map_err(|_| failure(code))?;
        let success = response.status().is_success();
        const LIMIT: usize = 4 * 1024 * 1024;
        if response.content_length().is_some_and(|n| n > LIMIT as u64) {
            return Err(failure(code));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| failure(code))? {
            if chunk.len() > LIMIT - bytes.len() {
                return Err(failure(code));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !success {
            return Err(serde_json::from_slice(&bytes).unwrap_or_else(|_| failure(code)));
        }
        serde_json::from_slice(&bytes).map_err(|_| failure(code))
    }
    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        input: &impl Serialize,
    ) -> Result<T, ApiError> {
        let body = serde_json::to_vec(input).map_err(|_| failure("invalid_request"))?;
        self.result(
            self.request(reqwest::Method::POST, path)
                .header("content-type", "application/json")
                .body(body),
            true,
        )
        .await
    }
}
