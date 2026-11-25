//! Crash-diagnostic history page. Games see a read-only view of eight
//! half-second snapshots, most recent first.
use core::{ffi::c_void, ptr};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU64, Ordering};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HINSTANCE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};
use windows_sys::Win32::System::Threading::{
    CloseThreadpoolTimer, CreateThreadpoolTimer, PTP_CALLBACK_INSTANCE, PTP_TIMER,
    SetThreadpoolTimer, WaitForThreadpoolTimerCallbacks,
};

#[unsafe(export_name = "XMemHistoryData")]
static VIEW: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static WRITER: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static TIMER: AtomicIsize = AtomicIsize::new(0);
static UPDATING: AtomicBool = AtomicBool::new(false);

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(
        class: u32,
        data: *mut c_void,
        size: u32,
        returned: *mut u32,
    ) -> i32;
    fn NtQueryInformationProcess(
        process: isize,
        class: u32,
        data: *mut c_void,
        size: u32,
        returned: *mut u32,
    ) -> i32;
}

// The timer can overlap DLL teardown. Use only eager ntdll queries, stack
// storage and atomics here so DLL unload never waits on its own loader lock.
unsafe extern "system" fn sample(_: PTP_CALLBACK_INSTANCE, context: *mut c_void, _: PTP_TIMER) {
    if UPDATING.swap(true, Ordering::Acquire) {
        return;
    }
    let mut performance = [0u32; 256];
    let mut basic = [0u32; 16];
    let mut vm = [0u64; 12];
    let mut vm_len = 0;
    let ok = unsafe {
        NtQuerySystemInformation(
            2,
            performance.as_mut_ptr().cast(),
            size_of_val(&performance) as u32,
            ptr::null_mut(),
        ) >= 0
            && NtQuerySystemInformation(
                0,
                basic.as_mut_ptr().cast(),
                size_of_val(&basic) as u32,
                ptr::null_mut(),
            ) >= 0
            && NtQueryInformationProcess(
                -1,
                3,
                vm.as_mut_ptr().cast(),
                size_of_val(&vm) as u32,
                &mut vm_len,
            ) >= 0
    };
    if ok && vm_len >= 88 {
        let page = u64::from(basic[2]);
        let used = if vm_len >= 96 && vm[11] != 0 {
            vm[11]
        } else {
            vm[9]
        };
        let available =
            u64::from(performance[11].min(performance[13].saturating_sub(performance[12]))) * page;
        let budget = used
            .saturating_add(available.saturating_sub(crate::physical_pages::relinquished()))
            .min(u64::from(basic[3]) * page);
        let mut latest = [0u64; 15];
        latest[0] = budget;
        latest[1] = used;
        latest[5] = budget;
        latest[6] = used;
        latest[11] = vm[10].max(used);
        // Both file views address the same zero-initialized page. Store whole
        // words so a guest sampling between ticks can read each counter intact.
        let words = unsafe { core::slice::from_raw_parts(context.cast::<AtomicU64>(), 8 * 15) };
        for n in (15..words.len()).rev() {
            words[n].store(words[n - 15].load(Ordering::Relaxed), Ordering::Relaxed);
        }
        for (dst, src) in words.iter().zip(latest) {
            dst.store(src, Ordering::Relaxed);
        }
    }
    UPDATING.store(false, Ordering::Release);
}

unsafe fn start() {
    let section = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            0,
            4096,
            ptr::null(),
        )
    };
    if section.is_null() {
        return;
    }
    let read = unsafe { MapViewOfFile(section, FILE_MAP_READ, 0, 0, 4096) };
    let write = unsafe { MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, 4096) };
    unsafe { CloseHandle(section) };
    let timer = if read.Value.is_null() || write.Value.is_null() {
        0
    } else {
        unsafe { CreateThreadpoolTimer(Some(sample), write.Value, ptr::null()) }
    };
    if timer == 0 {
        for view in [read, write] {
            if !view.Value.is_null() {
                unsafe { UnmapViewOfFile(view) };
            }
        }
        return;
    }
    VIEW.store(read.Value, Ordering::Release);
    WRITER.store(write.Value, Ordering::Release);
    TIMER.store(timer, Ordering::Release);
    // Negative relative FILETIME in 100 ns units.
    let due = FILETIME {
        dwLowDateTime: (-5_000_000i64) as u32,
        dwHighDateTime: u32::MAX,
    };
    unsafe { SetThreadpoolTimer(timer, &due, 500, 0) };
}

#[unsafe(no_mangle)]
extern "system" fn DllMain(_: HINSTANCE, event: u32, reserved: *const c_void) -> i32 {
    if event == DLL_PROCESS_ATTACH {
        unsafe { start() };
    } else if event == DLL_PROCESS_DETACH && reserved.is_null() {
        crate::pools::unload();
        let timer = TIMER.swap(0, Ordering::AcqRel);
        if timer != 0 {
            unsafe {
                SetThreadpoolTimer(timer, ptr::null(), 0, 0);
                WaitForThreadpoolTimerCallbacks(timer, 1);
                CloseThreadpoolTimer(timer);
            }
            for view in [&VIEW, &WRITER] {
                let address = MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: view.swap(ptr::null_mut(), Ordering::AcqRel),
                };
                unsafe { UnmapViewOfFile(address) };
            }
        }
    }
    1
}
