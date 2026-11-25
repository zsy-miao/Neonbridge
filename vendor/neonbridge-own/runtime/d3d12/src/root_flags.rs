//! The root-signature exports clear the Xbox high byte of the root-signature
//! flags before the desktop runtime sees them.
//!
//! Ported from leonida (GPL-3.0, credited in docs/DETAILS.md):
//! runtime/shims/d3d12_xs.cpp and d3d12_xs_rootsig.cpp, a5bb4b9b (tedin7,
//! versioned deserializer), 3ff887f9 (d7ommy, all-zero checksums kept) and the
//! fail-closed chunk table of 5c3487a94 (mynzjerko). Console root signatures
//! may carry flags in bits 24..31 that desktop D3D12 rejects with E_INVALIDARG.
//! The serializers get a copy of the description with that byte cleared; the
//! deserializers get a copy of an RTS0 container with it cleared and the DXBC
//! checksum recomputed (an all-zero checksum is not checked by d3d12.dll and is
//! kept). FidelityFX's upscaler imports D3D12CreateRootSignatureDeserializer
//! and the title D3D12SerializeVersionedRootSignature; before this all four
//! went to d3d12.dll unchanged. TUXBOX_ROOT_FLAGS=0 forwards them unchanged.

use core::ffi::c_void;
use std::sync::OnceLock;
use windows::Win32::Foundation::E_NOTIMPL;
use windows::core::{GUID, HRESULT};

const XBOX_FLAGS: u32 = 0xff00_0000;

fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_ROOT_FLAGS") != Some("0")
}

fn host(name: &'static [u8]) -> *const c_void {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
    use windows::core::PCSTR;
    // SAFETY: both strings are NUL-terminated; d3d12.dll stays loaded.
    unsafe {
        let Ok(module) = LoadLibraryA(PCSTR(b"d3d12.dll\0".as_ptr())) else { return core::ptr::null() };
        GetProcAddress(module, PCSTR(name.as_ptr())).map_or(core::ptr::null(), |f| f as *const c_void)
    }
}

fn noted(what: &str, flags: u32) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&ONCE, || format!(
        "{what}: root-signature flags {flags:#x} carry the Xbox high byte; cleared for the desktop runtime (leonida a5bb4b9b; TUXBOX_ROOT_FLAGS=0 forwards)"));
}

/// The d3d12.dll container hash: MD5 over bytes 20.. with the DXBC tail (the
/// bit count first in the last block and (bits >> 2) | 1 last).
fn dxbc_checksum(blob: &[u8]) -> [u8; 16] {
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
        0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
        0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
        0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
        0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
    ];
    const R: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
    fn block(state: &mut [u32; 4], m: &[u32; 16]) {
        let [mut a, mut b, mut c, mut d] = *state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let t = d;
            d = c;
            c = b;
            b = b.wrapping_add(a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]).rotate_left(R[i / 16 * 4 + i % 4]));
            a = t;
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }
    fn words(bytes: &[u8; 64]) -> [u32; 16] {
        core::array::from_fn(|i| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap()))
    }
    let data = &blob[20..];
    let bits = (data.len() as u32).wrapping_mul(8);
    let mut state = [0x67452301u32, 0xefcdab89, 0x98badcfe, 0x10325476];
    let full = data.len() / 64 * 64;
    for chunk in data[..full].chunks_exact(64) {
        block(&mut state, &words(chunk.try_into().unwrap()));
    }
    let left = &data[full..];
    let mut last = [0u8; 64];
    let mut m;
    if left.len() >= 56 {
        last[..left.len()].copy_from_slice(left);
        last[left.len()] = 0x80;
        block(&mut state, &words(&last));
        m = [0u32; 16];
    } else {
        last[4..4 + left.len()].copy_from_slice(left);
        last[4 + left.len()] = 0x80;
        m = words(&last);
    }
    m[0] = bits;
    m[15] = (bits >> 2) | 1;
    block(&mut state, &m);
    let mut out = [0u8; 16];
    for (i, s) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
    }
    out
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// A copy of a DXBC container whose RTS0 flags carry the Xbox byte, cleared and
/// re-hashed; None leaves the blob as it is (not DXBC, malformed, a bad
/// checksum the runtime should reject, or no Xbox byte).
fn cleared_container(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 36 || &bytes[..4] != b"DXBC" || u32_at(bytes, 24)? as usize != bytes.len() {
        return None;
    }
    let count = u32_at(bytes, 28)? as usize;
    if count == 0 || count > 16 || count * 4 > bytes.len() - 32 {
        return None;
    }
    let zero_hash = bytes[4..20].iter().all(|&b| b == 0);
    if !zero_hash && dxbc_checksum(bytes) != bytes[4..20] {
        return None;
    }
    let first = 32 + count * 4;
    let mut rts0 = None;
    for i in 0..count {
        let off = u32_at(bytes, 32 + i * 4)? as usize;
        if off < first || off > bytes.len() || bytes.len() - off < 8 {
            return None;
        }
        let size = u32_at(bytes, off + 4)? as usize;
        if size > bytes.len() - off - 8 {
            return None;
        }
        if rts0.is_none() && &bytes[off..off + 4] == b"RTS0" {
            if size < 24 {
                return None;
            }
            rts0 = Some(off + 8);
        }
    }
    let payload = rts0?;
    if !(1..=3).contains(&u32_at(bytes, payload)?) {
        return None;
    }
    let flags = u32_at(bytes, payload + 20)?;
    if flags & XBOX_FLAGS == 0 {
        return None;
    }
    noted("root-signature deserializer", flags);
    let mut out = bytes.to_vec();
    out[payload + 20..payload + 24].copy_from_slice(&(flags & !XBOX_FLAGS).to_le_bytes());
    if !zero_hash {
        let hash = dxbc_checksum(&out);
        out[4..20].copy_from_slice(&hash);
    }
    Some(out)
}

