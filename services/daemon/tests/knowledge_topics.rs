use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn topic_routes_require_owner_auth_and_roundtrip_folders_documents_and_graphs() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let p = store
        .create_project(CreateProject {
            name: "Topics".into(),
        })
        .await
        .unwrap();
    let app = router(
        AppState {
            native_token: None,
            runtime: Runtime::new(store, vec![], false).unwrap(),
            token: Arc::new("topic-test-token".into()),
            origins: Arc::new(vec!["http://127.0.0.1:5175".into()]),
        },
        None,
    );
    let path = format!("/api/v2/projects/{}/knowledge-folders", p.id);
    for (auth, origin, status) in [
        (false, None, StatusCode::UNAUTHORIZED),
        (true, Some("https://foreign.example"), StatusCode::FORBIDDEN),
        (true, None, StatusCode::OK),
    ] {
        let mut req = Request::builder().uri(&path);
        if auth {
            req = req.header(header::AUTHORIZATION, "Bearer topic-test-token");
        }
        if let Some(origin) = origin {
            req = req.header(header::ORIGIN, origin);
        }
        assert_eq!(
            app.clone()
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            status
        );
    }
    async fn call(app: &axum::Router, path: &str, method: &str, body: Value) -> Value {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .method(method)
                    .header(header::AUTHORIZATION, "Bearer topic-test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(&to_bytes(response.into_body(), 1_048_576).await.unwrap()).unwrap()
    }
    let folder = uuid::Uuid::new_v4().to_string();
    let saved = call(
        &app,
        &path,
        "PUT",
        json!({"expected_revision":0,"folders":[{"id":folder,"name":"Topic","parent_id":null}]}),
    )
    .await;
    assert_eq!(saved["revision"], 1);
    let document=call(&app,&format!("/api/v2/projects/{}/knowledge",p.id),"POST",json!({"id":null,"expected_revision":0,"kind":"knowledge","title":"NVFP4","content":"source material","tags":["precision"],"location":{"folder_id":folder}})).await;
    assert_eq!(document["folder_id"], folder);
    let graph_path = format!("/api/v2/projects/{}/knowledge-graphs", p.id);
    let graph = call(
        &app,
        &graph_path,
        "POST",
        json!({"folder_id":folder,"include_descendants":true}),
    )
    .await;
    assert_eq!(graph["documents"], 1);
    let hits = call(
        &app,
        &format!("{graph_path}/search"),
        "POST",
        json!({"folder_id":folder,"include_descendants":true,"query":"NVFP4"}),
    )
    .await;
    assert_eq!(hits["hits"][0]["document"]["id"], document["id"]);
    assert_eq!(
        call(
            &app,
            &graph_path,
            "DELETE",
            json!({"folder_id":folder,"include_descendants":true})
        )
        .await["deleted"],
        true
    );
}
