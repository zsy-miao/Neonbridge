//! How long a texture refresh holds title memory still (`watch::title_memory_reading`)
//! on this machine, step by step, for textures of the sizes the title streams: the copy
//! of the tiled bytes, the hash that tells whether they changed, and the detile into
//! linear rows. A refresh holds the lock through all three today; the title's page
//! remaps (`map_alias`) wait for it. Ignored by default:
//!   cargo test --release -p victory-gpu --test texture_hold -- --ignored --nocapture

use std::time::Instant;

use victory_formats::texture::{Desc, Dimension, Layout, Swizzle};

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[test]
#[ignore = "a measurement: run by hand"]
fn how_long_a_texture_refresh_holds_title_memory_still() {
    // (name, texels per side, mips, bytes per element, texels per element side)
    let cases = [
        ("BC1 1024x1024", 1024u32, 11u32, 8u32, 4u32),
        ("BC7 2048x2048", 2048, 12, 16, 4),
        ("BC7 4096x4096", 4096, 13, 16, 4),
        ("RGBA8 2048x2048", 2048, 12, 4, 1),
    ];
    println!();
    println!(
        "{:<17} {:>8} | {:>8} {:>8} {:>8} | {:>12} {:>12} | {:>7}",
        "texture", "MB", "copy ms", "hash ms", "detile", "held today", "held: copy", "shorter"
    );
    let (mut today_per_mb, mut copy_per_mb, mut mbs) = (0.0, 0.0, 0.0);
    for (name, side, mips, bytes, block) in cases {
        let desc = Desc {
            dimension: Dimension::Tex2d,
            width: side,
            height: side,
            depth: 1,
            mips,
            element_bytes: bytes,
            block,
            samples: 1,
            swizzle: Swizzle::SW_64KB_S,
        };
        let layout = Layout::new(&desc).expect("a layout");
        let size = layout.size as usize;
        let tiled: Vec<u8> = (0..size).map(|i| (i as u8).wrapping_mul(29) ^ (i >> 9) as u8).collect();
        let mut copy = vec![0u8; size];
        // Linear rows of every mip, as the staging buffer holds them (256-byte row pitch).
        let pitches: Vec<(usize, usize)> = layout
            .mips
            .iter()
            .map(|m| {
                let pitch = (m.width as usize * bytes as usize).next_multiple_of(256);
                (pitch, pitch * m.height as usize)
            })
            .collect();
        let mut linear = vec![0u8; pitches.iter().map(|p| p.1).sum()];
        let rounds = if size > 32 << 20 { 7 } else { 15 };
        let (mut c, mut h, mut d) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..rounds {
            let t = Instant::now();
            copy.copy_from_slice(&tiled);
            c.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            let hash = blake3::hash(std::hint::black_box(&tiled));
            h.push(t.elapsed().as_secs_f64() * 1e3);
            std::hint::black_box(hash);
            let t = Instant::now();
            let mut at = 0;
            for (mip, &(pitch, len)) in pitches.iter().enumerate() {
                layout
                    .detile(&tiled, mip as u32, 0, &mut linear[at..at + len], pitch, len)
                    .expect("detiled");
                at += len;
            }
            d.push(t.elapsed().as_secs_f64() * 1e3);
            std::hint::black_box(&linear);
        }
        let (c, h, d) = (median(c), median(h), median(d));
        let mb = size as f64 / (1 << 20) as f64;
        let today = h + d;
        println!(
            "{name:<17} {mb:>8.1} | {c:>8.2} {h:>8.2} {d:>8.2} | {today:>9.2} ms {c:>9.2} ms | {:>6.1}x",
            today / c
        );
        today_per_mb += today;
        copy_per_mb += c;
        mbs += mb;
    }
    println!(
        "RESULT held_today_ms_per_mb={:.3} held_copy_ms_per_mb={:.3}",
        today_per_mb / mbs,
        copy_per_mb / mbs
    );
}
