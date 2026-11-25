use super::*;

fn root(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("tuxbox-save-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn names_cannot_leave_the_root() {
    for n in ["..", "../x", "a\\b", "C:\\x", "a.tmp", "héllo", "", "con*?"] {
        let e = encode(n);
        assert!(!e.contains(['.', '/', '\\', ':', '*', '?']), "{e}");
        assert_eq!(decode(&e).as_deref(), Some(n));
    }
    assert_eq!(decode("%"), None);
    assert_eq!(decode("%zz"), None);
    assert!(container(&root("empty"), "").is_none());
}

#[test]
fn round_trip_replace_delete() {
    let r = root("rt");
    assert!(container(&r, "Player").is_none());
    commit(&r, "Player", "Slot 1", &[("a".into(), Some(vec![1, 2, 3])), ("b.c".into(), Some(vec![]))]).unwrap();
    let c = container(&r, "Player").unwrap();
    assert_eq!((c.display.as_str(), c.blobs.clone()), ("Slot 1", vec![("a".into(), 3), ("b.c".into(), 0)]));
    assert!(c.modified > 0);
    assert_eq!(read(&r, "Player", "a"), Some(vec![1, 2, 3]));
    commit(&r, "Player", "Slot 2", &[("a".into(), Some(vec![9])), ("b.c".into(), None), ("none".into(), None)]).unwrap();
    assert_eq!(container(&r, "Player").unwrap().blobs, vec![("a".into(), 1)]);
    assert_eq!(read(&r, "Player", "a"), Some(vec![9]));
    assert_eq!(containers(&r).len(), 1);
    delete(&r, "Player").unwrap();
    delete(&r, "Player").unwrap();
    assert!(containers(&r).is_empty());
}
