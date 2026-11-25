use super::*;
fn desc(origin: usize, base: u16) -> HashMap<u16, Sym> {
    (0..4).map(|k| (base+k, if k==0 { Sym::HeapDescriptor(4,Some(origin)) }
        else { Sym::HeapWord(k as u8,Some(origin)) })).collect()
}
#[test]
fn complete_alternatives_join_but_mixed_words_do_not() {
    let a=desc(1,4);let mut b=desc(2,4);
    let joined:HashMap<_,_>=intersection(&a,&b,3).into_iter().collect();
    assert!(super::super::heap_descriptor(&joined,4,4));
    assert_eq!(joined,intersection(&joined,&b,3).into_iter().collect());
    b.insert(5,Sym::HeapWord(1,Some(3)));
    let mixed:HashMap<_,_>=intersection(&joined,&b,3).into_iter().collect();
    assert!(!super::super::heap_descriptor(&mixed,4,4));
    assert_eq!(mixed,intersection(&mixed,&a,3).into_iter().collect());
}
#[test]
fn loop_entry_is_not_a_new_load_generation() {
    let mut state=desc(1,4);
    canonicalize(&mut state,2);
    state.insert(25,state[&5]); // retained word from previous iteration
    state.extend(desc(1,4)); // same site, new generation
    state.insert(5,state[&25]);
    assert!(!super::super::heap_descriptor(&state,4,4));
}
#[test]
fn missing_or_incompatible_words_are_not_restored() {
    let a=desc(1,4);let mut b=desc(2,4);b.remove(&5);
    b.insert(6,Sym::HeapWord(1,Some(2)));
    let joined:HashMap<_,_>=intersection(&a,&b,2).into_iter().collect();
    assert!(!joined.contains_key(&5)&&!joined.contains_key(&6));
}
