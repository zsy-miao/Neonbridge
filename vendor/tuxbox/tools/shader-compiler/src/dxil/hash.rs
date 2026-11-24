//! The digest D3D12 checks in a DXBC container (bytes 4..20, over everything
//! from byte 20): MD5 rounds whose last block carries the bit count first and
//! `2 * bytes + 1` last instead of MD5's padding (DxilContainerHash).
//!
//! Ported from five-star-recomp (GPL-3.0-or-later, ThatRecompDev and the Five
//! Star Recomp contributors), src/FiveStar.Shaders/Dxil/Container/ContainerHash.cs.

const SHIFTS: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
const INITIAL: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];

fn sines() -> [u32; 64] {
    let mut out = [0u32; 64];
    for (i, s) in out.iter_mut().enumerate() {
        *s = (((i + 1) as f64).sin().abs() * 4_294_967_296.0) as u32;
    }
    out
}

/// The digest of `data` (the container from byte 20 on).
pub fn compute(data: &[u8]) -> [u8; 16] {
    let length = data.len();
    let left = length % 64;
    let bit_count = (length as u32).wrapping_mul(8);
    let mut tail = Vec::with_capacity(128);
    if left < 56 {
        tail.extend_from_slice(&bit_count.to_le_bytes());
        tail.extend_from_slice(&data[length - left..]);
        tail.push(0x80);
        tail.resize(60, 0);
    } else {
        tail.extend_from_slice(&data[length - left..]);
        tail.push(0x80);
        tail.resize(64, 0);
        tail.extend_from_slice(&bit_count.to_le_bytes());
        tail.resize(124, 0);
    }
    tail.extend_from_slice(&(((length as u32) << 1) | 1).to_le_bytes());
    let k = sines();
    let mut state = INITIAL;
    for block in data[..length - left].chunks_exact(64) {
        round(&mut state, block, &k);
    }
    for block in tail.chunks_exact(64) {
        round(&mut state, block, &k);
    }
    let mut digest = [0u8; 16];
    for (i, s) in state.iter().enumerate() {
        digest[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
    }
    digest
}

fn round(state: &mut [u32; 4], block: &[u8], k: &[u32; 64]) {
    let mut w = [0u32; 16];
    for (i, word) in w.iter_mut().enumerate() {
        *word = u32::from_le_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
    }
    let [mut a, mut b, mut c, mut d] = *state;
    for r in 0..64 {
        let (mix, word) = match r / 16 {
            0 => ((b & c) | (!b & d), r),
            1 => ((d & b) | (!d & c), (5 * r + 1) % 16),
            2 => (b ^ c ^ d, (3 * r + 5) % 16),
            _ => (c ^ (b | !d), (7 * r) % 16),
        };
        let shift = SHIFTS[(r / 16) * 4 + r % 4];
        let rotated = a.wrapping_add(mix).wrapping_add(k[r]).wrapping_add(w[word]).rotate_left(shift);
        (a, b, c, d) = (d, b.wrapping_add(rotated), b, c);
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

/// Writes the digest into a finished container (bytes 4..20).
pub fn seal(container: &mut [u8]) {
    let digest = compute(&container[20..]);
    container[4..20].copy_from_slice(&digest);
}
