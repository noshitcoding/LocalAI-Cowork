use axum::{
    body::Body,
    http::{header, HeaderValue, Response, StatusCode},
};

const OPENAPI: &str = include_str!("../../../contracts/generated/v2/openapi.json");
const JSON_SCHEMAS: &str = include_str!("../../../contracts/generated/v2/contracts.schema.json");

pub async fn openapi() -> Response<Body> {
    json_document(OPENAPI)
}

pub async fn json_schemas() -> Response<Body> {
    json_document(JSON_SCHEMAS)
}

fn json_document(document: &'static str) -> Response<Body> {
    let mut response = Response::new(Body::from(document));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300, must-revalidate"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_contract_artifacts_are_valid_and_versioned() {
        let openapi: serde_json::Value = serde_json::from_str(OPENAPI).expect("valid OpenAPI JSON");
        let schemas: serde_json::Value =
            serde_json::from_str(JSON_SCHEMAS).expect("valid JSON Schema bundle");
        assert_eq!(openapi["openapi"], "3.1.0");
        assert_eq!(
            schemas["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert!(openapi["paths"]
            .as_object()
            .is_some_and(|paths| paths.len() > 80));
        assert!(schemas["$defs"]
            .as_object()
            .is_some_and(|defs| defs.len() > 40));
    }
}
