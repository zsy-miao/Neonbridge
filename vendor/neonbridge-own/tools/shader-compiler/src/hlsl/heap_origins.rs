//! Must-alias equivalence classes for words of dynamic heap descriptors.
//! A load site is a fresh class within its block. At a CFG edge rename
//! classes into the destination block's namespace, disjoint from all load
//! sites. A join intersects equivalence relations, not independent sets of
//! possible producers: words remain related only if related on every path.
use super::Sym;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind { Resource(u32), Sampler, Word(u8) }

fn parts(s: Sym) -> Option<(Kind, usize)> {
    match s {
        Sym::HeapDescriptor(n, Some(o)) => Some((Kind::Resource(n), o)),
        Sym::HeapSamplerIndex(Some(o)) => Some((Kind::Sampler, o)),
        Sym::HeapWord(k, Some(o)) => Some((Kind::Word(k), o)),
        _ => None,
    }
}

pub(super) fn tracked(s: Sym) -> bool { parts(s).is_some() }

fn symbol(kind: Kind, origin: usize) -> Sym {
    match kind {
        Kind::Resource(n) => Sym::HeapDescriptor(n, Some(origin)),
        Kind::Sampler => Sym::HeapSamplerIndex(Some(origin)),
        Kind::Word(k) => Sym::HeapWord(k, Some(origin)),
    }
}

fn block_origin(block: usize, register: u16) -> usize {
    // resolve_resources bounds the instruction count before enabling this.
    (1usize << (usize::BITS - 1)) | (block << 16) | usize::from(register)
}

pub(super) fn intersection(a: &HashMap<u16, Sym>, b: &HashMap<u16, Sym>, block: usize) -> Vec<(u16, Sym)> {
    let rows: Vec<_> = a.iter().filter_map(|(&r, &s)| {
        let (kind, x) = parts(s)?;
        let (other, y) = parts(*b.get(&r)?)?;
        (kind == other).then_some((r, kind, (x, y)))
    }).collect();
    let mut groups = HashMap::new();
    for &(r, _, pair) in &rows {
        groups.entry(pair).and_modify(|min: &mut u16| *min = (*min).min(r)).or_insert(r);
    }
    rows.into_iter().map(|(r, kind, pair)| (r, symbol(kind, block_origin(block, groups[&pair])))).collect()
}

pub(super) fn canonicalize(sym: &mut HashMap<u16, Sym>, block: usize) {
    let new = intersection(sym, sym, block);
    sym.retain(|_, s| !tracked(*s));
    sym.extend(new);
}

#[cfg(test)]
mod tests {
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
}
