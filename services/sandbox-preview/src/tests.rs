use super::*;
use axum::{Json, routing::post};
use cowork_contracts::ExecutionEnvironmentRef;
use cowork_runner_protocol::SigningKey;
use serde_json::json;
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn config(key: &SigningKey, port: u16) -> Config {
    Config {
        format: 1,
        listen: "127.0.0.1:0".parse().unwrap(),
        preview_domain: "previews.example.net".into(),
        app_origin: "https://app.example.com".into(),
        namespace: "tenant-one".into(),
        service_suffix: ".tenant-one.svc.cluster.local".into(),
        allowed_ports: vec![port],
        runner_id: "runner-one".into(),
        runner_generation: 1,
        runner_public_key: key.public_key(),
        runner_url: "https://runner.example.internal".into(),
        runner_bearer_file: "/private/validator".into(),
        runner_ca_file: None,
        max_ticket_ms: 60_000,
    }
}
fn grant(port: u16) -> SandboxPreviewGrant {
    SandboxPreviewGrant {
        preview_id: "preview-one".into(),
        runner_id: "runner-one".into(),
        runner_generation: 1,
        project_id: "project-one".into(),
        workspace_resource_id: "workspace-one".into(),
        environment: ExecutionEnvironmentRef {
            environment_id: "environment".into(),
            version_id: "version".into(),
            definition_sha256: "a".repeat(64),
            qualification_sha256: "b".repeat(64),
        },
        sandbox_uid: "sandbox-one".into(),
        pod_uid: "pod-one".into(),
        namespace: "tenant-one".into(),
        service_dns: "preview-one.tenant-one.svc.cluster.local".into(),
        port,
        preview_host: "sandbox-one.previews.example.net".into(),
        issued_at: now() - 100,
        expires_at: now() + 50_000,
    }
}
fn ticket(key: &SigningKey, grant: SandboxPreviewGrant) -> String {
    URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&key.sign("sandbox_preview", grant).unwrap()).unwrap())
}
async fn serve(router: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (address, task)
}

#[test]
fn production_policy_requires_separate_sites_https_and_noncontrol_ports() {
    let key = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let good = config(&key, 8081);
    good.validate().unwrap();
    let mut bad = good.clone();
    bad.preview_domain = "previews.example.com".into();
    assert!(bad.validate().is_err());
    let mut bad = good.clone();
    bad.runner_url = "http://runner.internal".into();
    assert!(bad.validate().is_err());
    let mut bad = good.clone();
    bad.allowed_ports = vec![8080];
    assert!(bad.validate().is_err());
    let mut bad = good.clone();
    bad.service_suffix = ".other.svc.cluster.local".into();
    assert!(bad.validate().is_err());
}

#[test]
fn forged_expired_cross_host_and_unapproved_service_tickets_are_denied() {
    let key = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let gateway = Gateway {
        config: config(&key, 8081),
        validator_token: "private-validator".into(),
        client: reqwest::Client::new(),
        test_upstream: None,
    };
    let valid = ticket(&key, grant(8081));
    assert!(
        gateway
            .ticket(&valid, "sandbox-one.previews.example.net", now())
            .is_ok()
    );
    let other = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    assert!(
        gateway
            .ticket(
                &ticket(&other, grant(8081)),
                "sandbox-one.previews.example.net",
                now()
            )
            .is_err()
    );
    assert!(
        gateway
            .ticket(&valid, "sandbox-two.previews.example.net", now())
            .is_err()
    );
    for change in 0..5 {
        let mut item = grant(8081);
        match change {
            0 => {
                item.issued_at = now() - 2000;
                item.expires_at = now() - 1
            }
            1 => item.namespace = "tenant-other".into(),
            2 => item.service_dns = "metadata.google.internal".into(),
            3 => item.port = 8080,
            _ => item.expires_at = item.issued_at + 60_001,
        };
        assert!(
            gateway
                .ticket(
                    &ticket(&key, item),
                    "sandbox-one.previews.example.net",
                    now()
                )
                .is_err()
        );
    }
}

