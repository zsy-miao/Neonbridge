//! Survey of runtime API classes TuxBox does not implement yet. Each class
//! answers QueryApiImpl with an interface whose every method logs its slot,
//! the first five integer arguments and the caller, then returns E_NOTIMPL.
//! The six identified classes are served by `services` (slots it implements)
//! by default; TUXBOX_RUNTIME_SERVICES=0 withholds them. TUXBOX_RUNTIME_PROBE=1
//! also exposes the unidentified class.
use super::{NOT_IMPLEMENTED, Object, Slot, report};
use std::sync::atomic::AtomicU32;
use windows_sys::core::GUID;

pub(super) const CLASSES: [(GUID, &str); 10] = [
    (GUID::from_u128(0x01acd177_91f9_4763_a38e_ccbb55ce32e0), "XUser"),
    (GUID::from_u128(0x973a344e_24bf_4d0f_8457_56c534892b29), "XGame"),
    (GUID::from_u128(0xaf406016_e850_4aa8_a88d_2f3dcb9dac7e), "XPackage"),
    (GUID::from_u128(0xf4faf4d4_2d04_4fce_b3e0_474a713a3e84), "XPersistentLocalStorage"),
    (GUID::from_u128(0x37e56907_2f10_41e8_b72f_36edb185331a), "XNetworking"),
    (GUID::from_u128(0x03f0fe74_fdd9_4e5c_b630_f9339c47acc5), "XDisplay"),
    (GUID::from_u128(0x3e241977_6237_41e9_8559_102c2d9983f1), "Unknown3e241977"),
    (GUID::from_u128(0xb884675d_b738_4a9c_815d_9a9a1e0c6c9b), "Unknownb884675d"),
    (GUID::from_u128(0x704c3f58_e629_4cc2_b197_30511b996fe2), "XGameSave"),
    // XUser's gamertag interface (IID cef4fac0); selected by IID, not class.
    (GUID::from_u128(0xcef4fac0_7676_4a94_a119_4c43f9eb5b74), "XUserGamertag"),
];

/// Interfaces chosen by IID within a class.
pub(super) const GAMERTAG: usize = 9;

pub(super) fn enabled() -> bool {
    std::env::var_os("TUXBOX_RUNTIME_PROBE").is_some_and(|v| v == "1")
}

fn caller_rva() -> usize {
    use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    let image = unsafe { GetModuleHandleW(core::ptr::null()) } as usize;
    let mut frames = [core::ptr::null_mut(); 8];
    let count = unsafe { RtlCaptureStackBackTrace(0, 8, frames.as_mut_ptr(), core::ptr::null_mut()) } as usize;
    frames[..count].iter().map(|&p| p as usize)
        .find(|&p| p >= image && p - image < 0x2000_0000).map_or(0, |p| p - image)
}

unsafe extern "system" fn method<const K: usize, const N: usize>(
    _this: &Object, a: usize, b: usize, c: usize, d: usize, e: usize,
) -> i32 {
    report(format_args!("probe {} slot {N} args {a:#x} {b:#x} {c:#x} {d:#x} {e:#x} from title RVA {:#x}",
        CLASSES[K].1, caller_rva()));
    NOT_IMPLEMENTED
}

macro_rules! table {
    ($k:literal) => {{
        let mut t = [Slot(method::<$k, 0> as *const ()); 64];
        table!(@fill t $k; 3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,
            32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63);
        t[0] = Slot(super::query_probe as *const ());
        t[1] = Slot(super::add_ref as *const ());
        t[2] = Slot(super::release as *const ());
        t
    }};
    (@fill $t:ident $k:literal; $($n:literal),*) => { $( $t[$n] = Slot(method::<$k, $n> as *const ()); )* };
}

macro_rules! service {
    ($f:path) => { Slot($f as *const ()) };
}

