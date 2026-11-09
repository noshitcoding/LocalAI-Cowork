use cap_std::{ambient_authority, fs::Dir};
use cowork_managed_runtime::*;
use ring::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
#[path = "packages/release_floor.rs"]
mod release_floor;
#[cfg(any(target_os = "linux", windows))]
#[path = "packages/staging.rs"]
mod staging;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
struct Fixture {
    key: Ed25519KeyPair,
    trust: TrustRoot,
    manifest: Value,
}
impl Fixture {
    fn new() -> Self {
        let key = Ed25519KeyPair::from_pkcs8(
            Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
                .unwrap()
                .as_ref(),
        )
        .unwrap();
        let trust = TrustRoot::new(
            "fixture-only".into(),
            key.public_key().as_ref().try_into().unwrap(),
        )
        .unwrap();
        // Tiny synthetic bytes qualify only this signature/stream contract, not
        // real programs, root filesystems, tool packages or runtime isolation.
        let roles = [
            "base_rootfs",
            "browser_tools",
            "containerd_bundle",
            "document_tools",
            "git_tools",
            "gvisor_bundle",
            "node_tools",
            "python_tools",
            "runtime_service",
        ];
        let components:Vec<_>=roles.iter().map(|role| json!({"role":role,"filename":format!("{role}.payload"),"version":"fixture.1","bytes":4,"sha256":sha(b"test")})).collect();
        Self {
            key,
            trust,
            manifest: json!({"format_version":1,"platform":"linux_x86_64","version":"2.0.0-alpha.7","release_sequence":1,"components":components}),
        }
    }
    fn sign(&self, bytes: &[u8]) -> Vec<u8> {
        let mut input = SIGNING_DOMAIN.to_vec();
        input.extend_from_slice(bytes);
        serde_json::to_vec(&PackageSignature {
            key_id: "fixture-only".into(),
            manifest_sha256: sha(bytes),
            signature: self.key.sign(&input).as_ref().to_vec(),
        })
        .unwrap()
    }
    fn verify(&self, value: &Value) -> Result<VerifiedPackage> {
        let bytes = serde_json::to_vec(value).unwrap();
        verify_manifest(
            &bytes,
            &self.sign(&bytes),
            &self.trust,
            Platform::LinuxX86_64,
        )
    }
    fn directory(&self) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("payload")).unwrap();
        let bytes = serde_json::to_vec_pretty(&self.manifest).unwrap();
        std::fs::write(dir.path().join("manifest.json"), &bytes).unwrap();
        std::fs::write(dir.path().join("manifest.sig.json"), self.sign(&bytes)).unwrap();
        for c in self.manifest["components"].as_array().unwrap() {
            std::fs::write(
                dir.path()
                    .join("payload")
                    .join(c["filename"].as_str().unwrap()),
                b"test",
            )
            .unwrap();
        }
        dir
    }
}

#[test]
fn exact_manifest_bytes_platform_and_external_trust_are_all_bound() {
    let f = Fixture::new();
    let bytes = serde_json::to_vec_pretty(&f.manifest).unwrap();
    let signed = f.sign(&bytes);
    let p = verify_manifest(&bytes, &signed, &f.trust, Platform::LinuxX86_64).unwrap();
    assert_eq!(p.manifest_sha256(), sha(&bytes));
    assert_eq!(p.manifest().release_sequence, 1);
    assert!(matches!(
        verify_manifest(&bytes, &signed, &f.trust, Platform::WindowsWsl2X86_64),
        Err(Error::Platform)
    ));
    assert!(matches!(
        verify_manifest(
            &serde_json::to_vec(&f.manifest).unwrap(),
            &signed,
            &f.trust,
            Platform::LinuxX86_64
        ),
        Err(Error::Signature)
    ));
    let other = Fixture::new();
    assert!(matches!(
        verify_manifest(&bytes, &signed, &other.trust, Platform::LinuxX86_64),
        Err(Error::Signature)
    ));
    let mut sig: Value = serde_json::from_slice(&signed).unwrap();
    sig["key_id"] = json!("untrusted");
    assert!(matches!(
        verify_manifest(
            &bytes,
            &serde_json::to_vec(&sig).unwrap(),
            &f.trust,
            Platform::LinuxX86_64
        ),
        Err(Error::Signature)
    ));
    let mut sig: Value = serde_json::from_slice(&signed).unwrap();
    sig["signature"] = json!(f.key.sign(&bytes).as_ref());
    assert!(matches!(
        verify_manifest(
            &bytes,
            &serde_json::to_vec(&sig).unwrap(),
            &f.trust,
            Platform::LinuxX86_64
        ),
        Err(Error::Signature)
    ));
}

