//! The serialized root signature's digest, kept on each host root as private
//! data, so pipeline library names include the root (test178: every library
//! miss loaded 0x80070057 and 8 known ones could not be stored again: a name
//! that leaves the root out matched an entry made with another root).
//! TUXBOX_PIPELINE_NAME_ROOT=0 leaves it out of names as before.

use windows::Win32::Graphics::Direct3D12::ID3D12RootSignature;
use windows::core::{GUID, Interface, Result};

const DIGEST: GUID = GUID::from_u128(0x7b1c_6f3a_52d4_4e1b_9a0c_3e5f_8d21_c467);

pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_PIPELINE_NAME_ROOT") != Some("0"))
}

/// A host root made from `bytes`, tagged with their digest.
pub(crate) fn named(root: Result<ID3D12RootSignature>, bytes: &[u8]) -> Result<ID3D12RootSignature> {
    if let Ok(root) = &root
        && let Some(digest) = super::shader_cache::source_digest(bytes)
    {
        let _ = unsafe { root.SetPrivateData(&DIGEST, digest.len() as u32, Some(digest.as_ptr().cast())) };
    }
    root
}

/// The digest tagged on the host root at `raw`, if any.
pub(crate) fn digest(raw: *mut core::ffi::c_void) -> Option<String> {
    if raw.is_null() {
        return None;
    }
    let root = unsafe { ID3D12RootSignature::from_raw_borrowed(&raw) }?;
    let mut text = [0u8; 64];
    let mut size = text.len() as u32;
    unsafe { root.GetPrivateData(&DIGEST, &mut size, Some(text.as_mut_ptr().cast())) }.ok()?;
    String::from_utf8(text[..size as usize].to_vec()).ok()
}
