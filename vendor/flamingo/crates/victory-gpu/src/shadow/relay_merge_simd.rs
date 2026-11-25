//! CPU-side relay merge. Keep the reference snapshot separate from
//! current title memory: CPU-only writes must not enter the snapshot.

pub(super) fn merge(now: &[u8], held: &mut [u8], title: &mut [u8]) -> bool {
    // Preserve the original zip semantics for differently sized slices.
    let len = now.len().min(held.len()).min(title.len());
    let (now, held, title) = (&now[..len], &mut held[..len], &mut title[..len]);
    #[cfg(target_arch = "x86_64")]
    if len >= 32 && std::arch::is_x86_feature_detected!("avx2") {
        // SAFETY: runtime AVX2 detection; all slices have equal length.
        return unsafe { avx2(now, held, title) };
    }
    scalar(now, held, title)
}

fn scalar(now: &[u8], held: &mut [u8], title: &mut [u8]) -> bool {
    let mut differs = 0u8;
    for ((n, h), t) in now.iter().zip(held.iter_mut()).zip(title.iter_mut()) {
        let wrote = if *n != *h { 0xffu8 } else { 0 };
        let merged = (*t & !wrote) | (*n & wrote);
        differs |= merged ^ *t;
        *t = merged;
        *h = *n;
    }
    differs != 0
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2(now: &[u8], held: &mut [u8], title: &mut [u8]) -> bool {
    use std::arch::x86_64::*;

    let end = now.len() / 32 * 32;
    let mut differs = _mm256_setzero_si256();
    for at in (0..end).step_by(32) {
        // SAFETY: all three equally sized slices have 32 accessible bytes
        // at this offset. Loads and stores permit unaligned addresses.
        unsafe {
            let n = _mm256_loadu_si256(now.as_ptr().add(at).cast());
            let h = _mm256_loadu_si256(held.as_ptr().add(at).cast());
            let t = _mm256_loadu_si256(title.as_ptr().add(at).cast());
            let merged = _mm256_blendv_epi8(n, t, _mm256_cmpeq_epi8(n, h));
            differs = _mm256_or_si256(differs, _mm256_xor_si256(merged, t));
            _mm256_storeu_si256(title.as_mut_ptr().add(at).cast(), merged);
            _mm256_storeu_si256(held.as_mut_ptr().add(at).cast(), n);
        }
    }
    let tail = scalar(&now[end..], &mut held[end..], &mut title[end..]);
    _mm256_testz_si256(differs, differs) == 0 || tail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_only_changes_do_not_enter_the_reference() {
        let now = [5; 65];
        let mut held = [5; 65];
        let mut title = [9; 65];
        assert!(!merge(&now, &mut held, &mut title));
        assert_eq!(held, [5; 65]);
        assert_eq!(title, [9; 65]);
    }

    #[test]
    fn mismatched_lengths_keep_original_zip_behavior() {
        let now = [7; 70];
        let mut held = [5; 80];
        let mut title = [9; 67];
        assert!(merge(&now, &mut held, &mut title));
        assert_eq!(&held[..67], &[7; 67]);
        assert_eq!(&held[67..], &[5; 13]);
        assert_eq!(title, [7; 67]);
    }

    #[test]
    fn all_alignments_and_tails_match_scalar() {
        let mut seed = 0x1234_abcd_u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        };
        for len in (0..=129).chain([255, 256, 257, 4095, 4096, 4097, 65535, 65536]) {
            for offset in 0..32 {
                let (n, h, t) = (offset, offset * 7 % 32, offset * 13 % 32);
                let mut now = vec![0x11; len + 64];
                let mut held = vec![0x22; len + 64];
                let mut title = vec![0x33; len + 64];
                for i in 0..len {
                    now[n + i] = next();
                    held[h + i] = if i % 3 == 0 { now[n + i] } else { next() };
                    title[t + i] = if i % 3 == 1 { now[n + i] } else { next() };
                }
                let mut expected_held = held.clone();
                let mut expected_title = title.clone();
                let expected = scalar(
                    &now[n..n + len],
                    &mut expected_held[h..h + len],
                    &mut expected_title[t..t + len],
                );
                assert_eq!(
                    merge(&now[n..n + len], &mut held[h..h + len], &mut title[t..t + len]),
                    expected
                );
                assert_eq!(held, expected_held);
                assert_eq!(title, expected_title);
            }
        }
    }
}
