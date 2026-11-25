//! Offline Xbox Live (XSTS v1) HTTP request signing for `XUserGetTokenAndSignature*`.
//!
//! Titles and GDK libraries (`libHttpClient.GDK`, `XCurl`, `EOSSDK`, `GameChat2`) call
//! `XUserGetTokenAndSignature(Utf16)Async` before talking to online or local backends and
//! expect both a non-empty `XBL3.0` authorization token and a valid Base64-encoded XSTS v1
//! signature header (`[version: u32 BE][filetime: u64 BE][64-byte raw ECDSA P-256 sig]`).
//! An empty signature causes string-conversion helpers (e.g. `GameChat2`) to index past the
//! start of the buffer.
//!
//! Signing uses an ephemeral per-process ECDSA P-256 key pair created through Windows CNG
//! (`bcrypt.dll`) with zero external dependencies.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::util::{wide, HRESULT};

const XSTS_SIG_VERSION: u32 = 1;
const MAX_SIGNED_BODY_BYTES: usize = 8192;
const UNIX_TO_FILETIME_SECS: u64 = 11_644_473_600;

#[link(name = "bcrypt")]
extern "system" {
    fn BCryptOpenAlgorithmProvider(
        alg: *mut *mut c_void,
        alg_id: *const u16,
        implementation: *const u16,
        flags: u32,
    ) -> i32;
    fn BCryptGenerateKeyPair(alg: *mut c_void, key: *mut *mut c_void, length: u32, flags: u32) -> i32;
    fn BCryptFinalizeKeyPair(key: *mut c_void, flags: u32) -> i32;
    fn BCryptHash(
        alg: *mut c_void,
        secret: *const u8,
        secret_len: u32,
        input: *const u8,
        input_len: u32,
        output: *mut u8,
        output_len: u32,
    ) -> i32;
    fn BCryptSignHash(
        key: *mut c_void,
        padding: *const c_void,
        input: *const u8,
        input_len: u32,
        output: *mut u8,
        output_len: u32,
        result: *mut u32,
        flags: u32,
    ) -> i32;
    #[cfg(test)]
    fn BCryptVerifySignature(
        key: *mut c_void,
        padding: *const c_void,
        hash: *const u8,
        hash_len: u32,
        signature: *const u8,
        signature_len: u32,
        flags: u32,
    ) -> i32;
}

struct EphemeralSigner {
    sha256_alg: usize,
    ecdsa_key: usize,
}

static SIGNER: OnceLock<Result<EphemeralSigner, HRESULT>> = OnceLock::new();

fn hresult_from_ntstatus(status: i32) -> HRESULT {
    if status >= 0 {
        status
    } else {
        ((status as u32) | 0x1000_0000) as HRESULT
    }
}

fn signer() -> Result<&'static EphemeralSigner, HRESULT> {
    SIGNER
        .get_or_init(|| unsafe {
            let mut sha256_alg = std::ptr::null_mut();
            let st = BCryptOpenAlgorithmProvider(&mut sha256_alg, wide("SHA256").as_ptr(), std::ptr::null(), 0);
            if st < 0 {
                return Err(hresult_from_ntstatus(st));
            }
            let mut ecdsa_alg = std::ptr::null_mut();
            let st = BCryptOpenAlgorithmProvider(&mut ecdsa_alg, wide("ECDSA_P256").as_ptr(), std::ptr::null(), 0);
            if st < 0 {
                return Err(hresult_from_ntstatus(st));
            }
            let mut ecdsa_key = std::ptr::null_mut();
            let st = BCryptGenerateKeyPair(ecdsa_alg, &mut ecdsa_key, 256, 0);
            if st < 0 {
                return Err(hresult_from_ntstatus(st));
            }
            let st = BCryptFinalizeKeyPair(ecdsa_key, 0);
            if st < 0 {
                return Err(hresult_from_ntstatus(st));
            }
            Ok(EphemeralSigner { sha256_alg: sha256_alg as usize, ecdsa_key: ecdsa_key as usize })
        })
        .as_ref()
        .map_err(|&e| e)
}

/// Offline `XBL3.0` authorization header derived from the configured local user id.
pub fn offline_token() -> String {
    format!("XBL3.0 x=xcl;local-{:016x}", crate::state::config().user_id)
}

