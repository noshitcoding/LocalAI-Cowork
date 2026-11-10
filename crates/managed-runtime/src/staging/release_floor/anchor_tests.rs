//! Deterministic test-build-only scheduling at native acquisition boundaries.
//! These are instrumented unit ELFs, never unchanged production/CLI ELF claims.
use super::*;
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::{Path, PathBuf},
};

type Hook = Box<dyn FnOnce()>;
thread_local! {
    static HOOKS: RefCell<BTreeMap<&'static str, Hook>> = RefCell::new(BTreeMap::new());
}
pub(super) fn fire(name: &'static str) {
    let hook = HOOKS.with(|h| h.borrow_mut().remove(name));
    if let Some(hook) = hook {
        hook();
    }
}
fn set(name: &'static str, hook: impl FnOnce() + 'static) {
    HOOKS.with(|h| assert!(h.borrow_mut().insert(name, Box::new(hook)).is_none()));
}
fn clear() {
    HOOKS.with(|h| h.borrow_mut().clear());
}
fn open(path: &Path) -> Dir {
    Dir::open_ambient_dir(path, cap_std::ambient_authority()).unwrap()
}
fn fixture(name: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let parent =
        PathBuf::from(std::env::var_os("COWORK_STAGING_ENOSPC_PARENT").unwrap()).join(name);
    std::fs::create_dir(&parent).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    parent
}
fn signed(sequence: u64) -> (TrustRoot, VerifiedPackage) {
    let key = Ed25519KeyPair::from_seed_unchecked(&[42; 32]).unwrap();
    let trust = TrustRoot::new(
        "fixture-anchor".into(),
        key.public_key().as_ref().try_into().unwrap(),
    )
    .unwrap();
    let manifest = PackageManifest {
        format_version: 1,
        platform: Platform::LinuxX86_64,
        version: "2.0.0-alpha.7".into(),
        release_sequence: sequence,
        components: ROLES
            .iter()
            .map(|role| PackageComponent {
                role: *role,
                filename: format!("{role:?}.payload"),
                version: "fixture.1".into(),
                bytes: 4,
                sha256: sha256(b"test"),
            })
            .collect(),
    };
    let raw = serde_json::to_vec(&manifest).unwrap();
    let mut message = SIGNING_DOMAIN.to_vec();
    message.extend_from_slice(&raw);
    let signature = serde_json::to_vec(&PackageSignature {
        key_id: "fixture-anchor".into(),
        manifest_sha256: sha256(&raw),
        signature: key.sign(&message).as_ref().to_vec(),
    })
    .unwrap();
    let verified = verify_manifest(&raw, &signature, &trust, Platform::LinuxX86_64).unwrap();
    (trust, verified)
}
fn policy(parent: &Path, trust: &TrustRoot) -> ReleaseFloor {
    ReleaseFloor::open(open(parent), "floor", trust, Platform::LinuxX86_64).unwrap()
}
fn initialize(parent: &Path, trust: &TrustRoot, package: &VerifiedPackage) -> ReleaseFloor {
    let mut floor =
        ReleaseFloor::initialize(open(parent), "floor", trust, Platform::LinuxX86_64).unwrap();
    floor.advance(package).unwrap();
    floor
}
fn move_files(from: &Path, to: &Path) {
    for name in FILES {
        std::fs::rename(from.join(name), to.join(name)).unwrap();
    }
}

