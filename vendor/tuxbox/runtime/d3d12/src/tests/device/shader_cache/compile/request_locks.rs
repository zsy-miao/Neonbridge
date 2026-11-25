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
