//! Metadata custody components only. The deliberately constructed retired state
//! proves no task completion, unmount, loop detachment or runtime qualification.
use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    pool: VolumePool,
    scratch: Dir,
    original: RetiringPublishedVolume,
}
impl Fixture {
    fn new() -> Self {
        assert_eq!(unsafe { libc::geteuid() }, 0);
        let temporary = tempfile::tempdir().unwrap();
        let parent = Dir::open_ambient_dir(temporary.path(), cap_std::ambient_authority()).unwrap();
        let root = private_dir(&parent, "pool").unwrap();
        let scratch = private_dir(&parent, "scratch").unwrap();
        let formatter_path = Path::new("/usr/sbin/mke2fs").canonicalize().unwrap();
        let formatter_hash = format!(
            "{:x}",
            Sha256::digest(std::fs::read(&formatter_path).unwrap())
        );
        let pool = VolumePool::initialize(
            root,
            PoolLimits {
                reserved_bytes: 64 * 1024 * 1024,
                max_volumes: 2,
            },
            PinnedFormatter::open(&formatter_path, &formatter_hash).unwrap(),
        )
        .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let namespace = private_dir(&pool.0.volumes, &id).unwrap();
        let base = private_dir(&namespace, "root").unwrap();
        let image = create_file(&namespace, "image.ext4").unwrap();
        let journals = [
            journal_file(&namespace, "volume.a").unwrap(),
            journal_file(&namespace, "volume.b").unwrap(),
        ];
        let mut record = Record {
            format: 1,
            sequence: 0,
            id,
            nonce: "a".repeat(32),
            request: VolumeRequest {
                request_sha256: "b".repeat(64),
                bytes: 32 * 1024 * 1024,
                inodes: 2048,
            },
            namespace: NativeId::directory(&namespace).unwrap(),
            image: NativeId::file(&image).unwrap(),
            base: NativeId::directory(&base).unwrap(),
            journal: journals
                .each_ref()
                .map(|file| NativeId::file(file).unwrap()),
            phase: Phase::PublishedRetired,
            device: None,
            mounted: None,
            mount_id: None,
            data_root: None,
        };
        let mut fence = create_file(&namespace, "publish.fence").unwrap();
        fence.write_all(fence_bytes(&record).as_bytes()).unwrap();
        fence.sync_all().unwrap();
        replace_json(&namespace, &mut record).unwrap();
        namespace.remove_file("image.ext4").unwrap();
        sync(&namespace).unwrap();
        // Synthetic component state, inaccessible outside this private module.
        let retired = RetiredPublishedVolume {
            volume_id: record.id.clone(),
            request_sha256: record.request.request_sha256.clone(),
            reserved_bytes: record.request.bytes,
            reserved_inodes: record.request.inodes,
            namespace: record.namespace.clone(),
            image: record.image.clone(),
            base: record.base.clone(),
            unmounted: true,
            loop_detached: true,
            image_absent: true,
        };
        let original = RetiringPublishedVolume {
            pool: pool.0.clone(),
            namespace,
            record,
            retired: Some(retired),
            collection: None,
            collected: false,
        };
        Self {
            _temporary: temporary,
            pool,
            scratch,
            original,
        }
    }
    fn pin_component_collection(&mut self) {
        let ns = &self.original.namespace;
        self.original.collection = Some(MetadataCollection {
            root: Some(ns.open_dir_nofollow("root").unwrap()),
            fence: Some(open_file(ns, "publish.fence", false).unwrap()),
            journals: [
                Some(open_file(ns, "volume.a", true).unwrap()),
                Some(open_file(ns, "volume.b", true).unwrap()),
            ],
        });
    }
}

#[test]
#[ignore = "UID0 own ext4 metadata component; no workloads or mounted volumes"]
fn first_validation_failure_retains_original_metadata_capability_for_retry() {
    let mut f = Fixture::new();
    drop(create_file(&f.original.namespace, "unexpected").unwrap());
    assert!(f.original.collect_metadata().is_err());
    assert!(f.original.collection.is_none());
    assert!(!f.original.collected);
    f.original.namespace.remove_file("unexpected").unwrap();
    f.original.collect_metadata().unwrap();
    f.original.collect_metadata().unwrap();
    let usage = f.pool.usage().unwrap();
    assert_eq!(
        (
            usage.reserved_bytes,
            usage.reserved_volumes,
            usage.retained_records
        ),
        (0, 0, 0)
    );
}

#[test]
#[ignore = "UID0 own ext4 metadata component; no workloads or mounted volumes"]
fn partial_collection_keeps_pinned_remaining_file_and_rejects_path_replacement() {
    let mut f = Fixture::new();
    f.pin_component_collection();
    // Model the actual namespace after successful preceding unlinks. Keeping
    // their original descriptors also covers lost successful fsync replies.
    let ns = &f.original.namespace;
    ns.remove_dir("root").unwrap();
    ns.remove_file("publish.fence").unwrap();
    ns.remove_file("volume.a").unwrap();
    ns.rename("volume.b", &f.scratch, "original-b").unwrap();
    drop(create_file(ns, "volume.b").unwrap());
    let replacement = NativeId::file(&open_file(ns, "volume.b", true).unwrap()).unwrap();
    assert!(matches!(
        f.original.collect_metadata(),
        Err(VolumeError::Identity)
    ));
    let state = f.original.collection.as_ref().unwrap();
    assert!(state.root.is_none() && state.fence.is_none() && state.journals[0].is_none());
    let original_id = NativeId::file(state.journals[1].as_ref().unwrap()).unwrap();
    assert_ne!(original_id, replacement);
    assert_eq!(original_id, f.original.record.journal[1]);
    assert!(
        f.pool.usage().is_err(),
        "Partial metadata cannot become a free/adopted reservation"
    );
    f.original.namespace.remove_file("volume.b").unwrap();
    f.scratch
        .rename("original-b", &f.original.namespace, "volume.b")
        .unwrap();
    f.original.collect_metadata().unwrap();
    assert!(f.original.collected);
    f.original.collect_metadata().unwrap();
    assert_eq!(f.pool.usage().unwrap().retained_records, 0);
}
