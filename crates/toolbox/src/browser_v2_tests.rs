use super::*;
use cowork_contracts::*;
use serde_json::json;
fn fixture() -> (PathBuf, Arc<Toolbox>) {
    let root = std::env::temp_dir().join(format!("cowork-browser-v2-{}", Uuid::new_v4()));
    let executable = std::env::current_exe().unwrap();
    let service = Toolbox::open(Config {
        root: root.clone(),
        programs: BTreeMap::from([
            ("python".into(), executable.clone()),
            ("chrome".into(), executable.clone()),
            ("firefox".into(), executable),
        ]),
        environment: BTreeMap::new(),
        github_token: None,
    })
    .unwrap();
    (root, service)
}
fn managed(engine: BrowserEngine) -> CreateBrowserSession {
    CreateBrowserSession {
        engine,
        ..Default::default()
    }
}
#[test]
fn project_engine_profiles_are_exclusive_and_only_explicitly_deleted() {
    let (root, service) = fixture();
    let one = service
        .create_browser_v2(
            "00000000-0000-4000-8000-000000000001",
            "00000000-0000-4000-8000-000000000004",
            managed(BrowserEngine::Chrome),
        )
        .unwrap();
    assert!(
        service
            .create_browser_v2(
                "00000000-0000-4000-8000-000000000001",
                "00000000-0000-4000-8000-000000000005",
                managed(BrowserEngine::Chrome)
            )
            .is_err()
    );
    let two = service
        .create_browser_v2(
            "00000000-0000-4000-8000-000000000001",
            "00000000-0000-4000-8000-000000000005",
            managed(BrowserEngine::Firefox),
        )
        .unwrap();
    let three = service
        .create_browser_v2(
            "00000000-0000-4000-8000-000000000002",
            "00000000-0000-4000-8000-000000000006",
            managed(BrowserEngine::Chrome),
        )
        .unwrap();
    // A second process cannot initialize or mutate this live toolbox root.
    assert!(matches!(
        Toolbox::open(Config {
            root: root.clone(),
            programs: service.config.programs.clone(),
            environment: BTreeMap::new(),
            github_token: None,
        }),
        Err(Error::RootInUse)
    ));
    let profile = root.join("browser-profiles/00000000-0000-4000-8000-000000000001/chrome/profile");
    private_dir(&profile).unwrap();
    fs::write(profile.join("cookie-fixture"), b"retained").unwrap();
    assert!(
        service
            .delete_browser_profile(
                "00000000-0000-4000-8000-000000000001",
                BrowserEngine::Chrome
            )
            .is_err()
    );
    assert!(
        service
            .close_browser_session("00000000-0000-4000-8000-000000000008", &one.id)
            .is_err()
    );
    service
        .close_browser_session("00000000-0000-4000-8000-000000000004", &one.id)
        .unwrap();
    assert!(profile.join("cookie-fixture").is_file());
    let reopened = service
        .create_browser_v2(
            "00000000-0000-4000-8000-000000000001",
            "00000000-0000-4000-8000-000000000004",
            managed(BrowserEngine::Chrome),
        )
        .unwrap();
    assert!(profile.join("cookie-fixture").is_file());
    service
        .close_browser_session("00000000-0000-4000-8000-000000000004", &reopened.id)
        .unwrap();
    service
        .delete_browser_profile(
            "00000000-0000-4000-8000-000000000001",
            BrowserEngine::Chrome,
        )
        .unwrap();
    assert!(!profile.exists());
    service
        .close_browser_session("00000000-0000-4000-8000-000000000005", &two.id)
        .unwrap();
    service
        .close_browser_session("00000000-0000-4000-8000-000000000006", &three.id)
        .unwrap();
    service.shutdown();
    drop(service);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn manual_takeover_blocks_model_dispatch_before_launch_and_preserves_selected_session() {
    let (root, service) = fixture();
    let session = service
        .create_browser_v2(
            "00000000-0000-4000-8000-000000000003",
            "00000000-0000-4000-8000-000000000009",
            managed(BrowserEngine::Chrome),
        )
        .unwrap();
    assert_eq!(
        service.selected_browser_session("00000000-0000-4000-8000-000000000009"),
        Some(session.id.clone())
    );
    assert!(
        service
            .browser_command_v2(
                "00000000-0000-4000-8000-000000000008",
                &session.id,
                BrowserCommand {
                    operation: "tabs".into(),
                    ..Default::default()
                },
                true,
                &AtomicBool::new(false)
            )
            .is_err()
    );
    service
        .browser_takeover("00000000-0000-4000-8000-000000000009", &session.id, true)
        .unwrap();
    assert!(
        service
            .browser_sessions_v2("00000000-0000-4000-8000-000000000009")
            .unwrap()[0]
            .manual
    );
    let error = service
        .browser_approved_command(
            "00000000-0000-4000-8000-000000000009",
            &Action::Browser {
                session_id: session.id.clone(),
                operation: "navigate".into(),
                url: Some("https://example.com".into()),
                selector: None,
                text: None,
            },
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert!(error.to_string().contains("browser_manual_takeover"));
    assert!(service.managed.lock().unwrap().is_empty());
    service
        .close_browser_session("00000000-0000-4000-8000-000000000009", &session.id)
        .unwrap();
    assert!(
        service
            .selected_browser_session("00000000-0000-4000-8000-000000000009")
            .is_none()
    );
    service.shutdown();
    drop(service);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn input_validation_rejects_privileged_urls_credentials_and_malformed_coordinates() {
    for url in [
        "file:///etc/passwd",
        "chrome://settings",
        "https://user:pass@example.com",
        "https://example.com/\n",
    ] {
        assert!(
            validate_browser_command(&BrowserCommand {
                operation: "navigate".into(),
                url: Some(url.into()),
                ..Default::default()
            })
            .is_err()
        );
    }
    for value in [
        json!({"operation":"pointer"}),
        json!({"operation":"resize","width":100000,"height":900}),
        json!({"operation":"eval","text":"alert(1)"}),
        json!({"operation":"click","selector":""}),
    ] {
        let command: BrowserCommand = serde_json::from_value(value).unwrap();
        assert!(validate_browser_command(&command).is_err());
    }
}
#[test]
fn legacy_browser_session_creation_remains_independent() {
    let (root, service) = fixture();
    let s = service
        .create_browser_session("00000000-0000-4000-8000-000000000010")
        .unwrap();
    assert!(!service.has_browser_v2(&s.id));
    service
        .close_browser_session("00000000-0000-4000-8000-000000000010", &s.id)
        .unwrap();
    service.shutdown();
    drop(service);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn manual_intents_never_retain_typed_text_or_redispatch_an_uncertain_input() {
    let (root, service) = fixture();
    let project = Uuid::new_v4().to_string();
    let order = Uuid::new_v4().to_string();
    let session = service
        .create_browser_v2(&project, &order, managed(BrowserEngine::Chrome))
        .unwrap();
    let input = BrowserInput {
        idempotency_key: Uuid::new_v4().to_string(),
        epoch: session.epoch.clone(),
        command: BrowserCommand {
            operation: "fill".into(),
            selector: Some("#field".into()),
            text: Some("never-journal-a-typed-password".into()),
            ..Default::default()
        },
    };
    assert!(
        service
            .browser_manual_input(&order, &session.id, input.clone())
            .is_err()
    );
    let intent = root
        .join("sessions")
        .join(&session.id)
        .join("manual-inputs")
        .join(format!(
            "{}.intent.json",
            hash(input.idempotency_key.as_bytes())
        ));
    let raw = fs::read_to_string(intent).unwrap();
    assert_eq!(raw.len(), 64);
    assert!(!raw.contains("typed-password"));
    assert!(
        service
            .browser_manual_input(&order, &session.id, input)
            .unwrap_err()
            .to_string()
            .contains("do_not_replay")
    );
    assert!(service.managed.lock().unwrap().is_empty());
    service.close_browser_session(&order, &session.id).unwrap();
    service.shutdown();
    drop(service);
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires COWORK_BROWSER_TEST_PYTHON, COWORK_BROWSER_TEST_EXECUTABLE and COWORK_BROWSER_TEST_ENGINE"]
fn actual_browser_shares_manual_and_approved_actions_and_retains_profile() {
    use std::net::TcpListener;
    let python = PathBuf::from(std::env::var("COWORK_BROWSER_TEST_PYTHON").unwrap());
    let executable = PathBuf::from(std::env::var("COWORK_BROWSER_TEST_EXECUTABLE").unwrap());
    let engine: BrowserEngine =
        serde_json::from_value(json!(std::env::var("COWORK_BROWSER_TEST_ENGINE").unwrap()))
            .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let server = std::thread::spawn(move || {
        while !done.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                    let mut buffer = [0; 8192];
                    let _ = stream.read(&mut buffer);
                    let body = "<title>Rust browser fixture</title><h1 id='result'>Ready</h1><input id='entry'><button id='save' onclick=\"document.querySelector('#result').textContent=document.querySelector('#entry').value;document.cookie='retained=yes;path=/'\">Save</button><script>document.body.dataset.cookie=document.cookie</script>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    });
    let root = std::env::temp_dir().join(format!("cowork-browser-actual-{}", Uuid::new_v4()));
    let service = Toolbox::open(Config {
        root: root.clone(),
        programs: BTreeMap::from([
            ("python".into(), python),
            (browser_v2::engine_name(engine).into(), executable),
        ]),
        environment: std::env::vars()
            .filter(|(k, _)| {
                [
                    "PATH",
                    "SYSTEMROOT",
                    "WINDIR",
                    "TEMP",
                    "TMP",
                    "HOME",
                    "USERPROFILE",
                    "LOCALAPPDATA",
                ]
                .contains(&k.to_ascii_uppercase().as_str())
            })
            .collect(),
        github_token: None,
    })
    .unwrap();
    let project = Uuid::new_v4().to_string();
    let order = Uuid::new_v4().to_string();
    let exercise = || -> Result<()> {
        let session = service.create_browser_v2(&project, &order, managed(engine))?;
        service.browser_takeover(&order, &session.id, true)?;
        for command in [
            BrowserCommand {
                operation: "navigate".into(),
                url: Some(url.clone()),
                ..Default::default()
            },
            BrowserCommand {
                operation: "fill".into(),
                selector: Some("#entry".into()),
                text: Some("Shared with AI".into()),
                ..Default::default()
            },
        ] {
            service.browser_manual_input(
                &order,
                &session.id,
                BrowserInput {
                    idempotency_key: Uuid::new_v4().to_string(),
                    epoch: session.epoch.clone(),
                    command,
                },
            )?;
        }
        service.browser_takeover(&order, &session.id, false)?;
        let model = std::env::var("COWORK_BROWSER_TEST_MODEL_ACTIONS")
            .ok()
            .map(|path| {
                fs::read(path).map_err(Error::from).and_then(|raw| {
                    serde_json::from_slice::<serde_json::Value>(&raw).map_err(Error::from)
                })
            })
            .transpose()?;
        let actions: Vec<BrowserCommand> = if let Some(model) = &model {
            serde_json::from_value(model["actions"].clone())?
        } else {
            vec![BrowserCommand {
                operation: "click".into(),
                selector: Some("#save".into()),
                ..Default::default()
            }]
        };
        let mut last = serde_json::Value::Null;
        for command in actions {
            let mut request = Request {
                idempotency_key: Uuid::new_v4().to_string(),
                action: Action::Browser {
                    session_id: session.id.clone(),
                    operation: command.operation,
                    url: command.url,
                    selector: command.selector,
                    text: command.text,
                },
                timeout_seconds: 45,
                output_bytes: 262144,
                approval_sha256: None,
            };
            request.approval_sha256 = Some(service.preview(&order, &request)?.request_sha256);
            let job = service.submit(&order, request.clone())?;
            let deadline = std::time::Instant::now() + Duration::from_secs(60);
            loop {
                let job = service.status(&order, &job.id)?;
                if job.state != "running" {
                    assert_eq!(job.state, "completed", "{:?}", job.error);
                    last = serde_json::from_slice(&job.outcome.unwrap().stdout.bytes)?;
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(50));
            }
            assert_eq!(service.submit(&order, request)?.id, job.id);
        }
        let expected = model
            .as_ref()
            .and_then(|v| v["expected"].as_str())
            .unwrap_or("Shared with AI");
        assert!(last["dom"].as_str().unwrap().contains(expected));
        service.close_browser_session(&order, &session.id)?;
        let reopened = service.create_browser_v2(&project, &order, managed(engine))?;
        let result = service.browser_approved_command(
            &order,
            &Action::Browser {
                session_id: reopened.id.clone(),
                operation: "navigate".into(),
                url: Some(url.clone()),
                selector: None,
                text: None,
            },
            &AtomicBool::new(false),
        )?;
        assert!(result["dom"].as_str().unwrap().contains("retained=yes"));
        service.close_browser_session(&order, &reopened.id)?;
        Ok(())
    };
    let result = exercise();
    service.shutdown();
    drop(service);
    stop.store(true, Ordering::Release);
    server.join().unwrap();
    if let Err(error) = &result {
        eprintln!("Actual browser failure: {error}");
    }
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    for attempt in 0..20 {
        match fs::remove_dir_all(&root) {
            Ok(()) => break,
            Err(error) => {
                if attempt == 19 {
                    panic!("Fixture cleanup failed: {error}");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    result.unwrap();
}
