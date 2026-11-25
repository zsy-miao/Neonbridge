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
#[path = "../tests/hlsl/heap_origins.rs"]
mod tests;
