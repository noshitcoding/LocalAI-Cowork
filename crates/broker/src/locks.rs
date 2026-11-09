use std::fs::File;

/// Holds the broker's OS lock until the owning operation finishes. No raw handle
/// or clone is exposed: callers cannot extend the admission by copying this guard.
/// Closing only the descriptor is insufficient on Unix when a fork/duplicate
/// temporarily retains the same open-file description. Unlock explicitly first.
#[derive(Debug)]
pub struct ResourceLock(pub(crate) File);
impl Drop for ResourceLock {
    fn drop(&mut self) {
        // If explicit unlock fails, closing remains conservative. Other operations
        // may stay blocked; this does not grant another operation an unsafe lock.
        let _ = self.0.unlock();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn completed_writer_releases_lock_even_when_duplicate_descriptor_remains_open() {
        exercise(false);
    }
    #[test]
    fn completed_reader_releases_lock_even_when_duplicate_descriptor_remains_open() {
        exercise(true);
    }
    fn exercise(shared: bool) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lock");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        if shared {
            file.lock_shared().unwrap();
        } else {
            file.lock().unwrap();
        }
        let guard = ResourceLock(file);
        // dup(), like fork(), retains the original open-file description.
        let inherited = guard.0.try_clone().unwrap();
        let next = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(matches!(
            next.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(guard);
        next.try_lock()
            .expect("completed operation must release despite inherited descriptor");
        next.unlock().unwrap();
        drop(inherited);
    }
}