/// Current UTC timestamp as a Windows FILETIME (100-ns intervals since 1601-01-01).
pub fn current_filetime() -> u64 {
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    dur.as_secs()
        .saturating_add(UNIX_TO_FILETIME_SECS)
        .saturating_mul(10_000_000)
        .saturating_add(u64::from(dur.subsec_nanos() / 100))
}

/// Extract the canonical path-and-query component from a URL for XSTS signing.
/// Strips any `scheme://authority` prefix and `#fragment`, ensuring a leading `/`.
pub fn canonical_path_and_query(url: &str) -> String {
    let without_scheme = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    let without_frag = without_scheme.split('#').next().unwrap_or("");
    match without_frag.find(['/', '?']) {
        Some(idx) if without_frag.as_bytes()[idx] == b'/' => without_frag[idx..].to_string(),
        Some(idx) => format!("/{}", &without_frag[idx..]),
        None => "/".to_string(),
    }
}

/// Assemble the XSTS v1 canonical byte string that is hashed with SHA-256 before signing.
pub fn canonical_payload(filetime: u64, method: &str, url: &str, authorization: &str, body: &[u8]) -> Vec<u8> {
    let path = canonical_path_and_query(url);
    let capped_body = &body[..body.len().min(MAX_SIGNED_BODY_BYTES)];
    let mut out = Vec::with_capacity(20 + method.len() + path.len() + authorization.len() + capped_body.len());
    out.extend_from_slice(&XSTS_SIG_VERSION.to_be_bytes());
    out.push(0);
    out.extend_from_slice(&filetime.to_be_bytes());
    out.push(0);
    out.extend_from_slice(method.to_ascii_uppercase().as_bytes());
    out.push(0);
    out.extend_from_slice(path.as_bytes());
    out.push(0);
    out.extend_from_slice(authorization.as_bytes());
    out.push(0);
    out.extend_from_slice(capped_body);
    out.push(0);
    out
}

