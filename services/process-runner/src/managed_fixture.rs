//! Actual private TLS service -> common Broker -> gVisor -> signed result proof.
//! Fixture keys/qualification metadata are synthetic and release no profile.
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_contracts::*;
use cowork_domain::{canonical_hash, environments::*, hash};
use cowork_managed_runtime::{containerd::bootstrap::*, volumes::PoolLimits};
use cowork_runner_protocol::*;
use std::{
    collections::BTreeMap,
    fs::File,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

mod cancel_gate;
mod delivery_gate;
mod missing_once;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ControllerPrepare {
    format_version: u32,
    control_public_key: Vec<u8>,
    control_key_file: PathBuf,
    environment_id: String,
    version_id: String,
    definition_sha256: String,
    target: ExecutionTargetRef,
}
async fn controller_phase(
    root: &Path,
    phase: &str,
    handoff: Option<&Path>,
    mut gate: Option<&mut cancel_gate::Gate>,
    mut delivery: Option<&mut delivery_gate::Gate>,
) {
    let binary = path("COWORK_TEST_CONTROLLER_BINARY");
    assert_eq!(
        hash(&std::fs::read(&binary).unwrap()),
        std::env::var("COWORK_TEST_CONTROLLER_SHA256").unwrap()
    );
    let mut command = Command::new(binary);
    command
        .arg(phase)
        .arg("--fixture-root")
        .arg(root)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(File::create(root.join(format!("controller-{phase}.stdout.log"))).unwrap())
        .stderr(File::create(root.join(format!("controller-{phase}.stderr.log"))).unwrap());
    if let Some(handoff) = handoff {
        command.arg("--service-handoff").arg(handoff);
        command.env(
            "COWORK_PROCESS_RUNNER_CONFIG",
            root.join("daemon-runner.json"),
        );
    }
    let mut child = OwnService(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Some(gate) = gate.as_deref_mut() {
            gate.poll();
            gate.poll_completion().await;
        }
        if let Some(delivery) = delivery.as_deref_mut() {
            delivery.poll();
        }
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(
                status.success(),
                "Controller {phase} failed; private bounded logs retained"
            );
            if let Some(gate) = gate.as_deref() {
                assert!(
                    gate.observed(),
                    "Cancel must follow original native guest start"
                );
                assert!(
                    gate.completed(),
                    "Cancel reconciliation must follow the original physical outcome"
                );
            }
            if let Some(delivery) = delivery.as_deref() {
                assert!(
                    delivery.completed(),
                    "Both original delivery/collection gates must complete"
                );
            }
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Bounded controller {phase} deadline"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn path(key: &str) -> PathBuf {
    std::env::var_os(key)
        .expect("Missing bounded fixture input")
        .into()
}
fn wall() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap()
}
fn private(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn directory(path: &Path) {
    std::fs::create_dir(path).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}
fn pinned(path: PathBuf) -> PinnedRuntimeFile {
    PinnedRuntimeFile {
        sha256: hash(&std::fs::read(&path).unwrap()),
        path,
    }
}
struct OwnService(Child);
impl Drop for OwnService {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn json<T: serde::de::DeserializeOwned>(response: reqwest::Response, expected: &[u16]) -> T {
    let status = response.status().as_u16();
    let bytes = response.bytes().await.unwrap();
    assert!(bytes.len() <= MANAGED_RESPONSE_BYTES);
    assert!(
        expected.contains(&status),
        "Unexpected service status {status}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}
async fn post<T: serde::Serialize>(
    client: &reqwest::Client,
    origin: &str,
    token: &str,
    route: &str,
    body: &T,
) -> reqwest::Response {
    client
        .post(format!("{origin}{route}"))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .unwrap()
}

fn recovery(case: &Path, receipt: &Signed<Receipt>) -> Option<serde_json::Value> {
    for volume in std::fs::read_dir(case.join("volumes")).unwrap() {
        let root = volume.unwrap().path().join("root");
        for entry in std::fs::read_dir(root).unwrap() {
            let entry = entry.unwrap();
            if !entry.file_name().to_str().unwrap().starts_with("cw-") {
                continue;
            }
            let slot = entry.path().join("result-snapshot.bin");
            if !slot.exists() {
                continue;
            }
            assert_eq!(slot.metadata().unwrap().len(), 8 * 1024 * 1024);
            let bytes = std::fs::read(slot).unwrap();
            let size = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
            if size == 0 {
                continue;
            }
            assert!(size <= bytes.len() - 36);
            let expected = bytes[4..36]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            assert_eq!(hash(&bytes[36..36 + size]), expected);
            assert!(bytes[36 + size..].iter().all(|b| *b == 0));
            let value: serde_json::Value = serde_json::from_slice(&bytes[36..36 + size]).unwrap();
            if value["execution_receipt_sha256"] == canonical_hash(receipt) {
                return Some(value);
            }
        }
    }
    None
}

async fn start_without_reading_receipt(
    address: std::net::SocketAddr,
    ca: &Path,
    token: &str,
    route: &str,
    operation: &ManagedInputOperation,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let pem = std::fs::read_to_string(ca).unwrap();
    let body = pem
        .strip_prefix("-----BEGIN CERTIFICATE-----")
        .unwrap()
        .split_once("-----END CERTIFICATE-----")
        .unwrap()
        .0;
    let der = STANDARD
        .decode(
            body.chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect::<String>(),
        )
        .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(der))
        .unwrap();
    let tls = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(tls));
    let socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let mut socket = connector
        .connect(
            rustls::pki_types::ServerName::try_from("localhost").unwrap(),
            socket,
        )
        .await
        .unwrap();
    let body = serde_json::to_vec(operation).unwrap();
    let headers = format!(
        "POST {route} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(headers.as_bytes()).await.unwrap();
    socket.write_all(&body).await.unwrap();
    socket.flush().await.unwrap();
    // A completed client write cannot prove server acceptance: an immediate
    // close can reset TLS before HTTP dispatch. Observe only the status/headers,
    // then lose the entire original signed receipt body as a real broken reply.
    let mut headers = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(12);
    while !headers.ends_with(b"\r\n\r\n") {
        assert!(
            headers.len() < 8192,
            "Bounded accepted-start response headers"
        );
        let mut byte = [0];
        tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            socket.read_exact(&mut byte),
        )
        .await
        .unwrap()
        .unwrap();
        headers.push(byte[0]);
    }
    assert!(
        headers.starts_with(b"HTTP/1.1 202 ") || headers.starts_with(b"HTTP/1.1 200 "),
        "Start must be accepted before intentionally losing its receipt body"
    );
    drop(socket);
}

fn guest_has_started(case: &Path) -> bool {
    std::fs::read_dir(case.join("volumes"))
        .unwrap()
        .any(|entry| {
            let marker = entry.unwrap().path().join("root/work/workspace/started");
            marker.exists() && std::fs::read(marker).unwrap() == b"started"
        })
}

struct ServiceFixture {
    base: PathBuf,
    case: PathBuf,
    state: PathBuf,
    controller_root: PathBuf,
    external_controller: bool,
    controller_case: String,
    controller_stop_read: bool,
    controller_cancel: bool,
    controller_delivery: bool,
    runner: SigningKey,
    control: SigningKey,
    binding: Binding,
    identity: ExecutionRunnerIdentity,
    target: ExecutionTargetRef,
    reference: ExecutionEnvironmentRef,
    installed: InstalledManagedProfile,
    token_path: PathBuf,
    ca: PathBuf,
    address: std::net::SocketAddr,
    origin: String,
    token: String,
    service: OwnService,
    client: reqwest::Client,
}

async fn service_fixture(allow_controller: bool) -> ServiceFixture {
    if !allow_controller {
        for key in [
            "COWORK_TEST_CONTROLLER_BINARY",
            "COWORK_TEST_CONTROLLER_SHA256",
            "COWORK_TEST_CONTROLLER_CASE",
        ] {
            assert!(
                std::env::var_os(key).is_none(),
                "Missing-once excludes Controller cases before setup"
            );
        }
    }
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let base = path("COWORK_RUNTIME_VOLUME_PROOF_DIR");
    assert_eq!(base.file_name().unwrap(), "case-storage");
    let case = base.join("actual-tasks-service");
    directory(&case);
    let state = base.join("service-state");
    directory(&state);
    let config_root = base.join("service-config");
    directory(&config_root);
    let runner_bytes = SigningKey::generate_pkcs8().unwrap();
    let runner = SigningKey::from_pkcs8(&runner_bytes).unwrap();
    let controller_root = base.join("controller");
    let external_controller = std::env::var_os("COWORK_TEST_CONTROLLER_BINARY").is_some();
    let controller_case =
        std::env::var("COWORK_TEST_CONTROLLER_CASE").unwrap_or_else(|_| "copy".into());
    assert!(matches!(
        controller_case.as_str(),
        "copy" | "cancel" | "stop_read" | "delivery"
    ));
    let controller_stop_read = controller_case == "stop_read";
    let controller_cancel = matches!(controller_case.as_str(), "cancel" | "stop_read");
    let controller_delivery = controller_case == "delivery";
    assert!(!controller_cancel || external_controller);
    let definition = EnvironmentDefinition {
        format_version: 3,
        network: Some(EnvironmentNetwork::Denied),
        artifact: EnvironmentArtifact::ManagedLinuxPackage {
            package_sha256: "b".repeat(64),
            rootfs_manifest_sha256: hash(
                &std::fs::read(path("COWORK_TEST_ROOTFS_MANIFEST")).unwrap(),
            ),
            gvisor_manifest_sha256: hash(
                &std::fs::read(path("COWORK_TEST_GVISOR_MANIFEST")).unwrap(),
            ),
            maximum_files: 16384,
            output_bytes: 262144,
        },
        programs: BTreeMap::from([("probe".into(), "bin/probe".into())]),
        packages: vec![],
        limits: EnvironmentLimits {
            cpu_millis: 1000,
            memory_bytes: 512 * 1024 * 1024,
            processes: 128,
            scratch_bytes: 32 * 1024 * 1024,
        },
    };
    let (control, prepared) = if external_controller {
        directory(&controller_root);
        private(
            &controller_root.join("controller-definition.json"),
            &serde_json::to_vec(&serde_json::json!({"format_version":1,"definition":definition,"runner":{"runner_id":"actual-managed-https-fixture","generation":1,"adapter_build_sha256":std::env::var("COWORK_TEST_SERVICE_SHA256").unwrap(),"runner_public_key":runner.public_key()}}))
                .unwrap(),
        );
        controller_phase(&controller_root, "prepare", None, None, None).await;
        let prepared: ControllerPrepare = serde_json::from_slice(
            &read_operator_file(
                &controller_root.join("controller-prepare.json"),
                65536,
                true,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(prepared.format_version, 1);
        assert!(prepared.control_key_file.starts_with(&controller_root));
        let key = SigningKey::from_pkcs8(
            &read_operator_file(&prepared.control_key_file, 16384, true).unwrap(),
        )
        .unwrap();
        assert_eq!(key.public_key(), prepared.control_public_key);
        assert_eq!(prepared.definition_sha256, canonical_hash(&definition));
        assert!(!prepared.environment_id.is_empty() && !prepared.version_id.is_empty());
        (key, Some(prepared))
    } else {
        (
            SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap(),
            None,
        )
    };
    let binding = Binding {
        runner_id: "actual-managed-https-fixture".into(),
        generation: 1,
        adapter_build_sha256: std::env::var("COWORK_TEST_SERVICE_SHA256").unwrap(),
        control_public_key: control.public_key(),
        runner_public_key: runner.public_key(),
    };
    let identity = ExecutionRunnerIdentity {
        runner_id: binding.runner_id.clone(),
        generation: binding.generation,
        adapter_build_sha256: binding.adapter_build_sha256.clone(),
        control_public_key: binding.control_public_key.clone(),
        runner_public_key: binding.runner_public_key.clone(),
    };
    let target = prepared
        .as_ref()
        .map(|p| p.target.clone())
        .unwrap_or_else(|| ExecutionTargetRef {
            target_id: cowork_domain::execution_targets::execution_target_id(&identity).unwrap(),
            revision: 1,
            identity_sha256: cowork_domain::execution_targets::runner_identity_hash(&identity)
                .unwrap(),
        });
    let q = EnvironmentQualification {
        runner_id: binding.runner_id.clone(),
        runner_generation: 1,
        adapter_build_sha256: binding.adapter_build_sha256.clone(),
        artifact_sha256: definition.artifact.sha256().into(),
        definition_sha256: canonical_hash(&definition),
        report_sha256: "c".repeat(64),
        capabilities: definition.required_capabilities(),
        verified_at: wall(),
    };
    let reference = ExecutionEnvironmentRef {
        environment_id: prepared.as_ref().map_or_else(
            || "private-service-fixture".into(),
            |p| p.environment_id.clone(),
        ),
        version_id: prepared
            .as_ref()
            .map_or_else(|| "actual-tools".into(), |p| p.version_id.clone()),
        definition_sha256: canonical_hash(&definition),
        qualification_sha256: canonical_hash(&q),
    };
    let installed = InstalledManagedProfile {
        format_version: 1,
        binding: binding.clone(),
        target: target.clone(),
        reference: reference.clone(),
        definition,
        qualification: runner.sign("environment_qualification", q).unwrap(),
    };
    let installed_path = config_root.join("installed.json");
    private(&installed_path, &serde_json::to_vec(&installed).unwrap());
    let bootstrap = ManagedBootstrap {
        format_version: 1,
        installed_profile: pinned(installed_path),
        gvisor_directory: path("COWORK_TEST_GVISOR_DIRECTORY"),
        gvisor_manifest: pinned(path("COWORK_TEST_GVISOR_MANIFEST")),
        rootfs_directory: path("COWORK_TEST_ROOTFS_DIRECTORY"),
        rootfs_manifest: pinned(path("COWORK_TEST_ROOTFS_MANIFEST")),
        containerd: pinned(path("COWORK_TEST_CONTAINERD_BINARY")),
        fence: pinned(path("COWORK_TEST_FENCE_BINARY")),
        runsc_driver: pinned(path("COWORK_TEST_DRIVER_BINARY")),
        formatter: pinned(Path::new("/usr/sbin/mke2fs").canonicalize().unwrap()),
        volume_pool_directory: case.clone(),
        volume_pool_limits: PoolLimits {
            reserved_bytes: 432 * 1024 * 1024,
            max_volumes: 18,
        },
        data_filesystem_max_bytes: 1024 * 1024 * 1024,
        delegated_cgroup_directory: path("COWORK_RUNTIME_CGROUP_PARENT"),
    };
    let cert = config_root.join("tls-cert.pem");
    let key = config_root.join("tls-key.pem");
    let ca = config_root.join("tls-ca.pem");
    let ca_key = config_root.join("tls-ca.key");
    let csr = config_root.join("tls.csr");
    let generated = Command::new("/usr/bin/openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-sha256",
            "-nodes",
            "-days",
            "1",
            "-subj",
            "/CN=Private Runtime Fixture CA",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
            "-keyout",
        ])
        .arg(&ca_key)
        .arg("-out")
        .arg(&ca)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "Private TLS CA generation failed"
    );
    let generated = Command::new("/usr/bin/openssl")
        .args([
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-sha256",
            "-nodes",
            "-subj",
            "/CN=localhost",
            "-keyout",
        ])
        .arg(&key)
        .arg("-out")
        .arg(&csr)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "Private TLS leaf generation failed"
    );
    let extensions = config_root.join("tls-extensions");
    private(&extensions,b"basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n");
    let generated = Command::new("/usr/bin/openssl")
        .args([
            "x509",
            "-req",
            "-sha256",
            "-days",
            "1",
            "-set_serial",
            "1",
            "-in",
        ])
        .arg(&csr)
        .arg("-CA")
        .arg(&ca)
        .arg("-CAkey")
        .arg(&ca_key)
        .arg("-extfile")
        .arg(&extensions)
        .arg("-out")
        .arg(&cert)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "Private TLS leaf signing failed"
    );
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::set_permissions(&ca, std::fs::Permissions::from_mode(0o600)).unwrap();
    let runner_path = config_root.join("runner.pk8");
    private(&runner_path, &runner_bytes);
    let token = uuid::Uuid::new_v4().to_string() + &uuid::Uuid::new_v4().to_string();
    let token_path = config_root.join("token");
    private(&token_path, token.as_bytes());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let origin = format!("https://127.0.0.1:{}", address.port());
    let config = config_root.join("configuration.json");
    private(&config,&serde_json::to_vec(&serde_json::json!({"format_version":2,"listen":address,
        "bootstrap":bootstrap,"state_directory":state,"runner_key_file":runner_path,"token_file":token_path,
        "tls_certificate_file":cert,"tls_key_file":key,"result_retention_seconds":if external_controller{600}else{8},
        "delivery_journal":{"mode":"initialize","slots":8}})).unwrap());
    let stderr = base.join("service.stderr.log");
    let stdout = base.join("service.stdout.log");
    let mut service = OwnService(
        Command::new(path("COWORK_TEST_SERVICE_BINARY"))
            .arg("--config")
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(File::create(stdout).unwrap())
            .stderr(File::create(&stderr).unwrap())
            .spawn()
            .unwrap(),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .https_only(true)
        .add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(&ca).unwrap()).unwrap())
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            service.0.try_wait().unwrap().is_none(),
            "Service exited: {}",
            std::fs::read_to_string(&stderr).unwrap()
        );
        match client
            .post(format!("{origin}/v2/managed/ready"))
            .body("invalid body")
            .send()
            .await
        {
            Ok(response) => {
                assert_eq!(response.status(), 401);
                break;
            }
            Err(error) => assert!(
                Instant::now() < until,
                "TLS service readiness deadline: {error:?}"
            ),
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    ServiceFixture {
        base,
        case,
        state,
        controller_root,
        external_controller,
        controller_case,
        controller_stop_read,
        controller_cancel,
        controller_delivery,
        runner,
        control,
        binding,
        identity,
        target,
        reference,
        installed,
        token_path,
        ca,
        address,
        origin,
        token,
        service,
        client,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Explicit private native HTTPS Begin/Missing observation only; no guest start"]
async fn actual_https_service_begin_and_missing_once_without_start() {
    missing_once::run(service_fixture(false).await)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Explicit private bounded native/TLS fixture only; launches actual gVisor workloads"]
async fn actual_https_service_transfer_result_and_rejected_export() {
    let ServiceFixture {
        base,
        case,
        state,
        controller_root,
        external_controller,
        controller_case,
        controller_stop_read,
        controller_cancel,
        controller_delivery,
        runner,
        control,
        binding,
        identity,
        target,
        reference,
        installed,
        token_path,
        ca,
        address,
        origin,
        token,
        mut service,
        client,
    } = service_fixture(true).await;
    assert_eq!(
        client
            .post(format!("{origin}/v2/launches"))
            .bearer_auth(&token)
            .body("invalid legacy launch")
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    if external_controller {
        let prepared: ControllerPrepare = serde_json::from_slice(
            &read_operator_file(
                &controller_root.join("controller-prepare.json"),
                65536,
                true,
            )
            .unwrap(),
        )
        .unwrap();
        let daemon_config = controller_root.join("daemon-runner.json");
        private(&daemon_config,&serde_json::to_vec(&serde_json::json!({"format":1,"endpoint":format!("{origin}/"),"binding":binding,
            "control_key_file":prepared.control_key_file,"transport_token_file":token_path,"ca_certificate_file":ca,"client_identity_file":null})).unwrap());
        let handoff = controller_root.join("managed-service-handoff.json");
        let handoff_value = serde_json::json!({"format_version":1,"controller_configuration_file":daemon_config,
            "binding":binding,"target":target,"environment":reference,"definition":installed.definition,"qualification":installed.qualification.payload,
            "host_sentinel":path("COWORK_TEST_HOST_SENTINEL"),"expected_input_utf8":"approved HTTPS controller snapshot bytes\n","program":"probe","mode":if controller_stop_read {"owner_stop_read"} else if controller_cancel {"owner_block"} else {"owner_copy"},"input_file_name":"value.txt","output_file_name":"result.txt"});
        let mut handoff_value = handoff_value;
        if controller_delivery {
            handoff_value["delivery"] = true.into();
        }
        private(&handoff, &serde_json::to_vec(&handoff_value).unwrap());
        let mut gate = cancel_gate::Gate::new(
            &case,
            &controller_root,
            client.clone(),
            origin.clone(),
            token.clone(),
            binding.clone(),
            controller_stop_read,
        );
        let mut delivery = delivery_gate::Gate::new(&case, &controller_root, binding.clone());
        controller_phase(
            &controller_root,
            "exercise",
            Some(&handoff),
            controller_cancel.then_some(&mut gate),
            controller_delivery.then_some(&mut delivery),
        )
        .await;
        let result: serde_json::Value = serde_json::from_slice(
            &read_operator_file(
                &controller_root.join("controller-result.json"),
                1024 * 1024,
                true,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(result["format_version"], 1);
        assert_eq!(result["status"], "passed");
        if controller_delivery {
            assert_eq!(result["delivery"], true);
            assert!(delivery.completed());
            private(&base.join("managed-service-observation.json"), &serde_json::to_vec(&serde_json::json!({
                "controller":result, "controller_case":"delivery", "native_delivery":delivery.observations(),
                "profile_released":false,"app_ui_acceptance":false
            })).unwrap());
            println!(
                "managed_https_controller=verified; actual_controller_child=true; production_transport=true; original_receipt_and_manifest_bound=true; profile_released=false"
            );
            assert_eq!(
                unsafe { libc::kill(service.0.id() as i32, libc::SIGINT) },
                0
            );
            assert!(service.0.wait().unwrap().success());
            return;
        }
        let attempt = result["process_attempt_id"].as_str().unwrap();
        if controller_stop_read {
            assert_eq!(result["scenario"], "stop_read");
            assert_eq!(result["business_state"], "interrupted");
            assert_eq!(result["process_state"]["state"], "reconciled_completed");
            assert!(!result["result_version_id"].as_str().unwrap().is_empty());
            assert!(!result["manifest_sha256"].as_str().unwrap().is_empty());
        } else if controller_cancel {
            assert_eq!(result["scenario"], "cancel");
            assert!(result["result_version_id"].is_null());
            assert!(result["manifest_sha256"].is_null());
            assert!(matches!(
                result["business_state"].as_str(),
                Some("cancel_requested" | "canceled" | "interrupted")
            ));
        } else {
            assert!(!result["result_version_id"].as_str().unwrap().is_empty());
        }
        let receipt: Signed<Receipt> = json(
            client
                .get(format!("{origin}/v2/receipts/{attempt}"))
                .bearer_auth(&token)
                .send()
                .await
                .unwrap(),
            &[200],
        )
        .await;
        receipt
            .verify("receipt", &binding.runner_public_key)
            .unwrap();
        assert_eq!(result["run_id"], receipt.payload.request.run_id);
        assert_eq!(result["execution_receipt_sha256"], canonical_hash(&receipt));
        if controller_cancel {
            assert!(
                matches!(&receipt.payload.state,ExecutionState::Completed{result} if result.termination==ProcessTermination::Canceled&&result.output_complete)
            );
        } else {
            assert!(
                matches!(&receipt.payload.state,ExecutionState::Completed{result} if result.termination==ProcessTermination::Exited&&result.exit_code==Some(0)&&result.output_complete)
            );
        }
        let deadline = Instant::now() + Duration::from_secs(4);
        let retained = loop {
            if let Some(retained) = recovery(&case, &receipt) {
                break retained;
            }
            assert!(
                Instant::now() < deadline,
                "Original native recovery publication missing"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(retained["output_state"], "ready");
        if !controller_cancel || controller_stop_read {
            assert_eq!(result["manifest_sha256"], retained["manifest"]["sha256"]);
        }
        let attestation: Signed<ManagedOutputAttestation> =
            serde_json::from_value(retained["attestation"].clone()).unwrap();
        attestation
            .verify("managed_output", &binding.runner_public_key)
            .unwrap();
        let native_start = if controller_cancel {
            Some(
                serde_json::from_slice::<serde_json::Value>(
                    &read_operator_file(
                        &controller_root.join("controller-native-start.json"),
                        65536,
                        true,
                    )
                    .unwrap(),
                )
                .unwrap(),
            )
        } else {
            None
        };
        private(&base.join("managed-service-observation.json"),&serde_json::to_vec(&serde_json::json!({"controller":result,"receipt":receipt,"native_recovery":retained,"native_start":native_start,"controller_case":controller_case,"profile_released":false,"app_ui_acceptance":false})).unwrap());
        println!(
            "managed_https_controller=verified; actual_controller_child=true; production_transport=true; original_receipt_and_manifest_bound=true; profile_released=false"
        );
        assert_eq!(
            unsafe { libc::kill(service.0.id() as i32, libc::SIGINT) },
            0
        );
        assert!(service.0.wait().unwrap().success());
        return;
    }
    let install = control
        .sign(
            "environment_install",
            EnvironmentInstall {
                environment_id: reference.environment_id.clone(),
                version_id: reference.version_id.clone(),
                definition: installed.definition.clone(),
            },
        )
        .unwrap();
    let qualified: Signed<EnvironmentQualification> = json(
        post(
            &client,
            &origin,
            &token,
            "/v2/environments/install",
            &install,
        )
        .await,
        &[200],
    )
    .await;
    assert_eq!(
        qualified, installed.qualification,
        "Install route must return the original operator qualification, never mint another"
    );
    let mut unknown = install.payload.clone();
    unknown.version_id = "not-installed".into();
    assert_eq!(
        post(
            &client,
            &origin,
            &token,
            "/v2/environments/install",
            &control.sign("environment_install", unknown).unwrap()
        )
        .await
        .status(),
        409
    );
    let untrusted = runner
        .sign("environment_install", install.payload.clone())
        .unwrap();
    assert_eq!(
        post(
            &client,
            &origin,
            &token,
            "/v2/environments/install",
            &untrusted
        )
        .await
        .status(),
        403
    );
    let observed: Signed<EnvironmentObservation> = json(
        post(
            &client,
            &origin,
            &token,
            "/v2/environments/observe",
            &reference,
        )
        .await,
        &[200],
    )
    .await;
    observed
        .verify("environment_observation", &binding.runner_public_key)
        .unwrap();
    assert_eq!(
        observed.payload.definition_sha256,
        reference.definition_sha256
    );
    assert_eq!(
        observed.payload.artifact_sha256,
        installed.qualification.payload.artifact_sha256
    );
    assert_eq!(
        observed.payload.capabilities,
        installed.qualification.payload.capabilities
    );
    let mut unknown = reference.clone();
    unknown.qualification_sha256 = "f".repeat(64);
    assert_eq!(
        post(
            &client,
            &origin,
            &token,
            "/v2/environments/observe",
            &unknown
        )
        .await
        .status(),
        409
    );
    println!(
        "managed_https_environment=verified; original_qualification=true; native_observation=true; unknown_package_denied=true; foreign_install_signature_denied=true"
    );
    let data = b"approved HTTPS snapshot bytes\n";
    let manifest = cowork_domain::workspace_transfer::build(
        vec![],
        vec![WorkspaceTransferFile {
            path: "input.txt".into(),
            size: data.len() as u64,
            sha256: hash(data),
            chunks: vec![WorkspaceTransferChunk {
                offset: 0,
                size: data.len() as u64,
                sha256: hash(data),
            }],
        }],
    )
    .unwrap();
    sqlx::any::install_default_drivers();
    let database = sqlx::any::AnyPoolOptions::new()
        .max_connections(1)
        .connect(&format!(
            "sqlite://{}?mode=ro",
            state.join("runner.db").display()
        ))
        .await
        .unwrap();
    let mut observations = Vec::new();
    for (completed_before, mode) in ["copy", "symlink_result", "block"].into_iter().enumerate() {
        let ready_request = ManagedReadyRequest {
            format_version: 1,
            request_id: uuid::Uuid::new_v4().to_string(),
            environment: reference.clone(),
            target: target.clone(),
        };
        let readiness: Signed<ManagedReadiness> = json(
            post(
                &client,
                &origin,
                &token,
                "/v2/managed/ready",
                &ready_request,
            )
            .await,
            &[200],
        )
        .await;
        readiness
            .verify("managed_readiness", &binding.runner_public_key)
            .unwrap();
        readiness
            .payload
            .validate(&ready_request, &binding, wall())
            .unwrap();
        let repeated: Signed<ManagedReadiness> = json(
            post(
                &client,
                &origin,
                &token,
                "/v2/managed/ready",
                &ready_request,
            )
            .await,
            &[200],
        )
        .await;
        assert_eq!(readiness, repeated);
        let issued = wall();
        let invocation = ProcessInvocation {
            program: "probe".into(),
            arguments: vec![
                mode.into(),
                path("COWORK_TEST_HOST_SENTINEL").to_str().unwrap().into(),
            ],
            timeout_seconds: 25,
        };
        let launch = control
            .sign(
                "launch",
                LaunchRequest {
                    format_version: 1,
                    runner_id: binding.runner_id.clone(),
                    runner_generation: 1,
                    adapter_build_sha256: binding.adapter_build_sha256.clone(),
                    process_attempt_id: format!("service-attempt-{mode}"),
                    run_id: format!("service-run-{mode}"),
                    tool_call_id: format!("service-tool-{mode}"),
                    request_sha256: hash(mode.as_bytes()),
                    worker_fence_sha256: "d".repeat(64),
                    lease_epoch: 1,
                    environment: reference.clone(),
                    workspace_resource_id: "not-input-authority".into(),
                    invocation: invocation.clone(),
                    issued_at: issued,
                    expires_at: issued + 30000,
                    execution_deadline: issued + 60000,
                },
            )
            .unwrap();
        let grant = control
            .sign(
                "managed_input_grant",
                ManagedInputGrant {
                    format_version: 1,
                    execution_plan_id: format!("fixture-confirmed-{mode}"),
                    execution_plan_sha256: "e".repeat(64),
                    target: target.clone(),
                    runner: identity.clone(),
                    input_manifest_sha256: manifest.sha256.clone(),
                    process_attempt_id: launch.payload.process_attempt_id.clone(),
                    run_id: launch.payload.run_id.clone(),
                    request_sha256: launch.payload.request_sha256.clone(),
                    environment: reference.clone(),
                    invocation_sha256: canonical_hash(&invocation),
                    worker_fence_sha256: launch.payload.worker_fence_sha256.clone(),
                    lease_epoch: 1,
                    issued_at: issued,
                    expires_at: issued + 60000,
                },
            )
            .unwrap();
        let begin = ManagedBeginRequest {
            format_version: 1,
            readiness,
            launch: launch.clone(),
            grant: grant.clone(),
            manifest: manifest.clone(),
        };
        let ManagedBeginResponse::Input { ticket } = json(
            post(&client, &origin, &token, "/v2/managed/inputs", &begin).await,
            &[201],
        )
        .await
        else {
            panic!()
        };
        ticket
            .verify("managed_input_ticket", &binding.runner_public_key)
            .unwrap();
        ticket.payload.validate(&begin, wall()).unwrap();
        let ManagedBeginResponse::Input { ticket: repeat } = json(
            post(&client, &origin, &token, "/v2/managed/inputs", &begin).await,
            &[200],
        )
        .await
        else {
            panic!()
        };
        assert_eq!(ticket, repeat);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runner_executions")
            .fetch_one(&database)
            .await
            .unwrap();
        assert_eq!(
            count, completed_before as i64,
            "Transfer must not enqueue/admit an execution before the verified snapshot"
        );
        let route = format!("/v2/managed/inputs/{}", ticket.payload.input_id);
        let operation = ManagedInputOperation {
            ticket: ticket.clone(),
        };
        let missing: ManagedMissingResponse = json(
            post(
                &client,
                &origin,
                &token,
                &format!("{route}/missing"),
                &operation,
            )
            .await,
            &[200],
        )
        .await;
        missing.validate(&ticket, &manifest).unwrap();
        assert_eq!(missing.missing.len(), 1);
        let part = missing.missing.into_iter().next().unwrap();
        assert_eq!(
            post(
                &client,
                &origin,
                &token,
                &format!("{route}/chunks"),
                &ManagedChunkRequest {
                    ticket: ticket.clone(),
                    chunk: part.clone(),
                    bytes_base64: STANDARD.encode(b"corrupt")
                }
            )
            .await
            .status(),
            409
        );
        let chunk = ManagedChunkRequest {
            ticket: ticket.clone(),
            chunk: part,
            bytes_base64: STANDARD.encode(data),
        };
        let first: ManagedChunkAck = json(
            post(&client, &origin, &token, &format!("{route}/chunks"), &chunk).await,
            &[200],
        )
        .await;
        let second: ManagedChunkAck = json(
            post(&client, &origin, &token, &format!("{route}/chunks"), &chunk).await,
            &[200],
        )
        .await;
        assert_eq!(first, second);
        if mode == "block" {
            start_without_reading_receipt(
                address,
                &ca,
                &token,
                &format!("{route}/start"),
                &operation,
            )
            .await;
            let deadline = Instant::now() + Duration::from_secs(20);
            while !guest_has_started(&case) {
                assert!(
                    Instant::now() < deadline,
                    "Disconnected start must actually reach the guest without replay"
                );
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            let fence = control
                .sign(
                    "fence",
                    FenceRequest {
                        runner_id: binding.runner_id.clone(),
                        runner_generation: binding.generation,
                        run_id: launch.payload.run_id.clone(),
                        lease_epoch: 2,
                        revoked: true,
                    },
                )
                .unwrap();
            assert_eq!(
                post(&client, &origin, &token, "/v2/fences", &fence)
                    .await
                    .status(),
                204
            );
        } else {
            let first: Signed<Receipt> = json(
                post(
                    &client,
                    &origin,
                    &token,
                    &format!("{route}/start"),
                    &operation,
                )
                .await,
                &[200, 202],
            )
            .await;
            first.verify("receipt", &binding.runner_public_key).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(35);
        let completed = loop {
            let response = client
                .get(format!(
                    "{origin}/v2/receipts/{}",
                    launch.payload.process_attempt_id
                ))
                .bearer_auth(&token)
                .send()
                .await
                .unwrap();
            let receipt: Signed<Receipt> = json(response, &[200, 202]).await;
            receipt
                .verify("receipt", &binding.runner_public_key)
                .unwrap();
            if let ExecutionState::Completed { result } = &receipt.payload.state {
                if mode == "block" {
                    assert_eq!(result.termination, ProcessTermination::Canceled);
                } else {
                    assert_eq!(result.exit_code, Some(0));
                }
                assert!(result.output_complete);
                break receipt;
            }
            assert!(
                Instant::now() < deadline,
                "Actual TLS/native completion deadline"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let replay: Signed<Receipt> = json(
            post(
                &client,
                &origin,
                &token,
                &format!("{route}/start"),
                &operation,
            )
            .await,
            &[200, 202],
        )
        .await;
        assert_eq!(replay, completed);
        let read_route = format!(
            "/v2/managed/results/{}/read",
            launch.payload.process_attempt_id
        );
        let read = |operation| {
            let issued_at = wall();
            control
                .sign(
                    "managed_result_read",
                    ManagedResultReadRequest {
                        format_version: 1,
                        runtime_instance_sha256: begin
                            .readiness
                            .payload
                            .runtime_instance_sha256
                            .clone(),
                        request_id: uuid::Uuid::new_v4().to_string(),
                        actor_id: "fixture-current-actor".into(),
                        authorization_sha256: "f".repeat(64),
                        execution_plan_id: grant.payload.execution_plan_id.clone(),
                        execution_plan_sha256: grant.payload.execution_plan_sha256.clone(),
                        process_attempt_id: launch.payload.process_attempt_id.clone(),
                        run_id: launch.payload.run_id.clone(),
                        launch_envelope_sha256: envelope_hash(&launch),
                        execution_receipt_sha256: canonical_hash(&completed),
                        runner: identity.clone(),
                        target: target.clone(),
                        operation,
                        issued_at,
                        expires_at: issued_at + 30000,
                        context: None,
                    },
                )
                .unwrap()
        };
        if mode == "block" {
            assert_eq!(
                post(
                    &client,
                    &origin,
                    &token,
                    &read_route,
                    &read(ManagedResultReadOperation::Manifest)
                )
                .await
                .status(),
                403,
                "Revoked execution fence must not be revived through an old result context"
            );
            observations.push(serde_json::json!({"mode":mode,"receipt":completed,"start_status_headers_read":true,"start_receipt_body_read":false,"actual_guest_started_before_stop":true}));
            println!(
                "managed_https_cancel=verified; start_receipt_body_lost=true; actual_guest_started=true; signed_stop_requested=true; original_physical_completion=true; revoked_read_context_denied=true; replay_without_dispatch=true"
            );
            continue;
        }
        let deadline = Instant::now() + Duration::from_secs(4);
        let output = loop {
            let response = post(
                &client,
                &origin,
                &token,
                &read_route,
                &read(ManagedResultReadOperation::Manifest),
            )
            .await;
            if response.status() == 200 {
                break Some(json::<ManagedResultManifest>(response, &[200]).await);
            }
            assert_eq!(response.status(), 409, "Unexpected output status");
            if mode == "symlink_result" || Instant::now() >= deadline {
                break None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        if mode == "copy" {
            let output = output.expect("Portable output must be published");
            let retained = recovery(&case, &completed)
                .expect("Original attestation must be durable before publication");
            assert_eq!(
                retained["attestation"],
                serde_json::to_value(&output.attestation).unwrap()
            );
            output
                .attestation
                .verify("managed_output", &binding.runner_public_key)
                .unwrap();
            output
                .attestation
                .payload
                .validate(
                    &grant.payload,
                    &launch,
                    &completed,
                    &output.manifest,
                    &begin.readiness.payload.runtime_instance_sha256,
                    wall(),
                )
                .unwrap();
            assert_eq!(
                output.attestation.payload.produced_at,
                completed.payload.updated_at
            );
            let file = output
                .manifest
                .files
                .iter()
                .find(|f| f.path == "result.txt")
                .unwrap();
            let operation = ManagedResultReadOperation::Chunk {
                attestation_sha256: canonical_hash(&output.attestation),
                manifest_sha256: output.manifest.sha256.clone(),
                chunk: ReadWorkspaceTransferChunk {
                    path: file.path.clone(),
                    offset: 0,
                    expected_sha256: file.chunks[0].sha256.clone(),
                },
            };
            let response = post(&client, &origin, &token, &read_route, &read(operation)).await;
            assert_eq!(response.status(), 200);
            assert_eq!(&response.bytes().await.unwrap()[..], data);
            tokio::time::sleep(Duration::from_millis(100)).await;
            let repeated: ManagedResultManifest = json(
                post(
                    &client,
                    &origin,
                    &token,
                    &read_route,
                    &read(ManagedResultReadOperation::Manifest),
                )
                .await,
                &[200],
            )
            .await;
            assert_eq!(repeated, output);
            assert_eq!(
                recovery(&case, &completed).unwrap(),
                retained,
                "Repeated reads must not rewrite original provenance"
            );
            let remaining = output.attestation.payload.retained_until - wall();
            if remaining > 0 {
                tokio::time::sleep(Duration::from_millis(remaining as u64 + 30)).await;
            }
            assert_eq!(
                post(
                    &client,
                    &origin,
                    &token,
                    &read_route,
                    &read(ManagedResultReadOperation::Manifest)
                )
                .await
                .status(),
                410
            );
            println!(
                "managed_https_copy=verified; original_attestation_stable=true; exact_chunk=true; retention_expired=true; start_replay_without_dispatch=true"
            );
            observations.push(serde_json::json!({"mode":mode,"receipt":completed,"output":output,"recovery":retained}));
        } else {
            assert!(output.is_none());
            let until = Instant::now() + Duration::from_secs(4);
            let retained = loop {
                if let Some(value) = recovery(&case, &completed) {
                    break value;
                }
                assert!(
                    Instant::now() < until,
                    "Original rejection recovery record deadline"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            assert_eq!(retained["output_state"], "rejected");
            assert_eq!(retained["reason"], "result_symlink_unsupported");
            assert!(retained.get("manifest").is_none() && retained.get("attestation").is_none());
            println!(
                "managed_https_export_rejected=verified; original_completed_receipt_retained=true; no_output_capability=true; start_replay_without_dispatch=true"
            );
            observations
                .push(serde_json::json!({"mode":mode,"receipt":completed,"recovery":retained}));
        }
    }
    private(
        &base.join("managed-service-observation.json"),
        &serde_json::to_vec(&observations).unwrap(),
    );
    // Stop only the positively owned fixture service after both guests ended.
    assert_eq!(
        unsafe { libc::kill(service.0.id() as i32, libc::SIGINT) },
        0
    );
    assert!(service.0.wait().unwrap().success());
}
