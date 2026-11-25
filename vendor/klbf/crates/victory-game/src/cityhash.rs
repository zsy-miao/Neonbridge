//! CityHash64 (v1.1), the hash the engine keys its compact pipeline states
//! by (and the shader pack stores).

const K0: u64 = 0xC3A5_C85C_97CB_3127;
const K1: u64 = 0xB492_B66F_BE98_F273;
const K2: u64 = 0x9AE1_6A3B_2F90_404F;
const KMUL: u64 = 0x9DDF_EA08_EB38_2D69;

fn f64(s: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(s[i..i + 8].try_into().expect("8 bytes"))
}

fn f32(s: &[u8], i: usize) -> u64 {
    u64::from(u32::from_le_bytes(s[i..i + 4].try_into().expect("4 bytes")))
}

fn smix(v: u64) -> u64 {
    v ^ (v >> 47)
}

fn len16(u: u64, v: u64, mul: u64) -> u64 {
    let a = smix((u ^ v).wrapping_mul(mul));
    let b = smix((v ^ a).wrapping_mul(mul));
    b.wrapping_mul(mul)
}

fn to16(s: &[u8]) -> u64 {
    let n = s.len();
    let mul = K2.wrapping_add(n as u64 * 2);
    if n >= 8 {
        let a = f64(s, 0).wrapping_add(K2);
        let b = f64(s, n - 8);
        let c = b.rotate_right(37).wrapping_mul(mul).wrapping_add(a);
        let d = a.rotate_right(25).wrapping_add(b).wrapping_mul(mul);
        return len16(c, d, mul);
    }
    if n >= 4 {
        return len16((n as u64).wrapping_add(f32(s, 0) << 3), f32(s, n - 4), mul);
    }
    if n > 0 {
        let y = u64::from(s[0]) + (u64::from(s[n >> 1]) << 8);
        let z = n as u64 + (u64::from(s[n - 1]) << 2);
        return smix(y.wrapping_mul(K2) ^ z.wrapping_mul(K0)).wrapping_mul(K2);
    }
    K2
}

fn to32(s: &[u8]) -> u64 {
    let n = s.len();
    let mul = K2.wrapping_add(n as u64 * 2);
    let a = f64(s, 0).wrapping_mul(K1);
    let b = f64(s, 8);
    let c = f64(s, n - 8).wrapping_mul(mul);
    let d = f64(s, n - 16).wrapping_mul(K2);
    len16(
        a.wrapping_add(b)
            .rotate_right(43)
            .wrapping_add(c.rotate_right(30))
            .wrapping_add(d),
        a.wrapping_add(b.wrapping_add(K2).rotate_right(18))
            .wrapping_add(c),
        mul,
    )
}

fn to64(s: &[u8]) -> u64 {
    let n = s.len();
    let mul = K2.wrapping_add(n as u64 * 2);
    let a = f64(s, 0).wrapping_mul(K2);
    let b = f64(s, 8);
    let c = f64(s, n - 24);
    let d = f64(s, n - 32);
    let e = f64(s, 16).wrapping_mul(K2);
    let f = f64(s, 24).wrapping_mul(9);
    let g = f64(s, n - 8);
    let h = f64(s, n - 16).wrapping_mul(mul);
    let u = a
        .wrapping_add(g)
        .rotate_right(43)
        .wrapping_add(b.rotate_right(30).wrapping_add(c).wrapping_mul(9));
    let v = (a.wrapping_add(g) ^ d).wrapping_add(f).wrapping_add(1);
    let w = u
        .wrapping_add(v)
        .wrapping_mul(mul)
        .swap_bytes()
        .wrapping_add(h);
    let x = e.wrapping_add(f).rotate_right(42).wrapping_add(c);
    let y = v
        .wrapping_add(w)
        .wrapping_mul(mul)
        .swap_bytes()
        .wrapping_add(g)
        .wrapping_mul(mul);
    let z = e.wrapping_add(f).wrapping_add(c);
    let a = x
        .wrapping_add(z)
        .wrapping_mul(mul)
        .wrapping_add(y)
        .swap_bytes()
        .wrapping_add(b);
    let b = smix(
        z.wrapping_add(a)
            .wrapping_mul(mul)
            .wrapping_add(d)
            .wrapping_add(h),
    )
    .wrapping_mul(mul);
    b.wrapping_add(x)
}