/// Standard RFC 4648 Base64 encoder with `=` padding.
pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut chunks = input.chunks_exact(3);
    for c in &mut chunks {
        let v = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32);
        out.push(TABLE[((v >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((v >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((v >> 6) & 0x3F) as usize] as char);
        out.push(TABLE[(v & 0x3F) as usize] as char);
    }
    match chunks.remainder() {
        [a] => {
            let v = (*a as u32) << 16;
            out.push(TABLE[((v >> 18) & 0x3F) as usize] as char);
            out.push(TABLE[((v >> 12) & 0x3F) as usize] as char);
            out.push('=');
            out.push('=');
        }
        [a, b] => {
            let v = ((*a as u32) << 16) | ((*b as u32) << 8);
            out.push(TABLE[((v >> 18) & 0x3F) as usize] as char);
            out.push(TABLE[((v >> 12) & 0x3F) as usize] as char);
            out.push(TABLE[((v >> 6) & 0x3F) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

/// Produce a Base64-encoded XSTS v1 signature header at an explicit FILETIME.
pub fn sign_request_at(filetime: u64, method: &str, url: &str, authorization: &str, body: &[u8]) -> Result<String, HRESULT> {
    let s = signer()?;
    let payload = canonical_payload(filetime, method, url, authorization, body);
    let mut digest = [0u8; 32];
    let mut raw_sig = [0u8; 64];
    let mut sig_len = 0u32;
    unsafe {
        let st = BCryptHash(
            s.sha256_alg as *mut c_void,
            std::ptr::null(),
            0,
            payload.as_ptr(),
            payload.len() as u32,
            digest.as_mut_ptr(),
            digest.len() as u32,
        );
        if st < 0 {
            return Err(hresult_from_ntstatus(st));
        }
        let st = BCryptSignHash(
            s.ecdsa_key as *mut c_void,
            std::ptr::null(),
            digest.as_ptr(),
            digest.len() as u32,
            raw_sig.as_mut_ptr(),
            raw_sig.len() as u32,
            &mut sig_len,
            0,
        );
        if st < 0 {
            return Err(hresult_from_ntstatus(st));
        }
    }
    let mut header = Vec::with_capacity(12 + sig_len as usize);
    header.extend_from_slice(&XSTS_SIG_VERSION.to_be_bytes());
    header.extend_from_slice(&filetime.to_be_bytes());
    header.extend_from_slice(&raw_sig[..sig_len as usize]);
    Ok(base64_encode(&header))
}

/// Produce a Base64-encoded XSTS v1 signature header stamped with the current UTC time.
pub fn sign_request(method: &str, url: &str, authorization: &str, body: &[u8]) -> Result<String, HRESULT> {
    sign_request_at(current_filetime(), method, url, authorization, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base64_decode(input: &str) -> Vec<u8> {
        let val = |b: u8| -> Option<u32> {
            match b {
                b'A'..=b'Z' => Some((b - b'A') as u32),
                b'a'..=b'z' => Some((b - b'a' + 26) as u32),
                b'0'..=b'9' => Some((b - b'0' + 52) as u32),
                b'+' => Some(62),
                b'/' => Some(63),
                _ => None,
            }
        };
        let bytes = input.as_bytes();
        assert_eq!(bytes.len() % 4, 0);
        let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
        for chunk in bytes.chunks_exact(4) {
            let a = val(chunk[0]).unwrap();
            let b = val(chunk[1]).unwrap();
            out.push(((a << 2) | (b >> 4)) as u8);
            if chunk[2] != b'=' {
                let c = val(chunk[2]).unwrap();
                out.push((((b & 0x0F) << 4) | (c >> 2)) as u8);
                if chunk[3] != b'=' {
                    let d = val(chunk[3]).unwrap();
                    out.push((((c & 0x03) << 6) | d) as u8);
                }
            }
        }
        out
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (plain, encoded) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(plain), encoded);
            assert_eq!(base64_decode(encoded), plain);
        }
    }

    #[test]
    fn canonical_url_extracts_path_and_query() {
        assert_eq!(canonical_path_and_query("https://userstats.xboxlive.com/batch?a=1#frag"), "/batch?a=1");
        assert_eq!(canonical_path_and_query("wss://rta.xboxlive.com/connect"), "/connect");
        assert_eq!(canonical_path_and_query("https://example.invalid"), "/");
        assert_eq!(canonical_path_and_query("https://example.invalid?k=v"), "/?k=v");
    }

    #[test]
    fn canonical_payload_caps_body_at_8192_bytes() {
        let big = vec![b'x'; MAX_SIGNED_BODY_BYTES + 256];
        let p = canonical_payload(0x1122_3344_5566_7788, "post", "https://h/p?q=1", "XBL3.0 x=a;b", &big);
        let prefix = [0, 0, 0, 1, 0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0];
        assert!(p.starts_with(&prefix));
        let meta = b"POST\0/p?q=1\0XBL3.0 x=a;b\0";
        assert_eq!(&p[prefix.len()..prefix.len() + meta.len()], meta);
        let body_part = &p[prefix.len() + meta.len()..];
        assert_eq!(body_part.len(), MAX_SIGNED_BODY_BYTES + 1);
        assert!(body_part[..MAX_SIGNED_BODY_BYTES].iter().all(|&b| b == b'x'));
        assert_eq!(body_part[MAX_SIGNED_BODY_BYTES], 0);
    }

    #[test]
    fn signature_header_verifies_with_ephemeral_p256_key() {
        let ts = 0x01DB_1234_5678_9ABCu64;
        let auth = offline_token();
        let encoded = sign_request_at(ts, "POST", "https://social.xboxlive.com/users/me/people", &auth, b"{\"ok\":true}").unwrap();
        let raw = base64_decode(&encoded);
        assert_eq!(raw.len(), 76);
        assert_eq!(&raw[0..4], &XSTS_SIG_VERSION.to_be_bytes());
        assert_eq!(&raw[4..12], &ts.to_be_bytes());

        // Verify the raw 64-byte P-256 signature against the exact SHA-256 digest of the payload.
        let s = signer().unwrap();
        let payload = canonical_payload(ts, "POST", "https://social.xboxlive.com/users/me/people", &auth, b"{\"ok\":true}");
        let mut digest = [0u8; 32];
        unsafe {
            assert!(
                BCryptHash(
                    s.sha256_alg as *mut c_void,
                    std::ptr::null(),
                    0,
                    payload.as_ptr(),
                    payload.len() as u32,
                    digest.as_mut_ptr(),
                    32,
                ) >= 0
            );
            let st = BCryptVerifySignature(
                s.ecdsa_key as *mut c_void,
                std::ptr::null(),
                digest.as_ptr(),
                32,
                raw[12..].as_ptr(),
                64,
                0,
            );
            assert!(st >= 0, "BCryptVerifySignature failed: {st:#x}");
        }
    }
}
