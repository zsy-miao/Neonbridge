//! Seven WinHTTP imports exposed by the Xbox guest redirection table.
//! The table and callbacks are local to the guest process.
use core::ffi::{CStr, c_char, c_void};
use std::sync::RwLock;
use windows_sys::Win32::Networking::WinHttp;
type Handle = *mut c_void;
type Wide = *const u16;

type Open = unsafe extern "system" fn(Wide, u32, Wide, Wide, u32) -> Handle;
type Connect = unsafe extern "system" fn(Handle, Wide, u16, u32) -> Handle;
type Request =
    unsafe extern "system" fn(Handle, Wide, Wide, Wide, Wide, *const Wide, u32) -> Handle;
type Upgrade = unsafe extern "system" fn(*const c_void, usize) -> Handle;
type Close = unsafe extern "system" fn(Handle) -> i32;
type Receive = unsafe extern "system" fn(Handle, Handle) -> i32;
type OptionFn = unsafe extern "system" fn(*const c_void, u32, *const c_void, u32) -> i32;

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Hooks {
    open: Option<Open>,
    connect: Option<Connect>,
    request: Option<Request>,
    upgrade: Option<Upgrade>,
    close: Option<Close>,
    receive: Option<Receive>,
    set_option: Option<OptionFn>,
}
static HOOKS: RwLock<Hooks> = RwLock::new(Hooks {
    open: None,
    connect: None,
    request: None,
    upgrade: None,
    close: None,
    receive: None,
    set_option: None,
});

/// The guest provides seven function pointers with the WinHTTP signatures.
/// Null entries select the system function. The caller owns callback lifetimes.
#[unsafe(export_name = "XbSetWinHttpHooks")]
unsafe extern "system" fn set_hooks(hooks: *const Hooks) {
    if !hooks.is_null() {
        *HOOKS.write().unwrap_or_else(|e| e.into_inner()) = unsafe { hooks.read_unaligned() };
    }
}
macro_rules! route {
    ($name:ident, $field:ident, ($($arg:ident: $ty:ty),*) -> $result:ty) => {
        #[unsafe(export_name = concat!("guest_", stringify!($name)))]
        #[allow(non_snake_case)]
        unsafe extern "system" fn $name($($arg: $ty),*) -> $result {
            // Release the lock before calling out. Guest callbacks can replace the table.
            let callback = HOOKS.read().unwrap_or_else(|e| e.into_inner()).$field;
            unsafe { callback.unwrap_or(WinHttp::$name)($($arg),*) }
        }
    };
}
route!(WinHttpOpen, open, (agent: Wide, access: u32, proxy: Wide, bypass: Wide, flags: u32) -> Handle);
route!(WinHttpConnect, connect, (session: Handle, server: Wide, port: u16, flags: u32) -> Handle);
route!(WinHttpOpenRequest, request, (connection: Handle, method: Wide, object: Wide, version: Wide, referrer: Wide, accept: *const Wide, flags: u32) -> Handle);
route!(WinHttpWebSocketCompleteUpgrade, upgrade, (request: *const c_void, context: usize) -> Handle);
route!(WinHttpCloseHandle, close, (handle: Handle) -> i32);
route!(WinHttpReceiveResponse, receive, (request: Handle, reserved: Handle) -> i32);
route!(WinHttpSetOption, set_option, (handle: *const c_void, option: u32, buffer: *const c_void, bytes: u32) -> i32);

#[repr(C)]
struct Entry {
    module: *const c_char,
    name: *const c_char,
    function: *const c_void,
}
// SAFETY: These pointers refer to immutable strings and static code in this DLL.
unsafe impl Sync for Entry {}
impl Entry {
    const fn winhttp(name: &'static CStr, function: *const c_void) -> Self {
        Self {
            module: c"WINHTTP.dll".as_ptr(),
            name: name.as_ptr(),
            function,
        }
    }
}
static ENTRIES: [Entry; 7] = [
    Entry::winhttp(c"WinHttpOpen", WinHttpOpen as _),
    Entry::winhttp(c"WinHttpConnect", WinHttpConnect as _),
    Entry::winhttp(c"WinHttpOpenRequest", WinHttpOpenRequest as _),
    Entry::winhttp(c"WinHttpCloseHandle", WinHttpCloseHandle as _),
    Entry::winhttp(c"WinHttpReceiveResponse", WinHttpReceiveResponse as _),
    Entry::winhttp(c"WinHttpSetOption", WinHttpSetOption as _),
    Entry::winhttp(
        c"WinHttpWebSocketCompleteUpgrade",
        WinHttpWebSocketCompleteUpgrade as _,
    ),
];
#[repr(C)]
struct Redirections {
    version: u32,
    count: u32,
    entries: &'static [Entry; 7],
}
#[unsafe(export_name = "__RedirectionInformation__")]
static REDIRECTIONS: Redirections = Redirections {
    version: 1,
    count: ENTRIES.len() as u32,
    entries: &ENTRIES,
};
