use super::*;

#[test]
fn reports_each_key_once_and_the_cap_once() {
    let seen = Seen::<String>::new(2);
    assert_eq!(seen.sight("a"), Sighting::New);
    assert_eq!(seen.sight("a"), Sighting::Repeat);
    assert_eq!(seen.sight("b"), Sighting::New);
    assert_eq!(seen.sight("c"), Sighting::Full);
    assert_eq!(seen.sight("d"), Sighting::Repeat);
    assert_eq!(seen.sight("b"), Sighting::Repeat);
}
