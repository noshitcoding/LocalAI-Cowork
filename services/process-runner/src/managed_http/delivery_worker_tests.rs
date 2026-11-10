//! Scheduler/custody regression only; this file certifies no physical task.
use super::*;
use std::os::unix::fs::MetadataExt;

#[tokio::test]
async fn independent_worker_retries_same_original_handle_after_a_real_io_failure() {
    let temporary = tempfile::tempdir().unwrap();
    let named = temporary.path().join("named");
    let retained = temporary.path().join("original");
    std::fs::create_dir(&named).unwrap();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(named.join("metadata"))
        .unwrap();
    let original = Arc::new(file);
    let stamp = original.metadata().unwrap();
    let identity = (stamp.dev(), stamp.ino());
    std::fs::rename(&named, &retained).unwrap();
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    tokio::time::timeout(
        Duration::from_secs(3),
        retry_cleanup(
            || {
                let original = original.clone();
                let named = named.clone();
                let retained = retained.clone();
                let attempts = attempts.clone();
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    let held = original.metadata().unwrap();
                    assert_eq!((held.dev(), held.ino()), identity);
                    let removed = std::fs::remove_file(named.join("metadata"));
                    if attempt == 0 {
                        assert_eq!(removed.unwrap_err().kind(), std::io::ErrorKind::NotFound);
                        // The same original directory/file return; no replacement is
                        // accepted and the same captured descriptor stays in custody.
                        std::fs::rename(&retained, &named).unwrap();
                        false
                    } else {
                        removed.unwrap();
                        true
                    }
                }
            },
            Duration::from_millis(1),
        ),
    )
    .await
    .unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(Arc::strong_count(&original), 1);
    drop(original);
    temporary.close().unwrap();
}
