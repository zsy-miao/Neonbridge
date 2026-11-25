use super::*;

#[test]
fn a_directory_is_listed_once_and_extended() {
    let dir = std::env::temp_dir().join(format!("tuxbox-names-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.sha256"), b"").unwrap();
    let names = Names::new();
    assert!(names.of(&dir).contains("a.sha256"));
    fs::write(dir.join("b.sha256"), b"").unwrap();
    assert!(!names.of(&dir).contains("b.sha256"));
    names.of(&dir).insert("b.sha256".into());
    assert!(names.of(&dir).contains("b.sha256"));
    fs::remove_dir_all(&dir).unwrap();
}