static TABLES: [[Slot; 64]; 10] = {
    use super::services::*;
    let mut t = [table!(0), table!(1), table!(2), table!(3), table!(4), table!(5), table!(6), table!(7), table!(8), table!(9)];
    // XUser
    t[0][3] = service!(user_duplicate);
    t[0][4] = service!(user_close);
    t[0][5] = service!(user_compare);
    t[0][6] = service!(user_max_users);
    t[0][7] = service!(user_add_async);
    t[0][8] = service!(user_add_result);
    t[0][9] = service!(user_local_id);
    t[0][10] = service!(user_find_by_local_id);
    t[0][11] = service!(user_id);
    t[0][12] = service!(user_find_by_id);
    t[0][13] = service!(user_is_guest);
    t[0][14] = service!(user_state);
    t[0][19] = service!(user_age_group);
    t[0][20] = service!(user_check_privilege);
    t[9][3] = service!(user_gamertag);
    // XGameSave
    t[8][3] = service!(save_initialize);
    t[8][4] = service!(save_initialize_async);
    t[8][5] = service!(save_initialize_result);
    t[8][6] = service!(save_close_provider);
    t[8][7] = service!(super::save::quota);
    t[8][8] = service!(super::save::quota_async);
    t[8][9] = service!(super::save::quota_result);
    t[8][10] = service!(super::save::delete);
    t[8][11] = service!(super::save::delete_async);
    t[8][12] = service!(super::save::status_result);
    t[8][13] = service!(super::save::container_info);
    t[8][14] = service!(super::save::enumerate);
    t[8][15] = service!(super::save::enumerate_by_name);
    t[8][16] = service!(super::save::create_container);
    t[8][17] = service!(super::save::close_container);
    t[8][18] = service!(super::save::blobs);
    t[8][19] = service!(super::save::blobs_by_name);
    t[8][20] = service!(super::save::read);
    t[8][21] = service!(super::save::read_async);
    t[8][22] = service!(super::save::read_result);
    t[8][23] = service!(super::save::create_update);
    t[8][24] = service!(super::save::close_update);
    t[8][25] = service!(super::save::blob_write);
    t[8][26] = service!(super::save::blob_delete);
    t[8][27] = service!(super::save::submit);
    t[8][28] = service!(super::save::submit_async);
    t[8][29] = service!(super::save::status_result);
    t[0][33] = service!(user_register_change);
    t[0][34] = service!(user_unregister_change);
    // XGame
    t[1][3] = service!(game_title_id);
    // XPackage
    t[2][3] = service!(package_identifier);
    t[2][4] = service!(package_is_packaged);
    t[2][5] = service!(package_create_monitor);
    t[2][6] = service!(package_close_monitor);
    t[2][7] = service!(package_progress);
    t[2][9] = service!(package_register_progress);
    t[2][10] = service!(unregister);
    // Slot 39 in this GDK (28 in older ones); identified by its callback's
    // XPackageDetails reads.
    t[2][39] = service!(package_enumerate);
    // XPersistentLocalStorage
    t[3][4] = service!(storage_get_path);
    // XNetworking
    t[4][15] = service!(network_hint);
    t[4][16] = service!(network_register);
    // XDisplay
    t[5][4] = service!(display_try_hdr);
    t
};

/// Classes with offline services; the last (unidentified) one is probe-only.
/// TUXBOX_RUNTIME_SERVICES=0 withholds all of them; a comma-separated list of
/// class names (for example `XUser,XGame`) offers only those.
pub(super) fn available(index: usize) -> bool {
    let selected = match std::env::var("TUXBOX_RUNTIME_SERVICES") {
        Err(_) => true,
        Ok(list) if list == "0" => false,
        Ok(list) => list.split(',').any(|name| name.trim() == CLASSES[index].1),
    };
    (index < 6 || index == 8 || index == GAMERTAG) && selected || enabled()
}
pub(super) static OBJECTS: [super::ProbeObject; 10] = {
    let mut i = 0;
    let mut out = [const { super::ProbeObject { vtable: &TABLES[0], references: AtomicU32::new(0) } }; 10];
    while i < 10 {
        out[i] = super::ProbeObject { vtable: &TABLES[i], references: AtomicU32::new(0) };
        i += 1;
    }
    out
};
