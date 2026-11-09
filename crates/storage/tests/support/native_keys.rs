#![allow(dead_code)]
use cowork_secrets::*;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Default)]
pub struct FakeKeys {
    values: Mutex<HashMap<String, [u8; 32]>>,
    pub creates: AtomicUsize,
    pub fail_after_create: AtomicBool,
    pub block_create: AtomicBool,
}
#[derive(Default, Clone)]
pub struct FakeProvider {
    pub keys: Arc<FakeKeys>,
}
impl FakeProvider {
    pub fn corrupt(&self, key: &KeyId) {
        self.keys
            .values
            .lock()
            .unwrap()
            .insert(key.cipher_id().unwrap(), [9; 32]);
    }
    pub fn remove(&self, key: &KeyId) {
        self.keys
            .values
            .lock()
            .unwrap()
            .remove(&key.cipher_id().unwrap());
    }
}
impl NativeKeyProvider for FakeProvider {
    fn open(&self, instance: Uuid) -> Result<Arc<dyn NativeKeyManager>> {
        if instance.is_nil() {
            return Err(Error::Invalid);
        }
        Ok(Arc::new(FakeManager {
            keys: self.keys.clone(),
            instance,
        }))
    }
}
struct FakeManager {
    keys: Arc<FakeKeys>,
    instance: Uuid,
}
impl KeyStore for FakeManager {
    fn load(&self, id: &KeyId) -> Result<Zeroizing<[u8; 32]>> {
        if self.instance != id.instance {
            return Err(Error::Invalid);
        }
        self.keys
            .values
            .lock()
            .unwrap()
            .get(&id.cipher_id()?)
            .copied()
            .map(Zeroizing::new)
            .ok_or(Error::Missing)
    }
}
impl NativeKeyManager for FakeManager {
    fn create(&self, id: &KeyId) -> Result<()> {
        if self.instance != id.instance {
            return Err(Error::Invalid);
        }
        let mut values = self.keys.values.lock().unwrap();
        let name = id.cipher_id()?;
        if values.contains_key(&name) {
            return Err(Error::Conflict);
        }
        values.insert(name, [7; 32]);
        drop(values);
        self.keys.creates.fetch_add(1, Ordering::SeqCst);
        while self.keys.block_create.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.keys.fail_after_create.swap(false, Ordering::SeqCst) {
            return Err(Error::Unconfirmed);
        }
        Ok(())
    }
}
