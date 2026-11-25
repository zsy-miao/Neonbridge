//! The title executable's SHA-256, logged once at device creation from a
//! background thread, so RVAs in notes can be checked against the build they
//! were written for (idea from leonida 49c81019, re-derived). The file on disk
//! is hashed, not the patched image in memory. TUXBOX_EXE_SHA=<hex> also says
//! whether it matches; TUXBOX_EXE_SHA=0 turns the check off.
use windows::Win32::Security::Cryptography::{
    BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALG_HANDLE, BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash, BCryptHashData,
};

pub(crate) fn start() {
    let expected = std::env::var("TUXBOX_EXE_SHA").ok();
    if expected.as_deref() == Some("0") {
        return;
    }
    let _ = std::thread::Builder::new().name("TuxBox exe sha".into()).spawn(move || {
        let Ok(path) = std::env::current_exe() else { return };
        let (digest, bytes) = match sha256(&path) {
            Ok(v) => v,
            Err(e) => {
                super::log_line(format_args!("exe sha: cannot hash {}: {e}", path.display()));
                return;
            }
        };
        let verdict = match expected.as_deref().map(str::trim) {
            Some(want) if want.eq_ignore_ascii_case(&digest) => " (matches TUXBOX_EXE_SHA)".to_string(),
            Some(want) => format!(" (DIFFERS from TUXBOX_EXE_SHA {want}: RVAs from those notes may not hold)"),
            None => String::new(),
        };
        super::log_line(format_args!("exe sha: {} {bytes} bytes sha256 {digest}{verdict}", path.display()));
    });
}

fn sha256(path: &std::path::Path) -> std::io::Result<(String, u64)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hash = BCRYPT_HASH_HANDLE::default();
    if unsafe { BCryptCreateHash(BCRYPT_SHA256_ALG_HANDLE, &mut hash, None, None, 0) }.is_err() {
        return Err(std::io::Error::other("BCryptCreateHash failed"));
    }
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0u64;
    let result = loop {
        let n = match file.read(&mut buffer) {
            Ok(0) => break Ok(()),
            Ok(n) => n,
            Err(e) => break Err(e),
        };
        total += n as u64;
        if unsafe { BCryptHashData(hash, &buffer[..n], 0) }.is_err() {
            break Err(std::io::Error::other("BCryptHashData failed"));
        }
    };
    let mut digest = [0u8; 32];
    let finished = unsafe { BCryptFinishHash(hash, &mut digest, 0) };
    let _ = unsafe { BCryptDestroyHash(hash) };
    result?;
    if finished.is_err() {
        return Err(std::io::Error::other("BCryptFinishHash failed"));
    }
    Ok((digest.iter().map(|b| format!("{b:02x}")).collect(), total))
}
