//! Run with `cargo run --release -p victory-formats --example texture_layout_bench`.

use std::hint::black_box;
use std::time::Instant;

use victory_formats::texture::{Dimension, Swizzle, Texture, TextureDesc};

fn main() {
    const CALLS: u64 = 50_000;
    let base = TextureDesc {
        dimension: Dimension::Tex2d,
        width: 512,
        height: 512,
        depth_or_array: 1,
        mips: 0,
        format: 98, // BC7
        samples: 1,
        swizzle: Swizzle::SW_64KB_D,
    };
    let cases = [
        ("BC7 64KB_D", base),
        (
            "BC1 64KB_S",
            TextureDesc {
                format: 71,
                swizzle: Swizzle::SW_64KB_S,
                ..base
            },
        ),
        (
            "RGBA8 4KB_D",
            TextureDesc {
                format: 28,
                swizzle: Swizzle::SW_4KB_D,
                ..base
            },
        ),
        (
            "R8 64KB_S",
            TextureDesc {
                format: 61,
                swizzle: Swizzle::SW_64KB_S,
                ..base
            },
        ),
        (
            "RGBA8 volume 64KB_S",
            TextureDesc {
                dimension: Dimension::Tex3d,
                width: 128,
                height: 128,
                depth_or_array: 64,
                format: 28,
                swizzle: Swizzle::SW_64KB_S,
                ..base
            },
        ),
        (
            "RGBA8 linear",
            TextureDesc {
                format: 28,
                swizzle: Swizzle::LINEAR,
                ..base
            },
        ),
    ];

    for (name, desc) in cases {
        let mut samples = [0.0; 7];
        black_box(Texture::new(black_box(&desc)).unwrap());
        let mut checksum = 0u64;
        for sample in &mut samples {
            let start = Instant::now();
            for i in 0..CALLS {
                let desc = TextureDesc {
                    width: desc.width + (i % 32) as u32,
                    ..desc
                };
                let texture = black_box(Texture::new(black_box(&desc)).unwrap());
                let layout = &texture.planes[0].layout;
                checksum =
                    checksum.wrapping_add(texture.size + layout.offset(0, 7, 11, 0).unwrap());
            }
            *sample = start.elapsed().as_secs_f64() * 1e9 / CALLS as f64;
        }
        samples.sort_by(f64::total_cmp);
        println!("{name:22} {:8.1} ns/layout  {checksum}", samples[3]);
    }
}