#[test]
fn an_independent_python_signature_uses_the_same_public_bytes() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/python-package.json")).unwrap();
    let key: Vec<u8> = serde_json::from_value(fixture["public_key"].clone()).unwrap();
    let trust = TrustRoot::new("python-fixture".into(), key.try_into().unwrap()).unwrap();
    let bytes = fixture["manifest_json"].as_str().unwrap().as_bytes();
    let verified = verify_manifest(
        bytes,
        &serde_json::to_vec(&fixture["signature"]).unwrap(),
        &trust,
        Platform::LinuxX86_64,
    )
    .unwrap();
    assert_eq!(
        verified.manifest_sha256(),
        fixture["signature"]["manifest_sha256"].as_str().unwrap()
    );
}

#[test]
fn even_valid_signatures_do_not_admit_incomplete_aliased_or_unbounded_packages() {
    let f = Fixture::new();
    for path in [
        "../escape",
        "nested/file",
        "C:secret",
        "CON.txt",
        "last.",
        "space name",
    ] {
        let mut m = f.manifest.clone();
        m["components"][0]["filename"] = json!(path);
        assert!(f.verify(&m).is_err(), "{path}");
    }
    let mut m = f.manifest.clone();
    m["components"].as_array_mut().unwrap().remove(0);
    assert!(f.verify(&m).is_err());
    let mut m = f.manifest.clone();
    m["components"][1]["role"] = m["components"][0]["role"].clone();
    assert!(f.verify(&m).is_err());
    let mut m = f.manifest.clone();
    m["components"][1]["filename"] = json!("BASE_ROOTFS.PAYLOAD");
    assert!(f.verify(&m).is_err());
    let mut m = f.manifest.clone();
    m["components"][0]["bytes"] = json!(u64::MAX);
    assert!(f.verify(&m).is_err());
    let mut m = f.manifest.clone();
    m["release_sequence"] = json!(0);
    assert!(f.verify(&m).is_err());
    let mut m = f.manifest.clone();
    m["qualified"] = json!(true);
    assert!(f.verify(&m).is_err());
}

#[test]
fn corrupt_truncated_or_overlong_streams_fail_with_bounded_reading() {
    let f = Fixture::new();
    let p = f.verify(&f.manifest).unwrap();
    for bytes in [b"tes".as_slice(), b"fail", b"test-extra"] {
        assert!(
            p.verify_component(ComponentRole::RuntimeService, &mut Cursor::new(bytes))
                .is_err()
        );
    }
    struct Infinite {
        read: usize,
    }
    impl Read for Infinite {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            b.fill(b'x');
            self.read += b.len();
            Ok(b.len())
        }
    }
    let mut reader = Infinite { read: 0 };
    assert!(
        p.verify_component(ComponentRole::RuntimeService, &mut reader)
            .is_err()
    );
    assert_eq!(reader.read, 5);
    p.verify_component(ComponentRole::RuntimeService, &mut Cursor::new(b"test"))
        .unwrap();
}

