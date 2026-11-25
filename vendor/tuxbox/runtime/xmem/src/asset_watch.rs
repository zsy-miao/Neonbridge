//! Streamed asset table watch (test171: game+0x73f892 read [slot 0 object
//! +0x30]+0x18 with the data pointer still 0 and crashed at t=90.8).
//! game+0x73f7a0 walks 56 16-byte slots at [game+0xab83ce0]; each slot holds
//! an object found by joaat key (keys at game+0x65b5e30, slot 0's 0xa6bfcf94)
//! whose type is [+0x10] (3) and whose loaded data is [+0x30]; it then uses
//! slot 0's data whenever the count at game+0xab83cdc is nonzero.
//! Every 250 ms this logs any change of slot 0's object, type, data pointer
//! and the count, so a run shows whether the data ever arrives and when.
//! TUXBOX_ASSET_WATCH=0 off.

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

const TABLE: usize = 0xab8_3ce0;
const COUNT: usize = 0xab8_3cdc;

fn read<const N: usize>(at: usize) -> Option<[u8; N]> {
    if at < 0x10000 {
        return None;
    }
    unsafe extern "system" {
        fn ReadProcessMemory(process: isize, base: *const core::ffi::c_void, buffer: *mut core::ffi::c_void, size: usize, read: *mut usize) -> i32;
    }
    let mut out = [0u8; N];
    let mut got = 0usize;
    (unsafe { ReadProcessMemory(-1, at as *const _, out.as_mut_ptr().cast(), N, &mut got) } != 0 && got == N).then_some(out)
}

fn q(at: usize) -> Option<u64> {
    read::<8>(at).map(u64::from_le_bytes)
}

pub(crate) fn install() {
    if std::env::var("TUXBOX_ASSET_WATCH").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    let base = unsafe { GetModuleHandleW(core::ptr::null()) } as usize;
    if base == 0 {
        return;
    }
    let _ = std::thread::Builder::new().name("TuxBox asset watch".into()).spawn(move || {
        let started = std::time::Instant::now();
        let mut last = None;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let table = q(base + TABLE).unwrap_or(0) as usize;
            let count = read::<4>(base + COUNT).map(u32::from_le_bytes);
            let object = if table != 0 { q(table).unwrap_or(0) as usize } else { 0 };
            let kind = if object != 0 { read::<2>(object + 0x10).map(u16::from_le_bytes) } else { None };
            let key = if object != 0 { read::<4>(object + 0x40).map(u32::from_le_bytes) } else { None };
            let data = if object != 0 { q(object + 0x30) } else { None };
            let now = (table, count, object, kind, key, data);
            if last != Some(now) {
                crate::loader::log(&format!(
                    "asset watch: table {table:#x} count {count:?} slot 0 object {object:#x} type {kind:?} key {:?} data {:?} at {:.1} s (TUXBOX_ASSET_WATCH=0 off)",
                    key.map(|k| format!("{k:#x}")), data.map(|d| format!("{d:#x}")), started.elapsed().as_secs_f32()));
                last = Some(now);
            }
        }
    });
}