#[tokio::test]
async fn real_proxy_authenticates_each_request_and_strips_credentials_cookies_and_external_redirects()
 {
    let app=Router::new().route("/",get(|headers:HeaderMap|async move{
        let mut response=Json(json!({"authorization":headers.get("authorization").and_then(|v|v.to_str().ok()),"cookie":headers.get("cookie").and_then(|v|v.to_str().ok()),"forwarded":headers.get("x-forwarded-host").and_then(|v|v.to_str().ok()),"accept":headers.get("accept").and_then(|v|v.to_str().ok())})).into_response();
        response.headers_mut().insert(header::SET_COOKIE,HeaderValue::from_static("app_secret=bad; Domain=example.com"));response
    })).route("/external",get(||async{(StatusCode::FOUND,[(header::LOCATION,"https://app.example.com/steal")])})).route("/local",get(||async{(StatusCode::FOUND,[(header::LOCATION,"/next?safe=1")])}));
    let (application, application_task) = serve(app).await;
    let checks = Arc::new(AtomicUsize::new(0));
    let revoked = Arc::new(AtomicBool::new(false));
    let calls = checks.clone();
    let blocked = revoked.clone();
    let validator = Router::new().route(
        "/v2/sandbox-previews/validate",
        post(
            move |headers: HeaderMap, Json(value): Json<Signed<SandboxPreviewGrant>>| {
                let calls = calls.clone();
                let blocked = blocked.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    if headers
                        .get(header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        != Some("Bearer private-validator")
                        || blocked.load(Ordering::SeqCst)
                        || value.payload.sandbox_uid != "sandbox-one"
                        || value.payload.pod_uid != "pod-one"
                    {
                        StatusCode::FORBIDDEN
                    } else {
                        StatusCode::NO_CONTENT
                    }
                }
            },
        ),
    );
    let (validation, validator_task) = serve(validator).await;
    let key = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let mut policy = config(&key, application.port());
    policy.runner_url = format!("http://{validation}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve("preview-one.tenant-one.svc.cluster.local", application)
        .build()
        .unwrap();
    // HTTP is allowed only by this in-module fixture. Production construction
    // calls Config::validate and requires the pinned HTTPS validator origin.
    let gateway = Arc::new(Gateway {
        config: policy,
        validator_token: "private-validator".into(),
        client,
        test_upstream: None,
    });
    let (address, gateway_task) = serve(gateway.router()).await;
    let browser = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let raw = ticket(&key, grant(application.port()));
    let origin = "https://sandbox-one.previews.example.net";
    let bootstrap = browser
        .post(format!("http://{address}/_cowork/bootstrap"))
        .header(header::HOST, "sandbox-one.previews.example.net")
        .header(header::ORIGIN, origin)
        .json(&json!({"ticket":raw}))
        .send()
        .await
        .unwrap();
    assert_eq!(bootstrap.status(), StatusCode::NO_CONTENT);
    let cookie = bootstrap
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        cookie.contains("HttpOnly")
            && cookie.contains("Secure")
            && cookie.contains("SameSite=Strict")
            && !cookie.contains("Domain=")
    );
    let response = browser
        .get(format!("http://{address}/"))
        .header(header::HOST, "sandbox-one.previews.example.net")
        .header(
            header::COOKIE,
            format!("{COOKIE}={raw}; app_secret=must-not-forward"),
        )
        .header(header::AUTHORIZATION, "Bearer must-not-forward")
        .header("x-forwarded-host", "must-not-forward")
        .header(header::ACCEPT, "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    assert_eq!(
        response.headers().get("referrer-policy").unwrap(),
        "no-referrer"
    );
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["authorization"].is_null() && body["cookie"].is_null() && body["forwarded"].is_null()
    );
    assert_eq!(body["accept"], "text/html");
    for (path, status) in [
        ("external", StatusCode::BAD_GATEWAY),
        ("local", StatusCode::FOUND),
    ] {
        let response = browser
            .get(format!("http://{address}/{path}"))
            .header(header::HOST, "sandbox-one.previews.example.net")
            .header(header::COOKIE, format!("{COOKIE}={raw}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        if path == "local" {
            assert_eq!(
                response.headers().get(header::LOCATION).unwrap(),
                "https://sandbox-one.previews.example.net/next?safe=1"
            );
        }
    }
    let cross = browser
        .get(format!("http://{address}/"))
        .header(header::HOST, "sandbox-two.previews.example.net")
        .header(header::COOKIE, format!("{COOKIE}={raw}"))
        .send()
        .await
        .unwrap();
    assert_eq!(cross.status(), StatusCode::UNAUTHORIZED);
    let csrf = browser
        .post(format!("http://{address}/"))
        .header(header::HOST, "sandbox-one.previews.example.net")
        .header(header::ORIGIN, "https://sandbox-two.previews.example.net")
        .header(header::COOKIE, format!("{COOKIE}={raw}"))
        .send()
        .await
        .unwrap();
    assert_eq!(csrf.status(), StatusCode::FORBIDDEN);
    revoked.store(true, Ordering::SeqCst);
    let denied = browser
        .get(format!("http://{address}/"))
        .header(header::HOST, "sandbox-one.previews.example.net")
        .header(header::COOKIE, format!("{COOKIE}={raw}"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(checks.load(Ordering::SeqCst), 5);
    for task in [application_task, validator_task, gateway_task] {
        task.abort();
        let _ = task.await;
    }
}

#[tokio::test]
async fn real_websocket_forwards_hmr_frames_without_credentials_and_closes_when_revoked() {
    let credential_leak = Arc::new(AtomicBool::new(false));
    let leak = credential_leak.clone();
    let app = Router::new().route(
        "/socket",
        get(move |upgrade: WebSocketUpgrade, headers: HeaderMap| {
            let leak = leak.clone();
            async move {
                if headers.contains_key(header::COOKIE)
                    || headers.contains_key(header::AUTHORIZATION)
                {
                    leak.store(true, Ordering::SeqCst);
                }
                upgrade
                    .protocols(["vite-hmr"])
                    .on_upgrade(|mut socket| async move {
                        while let Some(Ok(message)) = socket.recv().await {
                            if socket.send(message).await.is_err() {
                                break;
                            }
                        }
                    })
            }
        }),
    );
    let (application, app_task) = serve(app).await;
    let revoked = Arc::new(AtomicBool::new(false));
    let blocked = revoked.clone();
    let validator = Router::new().route(
        "/v2/sandbox-previews/validate",
        post(move || {
            let blocked = blocked.clone();
            async move {
                if blocked.load(Ordering::SeqCst) {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (validation, validation_task) = serve(validator).await;
    let key = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let mut policy = config(&key, application.port());
    policy.runner_url = format!("http://{validation}");
    let gateway = Arc::new(Gateway {
        config: policy,
        validator_token: "private-validator".into(),
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        test_upstream: Some(application),
    });
    let (address, gateway_task) = serve(gateway.router()).await;
    let mut request = "ws://sandbox-one.previews.example.net/socket"
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        header::COOKIE,
        HeaderValue::from_str(&format!(
            "{COOKIE}={}",
            ticket(&key, grant(application.port()))
        ))
        .unwrap(),
    );
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer must-not-leak"),
    );
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("https://sandbox-one.previews.example.net"),
    );
    request.headers_mut().insert(
        "sec-websocket-protocol",
        HeaderValue::from_static("vite-hmr"),
    );
    let (mut browser, response) = tokio_tungstenite::client_async(
        request,
        tokio::net::TcpStream::connect(address).await.unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        response.headers().get("sec-websocket-protocol").unwrap(),
        "vite-hmr"
    );
    browser
        .send(GuestMessage::Text("hmr-update".into()))
        .await
        .unwrap();
    let message = tokio::time::timeout(std::time::Duration::from_secs(3), browser.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(message.to_text().unwrap(), "hmr-update");
    assert!(!credential_leak.load(Ordering::SeqCst));
    revoked.store(true, Ordering::SeqCst);
    let closed = tokio::time::timeout(std::time::Duration::from_secs(5), browser.next())
        .await
        .unwrap();
    assert!(
        closed.is_none()
            || closed
                .is_some_and(|value| value.is_err() || matches!(value, Ok(GuestMessage::Close(_))))
    );
    revoked.store(false, Ordering::SeqCst);
    let mut expiring = grant(application.port());
    expiring.issued_at = now() - 1;
    expiring.expires_at = now() + 1200;
    let mut request = "ws://sandbox-one.previews.example.net/socket"
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("{COOKIE}={}", ticket(&key, expiring))).unwrap(),
    );
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("https://sandbox-one.previews.example.net"),
    );
    let (mut expiring, _) = tokio_tungstenite::client_async(
        request,
        tokio::net::TcpStream::connect(address).await.unwrap(),
    )
    .await
    .unwrap();
    let expired = tokio::time::timeout(std::time::Duration::from_secs(3), expiring.next())
        .await
        .unwrap();
    assert!(
        expired.is_none()
            || expired
                .is_some_and(|value| value.is_err() || matches!(value, Ok(GuestMessage::Close(_))))
    );
    for task in [app_task, validation_task, gateway_task] {
        task.abort();
        let _ = task.await;
    }
}