fn weak(s: &[u8], i: usize, a: u64, b: u64) -> (u64, u64) {
    let (w, x, y, z) = (f64(s, i), f64(s, i + 8), f64(s, i + 16), f64(s, i + 24));
    let a = a.wrapping_add(w);
    let b = b.wrapping_add(a).wrapping_add(z).rotate_right(21);
    let c = a;
    let a = a.wrapping_add(x).wrapping_add(y);
    let b = b.wrapping_add(a.rotate_right(44));
    (a.wrapping_add(z), b.wrapping_add(c))
}

pub fn city64(s: &[u8]) -> u64 {
    let n = s.len();
    match n {
        0..=16 => return to16(s),
        17..=32 => return to32(s),
        33..=64 => return to64(s),
        _ => {}
    }
    let mut x = f64(s, n - 40);
    let mut y = f64(s, n - 16).wrapping_add(f64(s, n - 56));
    let mut z = len16(f64(s, n - 48).wrapping_add(n as u64), f64(s, n - 24), KMUL);
    let mut v = weak(s, n - 64, n as u64, z);
    let mut w = weak(s, n - 32, y.wrapping_add(K1), x);
    x = x.wrapping_mul(K1).wrapping_add(f64(s, 0));
    let mut i = 0;
    let mut left = (n - 1) & !63;
    loop {
        x = x
            .wrapping_add(y)
            .wrapping_add(v.0)
            .wrapping_add(f64(s, i + 8))
            .rotate_right(37)
            .wrapping_mul(K1);
        y = y
            .wrapping_add(v.1)
            .wrapping_add(f64(s, i + 48))
            .rotate_right(42)
            .wrapping_mul(K1);
        x ^= w.1;
        y = y.wrapping_add(v.0).wrapping_add(f64(s, i + 40));
        z = z.wrapping_add(w.0).rotate_right(33).wrapping_mul(K1);
        v = weak(s, i, v.1.wrapping_mul(K1), x.wrapping_add(w.0));
        w = weak(
            s,
            i + 32,
            z.wrapping_add(w.1),
            y.wrapping_add(f64(s, i + 16)),
        );
        std::mem::swap(&mut z, &mut x);
        i += 64;
        left -= 64;
        if left == 0 {
            break;
        }
    }
    len16(
        len16(v.0, w.0, KMUL)
            .wrapping_add(smix(y).wrapping_mul(K1))
            .wrapping_add(z),
        len16(v.1, w.1, KMUL).wrapping_add(x),
        KMUL,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values of the reference implementation for each length class.
    #[test]
    fn matches_the_reference() {
        let text = |n: usize| (0..n).map(|i| (i * 7 + 3) as u8).collect::<Vec<_>>();
        for (n, want) in REFERENCE {
            assert_eq!(city64(&text(n)), want, "length {n}");
        }
    }

    const REFERENCE: [(usize, u64); 13] = [
        (0, 0x9ae16a3b2f90404f),
        (3, 0x40b4412e33eefd64),
        (5, 0x156112f7c93e5cb3),
        (8, 0x0c97ecbc1e9d2626),
        (12, 0x07e11fa045e761bc),
        (16, 0x7eda0ed8c837a168),
        (24, 0x310064a8de2c42d2),
        (32, 0xa53f806f23a314e8),
        (40, 0x9ff6339213d1f513),
        (64, 0x1ccefea99389ae75),
        (68, 0x6ae6691c896ee22b),
        (130, 0x6c64b6a998c050f9),
        (200, 0x6099c264b552b496),
    ];
}
