//! CPU reference for constructing one uncompressed Xbox BVH4 instance record.
//!
//! This is a format and mathematics control, not a runtime TLAS builder. It
//! deliberately accepts only unscaled BLAS metadata and 40-bit GPU addresses.
//! Layout is documented by the public XDXR standalone traversal declarations.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct Instance {
    pub transform: [[f32; 4]; 3],
    pub id_and_mask: u32,
    pub contribution_and_flags: u32,
    pub blas_address: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Blas {
    pub bounds: Bounds,
    pub root: u32,
    pub compression: u32,
}

#[derive(Debug, PartialEq)]
pub enum Error {
    Address,
    Root,
    Compression,
    Bounds,
    Transform,
    Singular,
}

#[derive(Debug)]
pub struct Record {
    pub words: [u32; 32],
    pub bounds: Bounds,
}

/// Produces a reference record and conservative world bounds for a single
/// instance. Zero mask or zero BLAS address has no visible geometry.
pub fn build(
    instance: Instance,
    blas: Blas,
    index: u32,
    parent: u32,
) -> Result<Option<Record>, Error> {
    if instance.id_and_mask >> 24 == 0 || instance.blas_address == 0 {
        return Ok(None);
    }
    if instance.blas_address & 255 != 0 || instance.blas_address >> 40 != 0 {
        return Err(Error::Address);
    }
    // TRI0 and BOX16/BOX32 are the decoded BLAS roots covered here.
    // Triangle types 1–3, procedural roots and the sentinel need validation.
    if blas.root >= (1 << 29) || !matches!(blas.root & 7, 0 | 4 | 5) {
        return Err(Error::Root);
    }
    if blas.compression != 0x007f_7f7f {
        return Err(Error::Compression);
    }
    if !(0..3).all(|i| {
        blas.bounds.min[i].is_finite()
            && blas.bounds.max[i].is_finite()
            && blas.bounds.min[i] <= blas.bounds.max[i]
    }) {
        return Err(Error::Bounds);
    }
    if instance.transform.iter().flatten().any(|v| !v.is_finite()) {
        return Err(Error::Transform);
    }
    let m = instance.transform.map(|row| row.map(f64::from));
    let (a, b, c) = (m[0][0], m[0][1], m[0][2]);
    let (d, e, f) = (m[1][0], m[1][1], m[1][2]);
    let (g, h, i) = (m[2][0], m[2][1], m[2][2]);
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det == 0.0 || !det.is_finite() {
        return Err(Error::Singular);
    }
    let inverse = [
        [
            (e * i - f * h) / det,
            (c * h - b * i) / det,
            (b * f - c * e) / det,
        ],
        [
            (f * g - d * i) / det,
            (a * i - c * g) / det,
            (c * d - a * f) / det,
        ],
        [
            (d * h - e * g) / det,
            (b * g - a * h) / det,
            (a * e - b * d) / det,
        ],
    ];
    let mut world_to_object = [[0f32; 4]; 3];
    for row in 0..3 {
        for col in 0..3 {
            world_to_object[row][col] = inverse[row][col] as f32;
        }
        world_to_object[row][3] =
            -(0..3).map(|col| inverse[row][col] * m[col][3]).sum::<f64>() as f32;
    }
    if world_to_object.iter().flatten().any(|v| !v.is_finite()) {
        return Err(Error::Transform);
    }
    let mut lower = [f64::INFINITY; 3];
    let mut upper = [f64::NEG_INFINITY; 3];
    for corner in 0..8 {
        let p: [f64; 3] = std::array::from_fn(|axis| {
            f64::from(if corner & (1 << axis) == 0 {
                blas.bounds.min[axis]
            } else {
                blas.bounds.max[axis]
            })
        });
        for axis in 0..3 {
            let mut lo = m[axis][3];
            let mut hi = lo;
            for (coefficient, coordinate) in m[axis][..3].iter().zip(p) {
                // Products of two finite f32 values are exact in f64. Round
                // each sum outwards to cover cancellation in the dot product.
                let term = coefficient * coordinate;
                lo = (lo + term).next_down();
                hi = (hi + term).next_up();
            }
            lower[axis] = lower[axis].min(lo);
            upper[axis] = upper[axis].max(hi);
        }
    }
    fn outward(value: f64, lower: bool) -> f32 {
        let rounded = value as f32;
        if lower && f64::from(rounded) > value {
            rounded.next_down()
        } else if !lower && f64::from(rounded) < value {
            rounded.next_up()
        } else {
            rounded
        }
    }
    let bounds = Bounds {
        min: lower.map(|v| outward(v, true)),
        max: upper.map(|v| outward(v, false)),
    };
    if bounds.min.iter().chain(&bounds.max).any(|v| !v.is_finite()) {
        return Err(Error::Bounds);
    }
    let mut words = [0u32; 32];
    for (dst, value) in words[..12]
        .iter_mut()
        .zip(instance.transform.into_iter().flatten())
    {
        *dst = value.to_bits();
    }
    words[12] = instance.id_and_mask;
    words[13] = instance.contribution_and_flags;
    let identity = (0..3).all(|row| {
        (0..3).all(|col| instance.transform[row][col] == if row == col { 1.0 } else { 0.0 })
    });
    words[14] = blas.root | if identity { 0x8000_0000 } else { 0 };
    words[15] = (instance.blas_address >> 8) as u32;
    for row in 0..3 {
        words[16 + row] = world_to_object[row][3].to_bits();
        for col in 0..3 {
            words[19 + row * 3 + col] = world_to_object[row][col].to_bits();
        }
    }
    words[28] = index;
    words[29] = parent;
    words[30] = blas.compression;
    Ok(Some(Record { words, bounds }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(transform: [[f32; 4]; 3]) -> Instance {
        Instance {
            transform,
            id_and_mask: 0x7f12_3456,
            contribution_and_flags: 0x0100_0002,
            blas_address: 0x1234_5600,
        }
    }

    fn blas() -> Blas {
        Blas {
            bounds: Bounds {
                min: [-1., -2., -3.],
                max: [2., 3., 4.],
            },
            root: 0x25,
            compression: 0x007f_7f7f,
        }
    }

    fn identity() -> [[f32; 4]; 3] {
        [[1., 0., 0., 4.], [0., 1., 0., -3.], [0., 0., 1., 2.]]
    }

    #[test]
    fn record_layout_translation_and_bounds() {
        let record = build(instance(identity()), blas(), 12, 0xffff_ffff)
            .unwrap()
            .unwrap();
        for (axis, (min, max)) in [(3., 6.), (-5., 0.), (-1., 6.)].into_iter().enumerate() {
            assert!(record.bounds.min[axis] <= min && record.bounds.max[axis] >= max);
            assert!((record.bounds.min[axis] - min).abs() < 1e-5);
            assert!((record.bounds.max[axis] - max).abs() < 1e-5);
        }
        assert_eq!(
            &record.words[12..16],
            &[0x7f12_3456, 0x0100_0002, 0x8000_0025, 0x12_3456]
        );
        assert_eq!(
            <[u32; 12]>::try_from(&record.words[16..28])
                .unwrap()
                .map(f32::from_bits),
            [-4., 3., -2., 1., 0., 0., 0., 1., 0., 0., 0., 1.]
        );
        assert_eq!(&record.words[28..], &[12, u32::MAX, 0x007f_7f7f, 0]);
    }

    #[test]
    fn rotated_scaled_sheared_instance_inverts_points_and_encloses_corners() {
        let transform = [[0., -2., 0.5, 10.], [1., 0., 0.25, -3.], [0., 0., -1., 7.]];
        let record = build(instance(transform), blas(), 0, 0).unwrap().unwrap();
        assert_eq!(record.words[14], 0x25);
        let inverse: Vec<_> = record.words[16..28]
            .iter()
            .map(|&w| f32::from_bits(w))
            .collect();
        for bits in 0..8 {
            let p: [f32; 3] = std::array::from_fn(|axis| {
                if bits & (1 << axis) == 0 {
                    blas().bounds.min[axis]
                } else {
                    blas().bounds.max[axis]
                }
            });
            let world: [f32; 3] = std::array::from_fn(|row| {
                transform[row][3] + (0..3).map(|col| transform[row][col] * p[col]).sum::<f32>()
            });
            for row in 0..3 {
                assert!(
                    record.bounds.min[row] <= world[row] && world[row] <= record.bounds.max[row]
                );
                let original = inverse[row]
                    + (0..3)
                        .map(|col| inverse[3 + row * 3 + col] * world[col])
                        .sum::<f32>();
                assert!((original - p[row]).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn disabled_and_invalid_instances() {
        let mut input = instance(identity());
        input.id_and_mask &= 0x00ff_ffff;
        assert!(build(input, blas(), 0, 0).unwrap().is_none());
        input.id_and_mask |= 0xff00_0000;
        input.blas_address = 0;
        assert!(build(input, blas(), 0, 0).unwrap().is_none());
        input.blas_address = 0x1234_5601;
        assert!(matches!(build(input, blas(), 0, 0), Err(Error::Address)));
        input.blas_address = 1 << 40;
        assert!(matches!(build(input, blas(), 0, 0), Err(Error::Address)));
        input.blas_address = 0x1234_5600;
        input.transform[0] = input.transform[1];
        assert!(matches!(build(input, blas(), 0, 0), Err(Error::Singular)));
        input.transform = identity();
        input.transform[0][0] = f32::NAN;
        assert!(matches!(build(input, blas(), 0, 0), Err(Error::Transform)));
        input.transform = identity();
        let mut invalid = blas();
        invalid.bounds.min[0] = 4.;
        assert!(matches!(build(input, invalid, 0, 0), Err(Error::Bounds)));
        invalid = blas();
        invalid.compression = 0;
        assert!(matches!(
            build(input, invalid, 0, 0),
            Err(Error::Compression)
        ));
        invalid = blas();
        invalid.root = 1;
        assert!(matches!(build(input, invalid, 0, 0), Err(Error::Root)));
        invalid.root = 0x3fff_ffff;
        assert!(matches!(build(input, invalid, 0, 0), Err(Error::Root)));
    }

    #[test]
    fn affine_records_enclose_all_corners_and_recover_points() {
        let mut random = 0x8c7a_12d5u32;
        let mut next = || {
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((random >> 16) as i32 - 32_768) as f32 / 8_192.0
        };
        let mut checked = 0;
        for _ in 0..800 {
            let transform = [
                [next(), next(), next(), next()],
                [next(), next(), next(), next()],
                [next(), next(), next(), next()],
            ];
            let matrix = transform.map(|row| row.map(f64::from));
            let determinant = matrix[0][0]
                * (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1])
                - matrix[0][1] * (matrix[1][0] * matrix[2][2] - matrix[1][2] * matrix[2][0])
                + matrix[0][2] * (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0]);
            if determinant.abs() < 0.25 {
                continue;
            }
            let record = build(instance(transform), blas(), 0, 0).unwrap().unwrap();
            let inverse: Vec<f64> = record.words[16..28]
                .iter()
                .map(|&word| f64::from(f32::from_bits(word)))
                .collect();
            for corner in 0..8 {
                let point: [f64; 3] = std::array::from_fn(|axis| {
                    f64::from(if corner & (1 << axis) == 0 {
                        blas().bounds.min[axis]
                    } else {
                        blas().bounds.max[axis]
                    })
                });
                let world: [f64; 3] = std::array::from_fn(|row| {
                    matrix[row][3] + (0..3).map(|col| matrix[row][col] * point[col]).sum::<f64>()
                });
                for row in 0..3 {
                    assert!(f64::from(record.bounds.min[row]) <= world[row]);
                    assert!(world[row] <= f64::from(record.bounds.max[row]));
                    let recovered = inverse[row]
                        + (0..3)
                            .map(|col| inverse[3 + row * 3 + col] * world[col])
                            .sum::<f64>();
                    assert!((recovered - point[row]).abs() < 2e-4);
                }
            }
            checked += 1;
        }
        assert!(checked > 700);
    }
}
