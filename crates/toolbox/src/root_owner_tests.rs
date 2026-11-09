use super::*;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("cowork-root-owner-{}", Uuid::new_v4().simple()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn config(&self) -> Config {
        Config {
            root: self.0.clone(),
            programs: BTreeMap::new(),
            environment: BTreeMap::new(),
            github_token: None,
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn second_open_preserves_live_document_and_office_files() {
    let scratch = Scratch::new();
    let first = Toolbox::open(scratch.config()).unwrap();
    let stage = scratch
        .0
        .join("document-stages")
        .join(Uuid::new_v4().to_string());
    let profile = scratch
        .0
        .join(".lo")
        .join(Uuid::new_v4().simple().to_string());
    for path in [&stage, &profile] {
        fs::create_dir(path).unwrap();
        fs::write(path.join("sentinel"), b"original").unwrap();
    }
    assert!(matches!(
        Toolbox::open(scratch.config()),
        Err(Error::RootInUse)
    ));
    for path in [&stage, &profile] {
        assert_eq!(fs::read(path.join("sentinel")).unwrap(), b"original");
    }
    let order = Uuid::new_v4().to_string();
    first
        .write_input_file(&order, "still-active.txt", b"first owner works")
        .unwrap();
    assert_eq!(
        fs::read(first.workspace(&order).unwrap().join("still-active.txt")).unwrap(),
        b"first owner works"
    );
    drop(first);
}

#[test]
fn all_original_references_must_close_before_reopen() {
    let scratch = Scratch::new();
    let first = Toolbox::open(scratch.config()).unwrap();
    let retained = first.clone();
    first.shutdown();
    drop(first);
    assert!(matches!(
        Toolbox::open(scratch.config()),
        Err(Error::RootInUse)
    ));
    drop(retained);
    let reopened = Toolbox::open(scratch.config()).unwrap();
    assert!(scratch.0.join(".toolbox-owner.lock").is_file());
    drop(reopened);
}

#[test]
fn independent_roots_can_open_together() {
    let one = Scratch::new();
    let two = Scratch::new();
    let first = Toolbox::open(one.config()).unwrap();
    let second = Toolbox::open(two.config()).unwrap();
    drop(first);
    drop(second);
}

#[test]
fn failed_setup_releases_only_its_own_root_lease() {
    let scratch = Scratch::new();
    let mut invalid = scratch.config();
    invalid
        .programs
        .insert("python".into(), scratch.0.join("missing.exe"));
    assert!(Toolbox::open(invalid).is_err());
    let owner = Toolbox::open(scratch.config()).unwrap();
    assert!(matches!(
        Toolbox::open(scratch.config()),
        Err(Error::RootInUse)
    ));
    drop(owner);
}

#[test]
fn hardlinked_lock_cannot_change_an_external_file() {
    let scratch = Scratch::new();
    let external = Scratch::new();
    let original = external.0.join("original");
    fs::write(&original, b"outside").unwrap();
    fs::hard_link(&original, scratch.0.join(".toolbox-owner.lock")).unwrap();
    assert!(Toolbox::open(scratch.config()).is_err());
    assert_eq!(fs::read(&original).unwrap(), b"outside");
    assert!(!scratch.0.join("document-stages").exists());
}