#[test]
#[ignore = "own UID0 bounded policy-file acquisition race fixture; test-build-only scheduling"]
fn actual_anchor_acquisition_rejects_valid_replacement_file_set() {
    let parent = fixture("anchor-acquisition");
    let (trust, eleven) = signed(11);
    let (_, seven) = signed(7);
    let original = initialize(&parent, &trust, &eleven);
    assert!(original.check_candidate(&seven).is_err());
    let original_binding = original.current.binding.clone();
    let root = parent.join("floor");
    let held_a = fixture("anchor-acquisition-held-a");
    let held_b = fixture("anchor-acquisition-held-b");
    let p = parent.clone();
    let r = root.clone();
    let a = held_a.clone();
    set("anchor_before_pins", move || {
        move_files(&r, &a);
        let directory = open(&r);
        let owner = native::create_file(&directory, FILES[0]).unwrap();
        owner.sync_all().unwrap();
        let slots = [
            native::create_file(&directory, FILES[1]).unwrap(),
            native::create_file(&directory, FILES[2]).unwrap(),
        ];
        for slot in &slots {
            native::allocate(slot, SLOT_BYTES as u64).unwrap();
        }
        // Fixture-local reinitialization at the SAME original directory. Local
        // records use actual new native IDs; no signature or syscall is mocked.
        // This low-level construction is disclosed and independently opened
        // below as a positive control. No public reinitialization API exists.
        let binding = Binding {
            owner: LocalId::file(&owner).unwrap(),
            slots: [
                LocalId::file(&slots[0]).unwrap(),
                LocalId::file(&slots[1]).unwrap(),
            ],
            ..original_binding.clone()
        };
        let initial = Record {
            format_version: POLICY_FORMAT_VERSION,
            binding: binding.clone(),
            revision: 0,
            release_sequence: 0,
            manifest_sha256: None,
            manifest: None,
            signature: None,
        };
        let (t, candidate) = signed(7);
        let advanced = Record {
            revision: 1,
            release_sequence: 7,
            manifest_sha256: Some(candidate.manifest_sha256().into()),
            manifest: Some(String::from_utf8(candidate.signed_manifest).unwrap()),
            signature: Some(String::from_utf8(candidate.signed_signature).unwrap()),
            ..initial.clone()
        };
        for (mut file, record) in slots.iter().zip([initial, advanced]) {
            file.write_all(&frame(&record).unwrap()).unwrap();
            file.sync_all().unwrap();
        }
        native::sync_directory(&directory).unwrap();
        let control = policy(&p, &t);
        control.check_candidate(&signed(7).1).unwrap();
        assert_eq!(
            control.current.binding.directory,
            original_binding.directory
        );
        assert_ne!(control.current.binding.owner, original_binding.owner);
        println!(
            "ANCHOR_ACQUIRE_VALID_REPLACEMENT={}",
            serde_json::json!({"a":original_binding,"b":binding,"a_floor":11,"b_floor":7})
        );
    });
    let acquired = original.anchor();
    clear();
    let lower_admitted = acquired.as_ref().is_ok_and(|a| a.admit(&seven).is_ok());
    println!("ANCHOR_ACQUIRE_REPLACEMENT_OBSERVED={lower_admitted}");
    move_files(&root, &held_b);
    move_files(&held_a, &root);
    original.verify_current().unwrap();
    assert!(original.check_candidate(&seven).is_err());
    assert!(
        acquired.is_err(),
        "original policy adopted newly opened replacement file IDs"
    );
    println!("FLOOR_ANCHOR_ACQUISITION_REJECTED=true");
    // Additional GREEN control: even a valid lower signed local record written
    // through the SAME original slot inodes cannot lower this live anchor's
    // initial minimum. This is fixture-local mutation, not a public repair API.
    let retained = original.anchor().unwrap();
    let previous = [
        read_frame(&original.slots[0]).unwrap(),
        read_frame(&original.slots[1]).unwrap(),
    ];
    let writable = [
        original.slots[0].try_clone().unwrap(),
        original.slots[1].try_clone().unwrap(),
    ];
    drop(original);
    let (_, lower) = signed(7);
    let low = Record {
        release_sequence: 7,
        manifest_sha256: Some(lower.manifest_sha256().into()),
        manifest: Some(String::from_utf8(lower.signed_manifest).unwrap()),
        signature: Some(String::from_utf8(lower.signed_signature).unwrap()),
        ..previous[1].clone()
    };
    let mut slot = &writable[1];
    slot.seek(SeekFrom::Start(0)).unwrap();
    slot.write_all(&frame(&low).unwrap()).unwrap();
    slot.sync_all().unwrap();
    policy(&parent, &trust).check_candidate(&seven).unwrap();
    assert!(matches!(
        retained.admit(&seven),
        Err(Error::Archive("release_floor_anchor_minimum_changed"))
    ));
    for (mut slot, record) in writable.iter().zip(previous) {
        slot.seek(SeekFrom::Start(0)).unwrap();
        slot.write_all(&frame(&record).unwrap()).unwrap();
        slot.sync_all().unwrap();
    }
    drop(retained.admit(&eleven).unwrap());
    println!("FLOOR_ANCHOR_INITIAL_MINIMUM_RETAINED=true");
}

#[test]
#[ignore = "own UID0 bounded policy-directory ABA fixture; test-build-only scheduling"]
fn actual_anchor_admission_rejects_temporary_valid_policy_aba() {
    let parent = fixture("anchor-admission");
    let (trust, eleven) = signed(11);
    let (_, seven) = signed(7);
    let original = initialize(&parent, &trust, &eleven);
    let anchor = original.anchor().unwrap();
    let a_binding = original.current.binding.clone();
    drop(original);
    assert!(anchor.admit(&seven).is_err());
    let root = parent.join("floor");
    let held_a = parent.join("held-a");
    let held_b = parent.join("held-b");
    std::fs::rename(&root, &held_a).unwrap();
    // B is produced only by existing public initialize+advance at the same leaf.
    let b = initialize(&parent, &trust, &seven);
    let b_binding = b.current.binding.clone();
    b.check_candidate(&seven).unwrap();
    drop(b);
    std::fs::rename(&root, &held_b).unwrap();
    std::fs::rename(&held_a, &root).unwrap();
    assert_ne!(a_binding.directory, b_binding.directory);
    assert!(policy(&parent, &trust).check_candidate(&seven).is_err());
    println!(
        "ANCHOR_ADMIT_VALID_REPLACEMENT={}",
        serde_json::json!({"a":a_binding,"b":b_binding,"a_floor":11,"b_floor":7})
    );
    let r = root.clone();
    let a = held_a.clone();
    let b = held_b.clone();
    set("admit_before_open", move || {
        std::fs::rename(&r, &a).unwrap();
        std::fs::rename(&b, &r).unwrap();
    });
    let r = root.clone();
    let a = held_a.clone();
    let b = held_b.clone();
    set("admit_after_check", move || {
        std::fs::rename(&r, &b).unwrap();
        std::fs::rename(&a, &r).unwrap();
    });
    let admitted = anchor.admit(&seven);
    clear();
    // A corrected implementation can reject B before the second hook fires.
    if held_a.exists() {
        std::fs::rename(&root, &held_b).unwrap();
        std::fs::rename(&held_a, &root).unwrap();
    }
    anchor.verify().unwrap();
    let a_lock_available_while_b_returned = admitted.is_ok()
        && ReleaseFloor::open(open(&parent), "floor", &trust, Platform::LinuxX86_64).is_ok();
    println!(
        "ANCHOR_ADMIT_ABA_GUARD_RETURNED={};A_WRITER_UNLOCKED={a_lock_available_while_b_returned}",
        admitted.is_ok()
    );
    assert!(
        admitted.is_err(),
        "returned B guard after final name verification saw A"
    );
    println!("FLOOR_ANCHOR_ADMISSION_ABA_REJECTED=true");
}