#[test]
fn actual_directory_rejects_extra_files_corrupt_payloads_and_hardlinks() {
    let f = Fixture::new();
    let dir = f.directory();
    let open = || Dir::open_ambient_dir(dir.path(), ambient_authority()).unwrap();
    verify_directory(&open(), &f.trust, Platform::LinuxX86_64).unwrap();
    let file = dir.path().join("payload/base_rootfs.payload");
    std::fs::write(&file, b"fail").unwrap();
    assert!(verify_directory(&open(), &f.trust, Platform::LinuxX86_64).is_err());
    std::fs::write(&file, b"test").unwrap();
    std::fs::write(dir.path().join("payload/extra"), b"test").unwrap();
    assert!(verify_directory(&open(), &f.trust, Platform::LinuxX86_64).is_err());
    std::fs::remove_file(dir.path().join("payload/extra")).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let link = outside.path().join("linked-original");
    std::fs::hard_link(&file, &link).unwrap();
    // Keep the hardlink outside the payload; the descriptor's link count, not
    // merely enumeration of the expected filenames, must reject it.
    assert!(verify_directory(&open(), &f.trust, Platform::LinuxX86_64).is_err());
    std::fs::remove_file(&link).unwrap();
    verify_directory(&open(), &f.trust, Platform::LinuxX86_64).unwrap();
}

#[cfg(unix)]
#[test]
fn native_symlinks_and_fifos_cannot_replace_package_components() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let dir = f.directory();
    let root = Dir::open_ambient_dir(dir.path(), ambient_authority()).unwrap();
    let file = dir.path().join("payload/base_rootfs.payload");
    std::fs::remove_file(&file).unwrap();
    symlink("/etc/machine-id", &file).unwrap();
    assert!(verify_directory(&root, &f.trust, Platform::LinuxX86_64).is_err());
    std::fs::remove_file(&file).unwrap();
    let path = std::ffi::CString::new(file.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let started = Instant::now();
    assert!(verify_directory(&root, &f.trust, Platform::LinuxX86_64).is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn native_cli_verifies_bytes_but_never_claims_installation_or_qualification() {
    let f = Fixture::new();
    let dir = f.directory();
    let config = tempfile::tempdir().unwrap();
    let key = config.path().join("trusted.pub");
    std::fs::write(&key, f.key.public_key().as_ref()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cowork-runtime-package"))
        .arg(dir.path())
        .arg(&key)
        .arg("fixture-only")
        .arg("linux_x86_64")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["signature_verified"], true);
    assert_eq!(receipt["payload_verified"], true);
    assert_eq!(receipt["installed"], false);
    assert_eq!(receipt["qualified"], false);
    assert_eq!(receipt["execution_authority"], false);
    let key = dir.path().join("self.pub");
    std::fs::write(&key, f.key.public_key().as_ref()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cowork-runtime-package"))
        .arg(dir.path())
        .arg(&key)
        .arg("fixture-only")
        .arg("linux_x86_64")
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[cfg(windows)]
#[test]
fn a_native_windows_junction_cannot_substitute_the_payload_directory() {
    use std::os::windows::fs::MetadataExt;
    let f = Fixture::new();
    let original = f.directory();
    let package = tempfile::tempdir().unwrap();
    for name in ["manifest.json", "manifest.sig.json"] {
        std::fs::copy(original.path().join(name), package.path().join(name)).unwrap();
    }
    let link = package.path().join("payload");
    let target = original.path().join("payload").canonicalize().unwrap();
    assert!(target.starts_with(original.path().canonicalize().unwrap()));
    assert!(
        link.parent().unwrap().canonicalize().unwrap() == package.path().canonicalize().unwrap()
    );
    let result = std::process::Command::new("cmd.exe")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_ne!(
        std::fs::symlink_metadata(&link).unwrap().file_attributes() & 0x400,
        0
    );
    let root = Dir::open_ambient_dir(package.path(), ambient_authority()).unwrap();
    assert!(verify_directory(&root, &f.trust, Platform::LinuxX86_64).is_err());
    drop(root);
    // Remove only our exact junction; the original payload is retained.
    std::fs::remove_dir(&link).unwrap();
    assert_eq!(
        std::fs::read(target.join("base_rootfs.payload")).unwrap(),
        b"test"
    );
}
