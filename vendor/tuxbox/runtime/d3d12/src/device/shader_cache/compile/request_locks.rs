//! Serialize publication for identical requests without blocking unrelated keys.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

#[derive(Default)]
pub(super) struct RequestLocks(Mutex<HashMap<String, Weak<Mutex<()>>>>);

impl RequestLocks {
    pub(super) fn for_key(&self, key: &str) -> Arc<Mutex<()>> {
        let mut locks = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(lock) = locks.get(key).and_then(Weak::upgrade) {
            return lock;
        }
        // The registry does not own the locks. Remove entries for completed
        // jobs so it is bounded by outstanding keys rather than title lifetime.
        locks.retain(|_, lock| lock.strong_count() != 0);
        let lock = Arc::new(Mutex::new(()));
        locks.insert(key.to_owned(), Arc::downgrade(&lock));
        lock
    }
}

#[cfg(test)]
#[path = "../../../tests/device/shader_cache/compile/request_locks.rs"]
mod tests;