type Deserializer = unsafe extern "system" fn(*const c_void, usize, *const GUID, *mut *mut c_void) -> HRESULT;
type Serializer = unsafe extern "system" fn(*const c_void, u32, *mut *mut c_void, *mut *mut c_void) -> HRESULT;
type Versioned = unsafe extern "system" fn(*const c_void, *mut *mut c_void, *mut *mut c_void) -> HRESULT;

unsafe fn deserialize(name: &'static [u8], cell: &OnceLock<usize>, data: *const c_void, size: usize, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    let f = *cell.get_or_init(|| host(name) as usize);
    if f == 0 {
        return E_NOTIMPL;
    }
    // SAFETY: f is d3d12.dll's export of this name.
    let f: Deserializer = unsafe { core::mem::transmute(f) };
    if enabled() && !data.is_null() && size != 0 {
        // SAFETY: the caller passes `size` readable bytes, as the runtime requires.
        let bytes = unsafe { core::slice::from_raw_parts(data.cast::<u8>(), size) };
        if let Some(copy) = cleared_container(bytes) {
            return unsafe { f(copy.as_ptr().cast(), copy.len(), iid, out) };
        }
    }
    unsafe { f(data, size, iid, out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn tuxbox_create_root_signature_deserializer(data: *const c_void, size: usize, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    static F: OnceLock<usize> = OnceLock::new();
    unsafe { deserialize(b"D3D12CreateRootSignatureDeserializer\0", &F, data, size, iid, out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn tuxbox_create_versioned_root_signature_deserializer(data: *const c_void, size: usize, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    static F: OnceLock<usize> = OnceLock::new();
    unsafe { deserialize(b"D3D12CreateVersionedRootSignatureDeserializer\0", &F, data, size, iid, out) }
}

// D3D12_ROOT_SIGNATURE_DESC, DESC1 and DESC2 all hold Flags at byte 32 of a
// 40-byte struct; D3D12_VERSIONED_ROOT_SIGNATURE_DESC is a u32 version then
// that union at byte 8.
const DESC_BYTES: usize = 40;
const FLAGS_AT: usize = 32;

#[unsafe(no_mangle)]
pub unsafe extern "system" fn tuxbox_serialize_root_signature(desc: *const c_void, version: u32, blob: *mut *mut c_void, error: *mut *mut c_void) -> HRESULT {
    static F: OnceLock<usize> = OnceLock::new();
    let f = *F.get_or_init(|| host(b"D3D12SerializeRootSignature\0") as usize);
    if f == 0 {
        return E_NOTIMPL;
    }
    // SAFETY: f is d3d12.dll's export of this name.
    let f: Serializer = unsafe { core::mem::transmute(f) };
    if enabled() && !desc.is_null() {
        let mut copy = [0u64; DESC_BYTES / 8];
        // SAFETY: desc points at a D3D12_ROOT_SIGNATURE_DESC.
        unsafe { core::ptr::copy_nonoverlapping(desc.cast::<u8>(), copy.as_mut_ptr().cast::<u8>(), DESC_BYTES) };
        let flags = unsafe { copy.as_mut_ptr().cast::<u8>().add(FLAGS_AT).cast::<u32>() };
        if unsafe { *flags } & XBOX_FLAGS != 0 {
            noted("D3D12SerializeRootSignature", unsafe { *flags });
            unsafe { *flags &= !XBOX_FLAGS };
            return unsafe { f(copy.as_ptr().cast(), version, blob, error) };
        }
    }
    unsafe { f(desc, version, blob, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn tuxbox_serialize_versioned_root_signature(desc: *const c_void, blob: *mut *mut c_void, error: *mut *mut c_void) -> HRESULT {
    static F: OnceLock<usize> = OnceLock::new();
    let f = *F.get_or_init(|| host(b"D3D12SerializeVersionedRootSignature\0") as usize);
    if f == 0 {
        return E_NOTIMPL;
    }
    // SAFETY: f is d3d12.dll's export of this name.
    let f: Versioned = unsafe { core::mem::transmute(f) };
    if enabled() && !desc.is_null() {
        // SAFETY: desc points at a D3D12_VERSIONED_ROOT_SIGNATURE_DESC.
        let version = unsafe { *desc.cast::<u32>() };
        if (1..=3).contains(&version) {
            let mut copy = [0u64; (8 + DESC_BYTES) / 8];
            unsafe { core::ptr::copy_nonoverlapping(desc.cast::<u8>(), copy.as_mut_ptr().cast::<u8>(), 8 + DESC_BYTES) };
            let flags = unsafe { copy.as_mut_ptr().cast::<u8>().add(8 + FLAGS_AT).cast::<u32>() };
            if unsafe { *flags } & XBOX_FLAGS != 0 {
                noted("D3D12SerializeVersionedRootSignature", unsafe { *flags });
                unsafe { *flags &= !XBOX_FLAGS };
                return unsafe { f(copy.as_ptr().cast(), blob, error) };
            }
        }
    }
    unsafe { f(desc, blob, error) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-chunk RTS0 container with the given flags and a real or zero hash.
    fn container(flags: u32, hashed: bool) -> Vec<u8> {
        let mut payload = vec![0u8; 24];
        payload[..4].copy_from_slice(&2u32.to_le_bytes());
        payload[20..24].copy_from_slice(&flags.to_le_bytes());
        let mut b = Vec::new();
        b.extend_from_slice(b"DXBC");
        b.extend_from_slice(&[0; 16]);
        b.extend_from_slice(&1u32.to_le_bytes());
        let total = 32 + 4 + 8 + payload.len();
        b.extend_from_slice(&(total as u32).to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&36u32.to_le_bytes());
        b.extend_from_slice(b"RTS0");
        b.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        b.extend_from_slice(&payload);
        if hashed {
            let h = dxbc_checksum(&b);
            b[4..20].copy_from_slice(&h);
        }
        b
    }

    #[test]
    fn xbox_flags_are_cleared_and_the_hash_follows() {
        let original = container(0x0100_0011, true);
        let out = cleared_container(&original).expect("rewritten");
        assert_eq!(u32_at(&out, 36 + 8 + 20), Some(0x11));
        assert_eq!(dxbc_checksum(&out), out[4..20]);
    }

    #[test]
    fn zero_hashes_stay_zero_and_bad_hashes_are_left_alone() {
        let out = cleared_container(&container(0x0100_0000, false)).expect("rewritten");
        assert!(out[4..20].iter().all(|&b| b == 0));
        let mut bad = container(0x0100_0000, true);
        bad[4] ^= 1;
        assert!(cleared_container(&bad).is_none());
        assert!(cleared_container(&container(0x11, true)).is_none());
    }
}
