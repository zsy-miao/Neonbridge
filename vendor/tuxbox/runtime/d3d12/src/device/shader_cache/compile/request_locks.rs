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
mod tests {
    use super::*;

    #[test]
    fn same_key_serializes_but_old_bucket_collision_does_not() {
        let registry = RequestLocks::default();
        let a = registry.for_key("00-first");
        let alias = registry.for_key("00-first");
        let b = registry.for_key("20-second");
        assert!(Arc::ptr_eq(&a, &alias));
        let _held = a.lock().unwrap();
        assert!(alias.try_lock().is_err());
        assert!(b.try_lock().is_ok());
    }

    #[test]
    fn completed_requests_do_not_accumulate() {
        let registry = RequestLocks::default();
        let active = registry.for_key("active");
        for n in 0..1000 {
            drop(registry.for_key(&n.to_string()));
        }
        assert!(Arc::ptr_eq(&active, &registry.for_key("active")));
        let _next = registry.for_key("next");
        assert_eq!(registry.0.lock().unwrap().len(), 2);
    }
}
