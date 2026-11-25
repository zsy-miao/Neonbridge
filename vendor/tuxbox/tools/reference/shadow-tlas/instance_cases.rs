//! Independent CPU fixture for the GPU instance encoder.
#[path = "../blas/shadow_instance.rs"]
mod shadow_instance;
use shadow_instance::{Blas, Bounds, Instance, build};

fn main() {
    let cases = [
        (
            [[1., 0., 0., 4.], [0., 1., 0., -2.], [0., 0., 1., 7.]],
            0xff001234,
            0x12345600,
        ),
        (
            [[0., -2., 0.5, 512.], [0.5, 0., 0.25, 0.], [0., 0., 1.5, 0.]],
            0xff000008,
            0x12345600,
        ),
        (
            [[2., 1., 0., 1.], [0., 3., 0., 2.], [0., 0., 4., 3.]],
            0x10000001,
            0x12345600,
        ),
        (
            [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
            0x00000009,
            0x12345600,
        ),
        (
            [[1., 0., 0., 0.], [0., 0., 0., 0.], [0., 0., 1., 0.]],
            0xff000009,
            0x12345600,
        ),
        (
            [[-1., 0., 0., 100.], [0., 1., 0., -200.], [0., 0., 1., 300.]],
            0xff00000a,
            0x12345600,
        ),
        (
            [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
            0xff00000b,
            0,
        ),
        (
            [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
            0xff00000c,
            0x12345601,
        ),
        (
            [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
            0xff00000d,
            0x10000000000,
        ),
        (
            [[1., 2., 3., -11.], [0., 4., 5., 13.], [0., 0., 6., -17.]],
            0x8000000e,
            0x12345600,
        ),
    ];
    let mut cases = cases.to_vec();
    let tiny = f32::MIN_POSITIVE;
    cases.extend([
        (
            [[1., tiny, 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
            0xff000010,
            0x12345600,
        ),
        (
            [[1., -tiny, 0., -1.], [0., 1., tiny, 1.], [0., 0., 1., 0.]],
            0xff000011,
            0x12345600,
        ),
        (
            [[1., 1., 0., 2.], [0., 1., -1., -2.], [0., 0., 1., 0.]],
            0xff000012,
            0x12345600,
        ),
        (
            [
                [1., 0., 0., 16777216.],
                [0., 1., 0., -16777216.],
                [0., 0., 1., 0.],
            ],
            0xff000013,
            0x12345600,
        ),
    ]);
    let mut state = 0x98765432u32;
    let mut next = || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        state
    };
    for _ in 0..32 {
        let mut matrix = [[0.0f32; 4]; 3];
        for (row, values) in matrix.iter_mut().enumerate() {
            for (col, value) in values[..3].iter_mut().enumerate() {
                *value = if row == col {
                    2.0 + (next() % 12) as f32 * 0.25
                } else {
                    (next() % 9) as f32 * 0.125 - 0.5
                };
            }
            values[3] = (next() % 2001) as f32 - 1000.0;
        }
        cases.push((matrix, 0xff000001, 0x12345600));
    }
    let blas = Blas {
        bounds: Bounds {
            min: [-1.; 3],
            max: [1.; 3],
        },
        root: 0xa94,
        compression: 0x007f7f7f,
    };
    for (index, (transform, id_and_mask, address)) in cases.into_iter().enumerate() {
        let instance = Instance {
            transform,
            id_and_mask,
            contribution_and_flags: 0x01000002,
            blas_address: address,
        };
        let mut input = [0u32; 16];
        for (dst, f) in input[..12].iter_mut().zip(transform.into_iter().flatten()) {
            *dst = f.to_bits();
        }
        input[12] = id_and_mask;
        input[13] = instance.contribution_and_flags;
        input[14] = address as u32;
        input[15] = (address >> 32) as u32;
        let (status, output, bounds) = match build(instance, blas, index as u32, 0x33) {
            Ok(Some(value)) => (
                1,
                value.words,
                [value.bounds.min, value.bounds.max].concat(),
            ),
            Ok(None) => (0, [0; 32], vec![0.; 6]),
            Err(_) => (2, [0; 32], vec![0.; 6]),
        };
        for n in input {
            print!("{n:08x} ");
        }
        print!("{status:08x} ");
        for n in output {
            print!("{n:08x} ");
        }
        for v in bounds {
            print!("{:08x} ", v.to_bits());
        }
        println!();
    }
}
