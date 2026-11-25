//! UAV-from-SRV slots: xshader's `<key>.slots` sidecar (beside `<key>.cso`)
//! names the guest root table slots a program stores to while the guest root
//! (RTS0) types them SRV (Coder4's image_store programs, 1233c986, 753c5178,
//! 2211091a: s5+0x20 -> t240). One line per slot:
//! `uav_from_srv space=<s> offset=<bytes> range=<n>`, where offset is the byte
//! offset in the guest table and range is the index into the root parameter's
//! ranges. The runtime binds such a slot as a UAV of the same resource.
//! TUXBOX_SRV_AS_UAV=1 on (off until every piece has landed).
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Slot {
    pub(crate) space: u32,
    pub(crate) offset: u32,
    pub(crate) range: u32,
}

pub(crate) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_SRV_AS_UAV") == Some("1")
}

/// Translated programs (by the SHA-256 of their DXIL) and their slots.
static SLOTS: Mutex<Option<HashMap<String, Arc<[Slot]>>>> = Mutex::new(None);

pub(super) fn parse(text: &str) -> Vec<Slot> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            if words.next()? != "uav_from_srv" {
                return None;
            }
            let (mut space, mut offset, mut range) = (None, None, None);
            for word in words {
                let (name, value) = word.split_once('=')?;
                let value = value.trim().parse::<u32>().ok()?;
                match name {
                    "space" => space = Some(value),
                    "offset" => offset = Some(value),
                    "range" => range = Some(value),
                    _ => {}
                }
            }
            Some(Slot { space: space?, offset: offset?, range: range? })
        })
        .collect()
}

/// Keeps the slots read beside the program whose DXIL hashes to `digest`.
pub(super) fn note(digest: &str, text: &str) {
    let slots = parse(text);
    if slots.is_empty() {
        return;
    }
    crate::diagnostics::log_line(format_args!(
        "srv as uav: program {} stores to {} SRV slot(s) of its root: {:?} (TUXBOX_SRV_AS_UAV)",
        &digest[..digest.len().min(16)], slots.len(), slots));
    SLOTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(digest.to_owned(), slots.into());
}

/// The slots of a translated program, by its DXIL (read by the bind pieces to come).
#[allow(dead_code)]
pub(crate) fn for_code(code: &[u8]) -> Option<Arc<[Slot]>> {
    if !enabled() {
        return None;
    }
    let digest = super::source_digest(code)?;
    SLOTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(&digest).cloned()
}
