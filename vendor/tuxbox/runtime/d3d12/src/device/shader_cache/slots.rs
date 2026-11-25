//! UAV-from-SRV slots: xshader's `<key>.slots` sidecar (published by the
//! launcher beside `<key>.cso`) names the guest root table slots a program
//! stores to while the guest root (RTS0) types them SRV (Coder4's
//! image_store programs 1233c986, 753c5178, 2211091a: s5+0x20 -> t240). One
//! line per slot: `uav_from_srv param=<root param> slot=<descriptor index>
//! space=<s> register=<u register> offset=<bytes> range=<n>`. offset is the
//! byte offset in the guest table and range the index into the root
//! parameter's ranges; slot defaults to offset / 32 (a T# is 32 bytes) and
//! param to 0. The runtime binds such a slot as a UAV of the same resource.
//! TUXBOX_SRV_AS_UAV=1 on (off until every piece has landed).
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Slot {
    pub(crate) param: u32,
    pub(crate) slot: u32,
    pub(crate) space: u32,
    pub(crate) register: Option<u32>,
    pub(crate) offset: u32,
    pub(crate) range: u32,
}

pub(crate) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_SRV_AS_UAV") == Some("1")
}

pub(super) fn parse(text: &str) -> Vec<Slot> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            if words.next()? != "uav_from_srv" {
                return None;
            }
            let mut field: HashMap<&str, u32> = HashMap::new();
            for word in words {
                if let Some((name, value)) = word.split_once('=')
                    && let Ok(value) = value.trim().parse::<u32>()
                {
                    field.insert(name, value);
                }
            }
            let offset = *field.get("offset")?;
            Some(Slot {
                param: field.get("param").copied().unwrap_or(0),
                slot: field.get("slot").copied().unwrap_or(offset / 32),
                space: *field.get("space")?,
                register: field.get("register").copied(),
                offset,
                range: *field.get("range")?,
            })
        })
        .collect()
}

/// Programs (by the SHA-256 of their guest bytecode) and their slots.
static SLOTS: Mutex<Option<HashMap<String, Arc<[Slot]>>>> = Mutex::new(None);

/// The slots of the guest program `guest`, once its `.slots` is published
/// (None: off, no file yet, or none).
#[allow(dead_code)]
pub(crate) fn for_guest(guest: &[u8]) -> Option<Arc<[Slot]>> {
    if !enabled() {
        return None;
    }
    let source = super::source_digest(guest)?;
    if let Some(found) = SLOTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&source).cloned()) {
        return Some(found);
    }
    let slots: Arc<[Slot]> = parse(&super::compile::slots_text(&source)?).into();
    if slots.is_empty() {
        return None;
    }
    crate::diagnostics::log_line(format_args!(
        "srv as uav: program {} stores to {} SRV slot(s) of its root: {:?} (TUXBOX_SRV_AS_UAV)",
        &source[..16], slots.len(), slots));
    SLOTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(source, slots.clone());
    Some(slots)
}
