//! Copy next to baseline.rs and candidate.rs (the two heap_page_cache.rs
//! implementations), then compile with rustc --edition 2024 -O.
#[path = "baseline.rs"] mod baseline;
#[path = "candidate.rs"] mod candidate;
use std::{hint::black_box, sync::Arc, time::Instant};

macro_rules! run {
    ($module:ident) => {{
        let mut cache = $module::PageCache::default();
        let live: Vec<_> = (0..4096).map(|n| cache.get_or_open(n, |_| true, || Ok::<_, ()>(Arc::new(n))).unwrap()).collect();
        let start = Instant::now();
        for n in 0..100000 {
            let value = cache.get_or_open(100000+n, |_| true, || Ok::<_, ()>(Arc::new(n))).unwrap();
            black_box(value);
        }
        black_box(live);
        start.elapsed().as_secs_f64()
    }}
}
fn main() {
    for i in 0..5 {
        let (a,b) = if i%2 == 0 { (run!(baseline),run!(candidate)) }
        else { let b = run!(candidate); (run!(baseline),b) };
        println!("{a:.9} {b:.9}");
    }
}
