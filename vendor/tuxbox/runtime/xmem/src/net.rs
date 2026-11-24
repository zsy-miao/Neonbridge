//! The title is offline by default. Its crash handler uploads a process
//! minidump (with host details) over WinHTTP, and telemetry and online services
//! use WinHTTP and Winsock directly. The exe's and libHttpClient's imports of
//! those, and GetProcAddress lookups of them, are patched so every outside
//! connection fails the way an unplugged console's would: name lookups find no
//! host, connects find no network. Each blocked attempt is logged with its host
//! (never its payload). Loopback stays allowed. TUXBOX_NETWORK=1 lets it all
//! through unchanged.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::loader::{log, patch_module_imports};

type H = *mut c_void;

unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> H;
    fn GetModuleFileNameW(module: H, buf: *mut u16, len: u32) -> u32;
    fn GetProcAddress(module: H, name: *const u8) -> *const c_void;
    fn SetLastError(code: u32);
}

const WSAENETUNREACH: i32 = 10051;
const WSAHOST_NOT_FOUND: i32 = 11001;
const ERROR_WINHTTP_NAME_NOT_RESOLVED: u32 = 12007;
const SOCKET_ERROR: i32 = -1;

/// The real `WSASetLastError` and `connect`/`sendto`, for loopback and errors.
static WSA_SET_LAST_ERROR: AtomicUsize = AtomicUsize::new(0);
static CONNECT: AtomicUsize = AtomicUsize::new(0);
static SENDTO: AtomicUsize = AtomicUsize::new(0);
static GETADDRINFO: AtomicUsize = AtomicUsize::new(0);
static GETADDRINFOW: AtomicUsize = AtomicUsize::new(0);
static GETHOSTBYNAME: AtomicUsize = AtomicUsize::new(0);
static GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);

fn blocked(what: &str, host: &str) {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 32 || n.is_power_of_two() {
        log(&format!("network blocked: {what} {host} ({n} so far; TUXBOX_NETWORK=1 allows it)"));
    }
}

fn wsa_error(code: i32) {
    let f = WSA_SET_LAST_ERROR.load(Ordering::Relaxed);
    if f != 0 {
        let f: unsafe extern "system" fn(i32) = unsafe { std::mem::transmute(f) };
        unsafe { f(code) };
    }
}

fn wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) })
}

fn ansi(p: *const u8) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned()
}

fn local_name(host: &str) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    h.is_empty() || h == "localhost" || h.starts_with("127.") || h == "::1" || h == "[::1]"
}

/// The printable address of a sockaddr and whether it is loopback (or unset).
fn address(addr: *const u8, len: i32) -> (String, bool) {
    if addr.is_null() || len < 2 {
        return (String::from("(none)"), true);
    }
    let family = unsafe { std::ptr::read_unaligned(addr as *const u16) };
    match family {
        2 if len >= 8 => {
            let b = unsafe { std::slice::from_raw_parts(addr, 8) };
            let port = u16::from_be_bytes([b[2], b[3]]);
            (format!("{}.{}.{}.{}:{port}", b[4], b[5], b[6], b[7]), b[4] == 127 || b[4..8] == [0, 0, 0, 0])
        }
        23 if len >= 24 => {
            let b = unsafe { std::slice::from_raw_parts(addr, 24) };
            let port = u16::from_be_bytes([b[2], b[3]]);
            let ip = &b[8..24];
            let loopback = ip[..15].iter().all(|&x| x == 0) && ip[15] <= 1;
            let v4 = ip[..10].iter().all(|&x| x == 0) && ip[10] == 0xff && ip[11] == 0xff;
            let text = if v4 { format!("{}.{}.{}.{}", ip[12], ip[13], ip[14], ip[15]) } else { ip.chunks(2).map(|c| format!("{:x}", u16::from_be_bytes([c[0], c[1]]))).collect::<Vec<_>>().join(":") };
            (format!("[{text}]:{port}"), loopback || (v4 && ip[12] == 127))
        }
        _ => (format!("family {family}"), false),
    }
}

unsafe extern "system" fn connect(s: usize, addr: *const u8, len: i32) -> i32 {
    let (text, local) = address(addr, len);
    let real = CONNECT.load(Ordering::Relaxed);
    if local && real != 0 {
        let f: unsafe extern "system" fn(usize, *const u8, i32) -> i32 = unsafe { std::mem::transmute(real) };
        return unsafe { f(s, addr, len) };
    }
    blocked("connect", &text);
    wsa_error(WSAENETUNREACH);
    SOCKET_ERROR
}

unsafe extern "system" fn sendto(s: usize, buf: *const u8, n: i32, flags: i32, addr: *const u8, len: i32) -> i32 {
    // A connected socket sends with no address; its connect was already checked.
    let (text, local) = address(addr, len);
    let real = SENDTO.load(Ordering::Relaxed);
    if (addr.is_null() || local) && real != 0 {
        let f: unsafe extern "system" fn(usize, *const u8, i32, i32, *const u8, i32) -> i32 = unsafe { std::mem::transmute(real) };
        return unsafe { f(s, buf, n, flags, addr, len) };
    }
    blocked("sendto", &text);
    wsa_error(WSAENETUNREACH);
    SOCKET_ERROR
}

unsafe extern "system" fn getaddrinfo(node: *const u8, service: *const u8, hints: *const c_void, out: *mut *mut c_void) -> i32 {
    let host = ansi(node);
    let real = GETADDRINFO.load(Ordering::Relaxed);
    if local_name(&host) && real != 0 {
        let f: unsafe extern "system" fn(*const u8, *const u8, *const c_void, *mut *mut c_void) -> i32 = unsafe { std::mem::transmute(real) };
        return unsafe { f(node, service, hints, out) };
    }
    blocked("name lookup", &host);
    if !out.is_null() {
        unsafe { out.write(std::ptr::null_mut()) };
    }
    wsa_error(WSAHOST_NOT_FOUND);
    WSAHOST_NOT_FOUND
}

unsafe extern "system" fn getaddrinfo_w(node: *const u16, service: *const u16, hints: *const c_void, out: *mut *mut c_void) -> i32 {
    let host = wide(node);
    let real = GETADDRINFOW.load(Ordering::Relaxed);
    if local_name(&host) && real != 0 {
        let f: unsafe extern "system" fn(*const u16, *const u16, *const c_void, *mut *mut c_void) -> i32 = unsafe { std::mem::transmute(real) };
        return unsafe { f(node, service, hints, out) };
    }
    blocked("name lookup", &host);
    if !out.is_null() {
        unsafe { out.write(std::ptr::null_mut()) };
    }
    wsa_error(WSAHOST_NOT_FOUND);
    WSAHOST_NOT_FOUND
}

unsafe extern "system" fn gethostbyname(name: *const u8) -> *mut c_void {
    let host = ansi(name);
    let real = GETHOSTBYNAME.load(Ordering::Relaxed);
    if local_name(&host) && real != 0 {
        let f: unsafe extern "system" fn(*const u8) -> *mut c_void = unsafe { std::mem::transmute(real) };
        return unsafe { f(name) };
    }
    blocked("name lookup", &host);
    wsa_error(WSAHOST_NOT_FOUND);
    std::ptr::null_mut()
}

unsafe extern "system" fn winhttp_connect(_session: H, server: *const u16, port: u16, _reserved: u32) -> H {
    blocked("WinHTTP connect", &format!("{}:{port}", wide(server)));
    unsafe { SetLastError(ERROR_WINHTTP_NAME_NOT_RESOLVED) };
    std::ptr::null_mut()
}

unsafe extern "system" fn internet_connect(_net: H, server: *const u16, port: u16, _user: *const u16, _pass: *const u16, _service: u32, _flags: u32, _context: usize) -> H {
    blocked("WinINet connect", &format!("{}:{port}", wide(server)));
    unsafe { SetLastError(ERROR_WINHTTP_NAME_NOT_RESOLVED) };
    std::ptr::null_mut()
}

unsafe extern "system" fn internet_connect_a(_net: H, server: *const u8, port: u16, _user: *const u8, _pass: *const u8, _service: u32, _flags: u32, _context: usize) -> H {
    blocked("WinINet connect", &format!("{}:{port}", ansi(server)));
    unsafe { SetLastError(ERROR_WINHTTP_NAME_NOT_RESOLVED) };
    std::ptr::null_mut()
}

unsafe extern "system" fn internet_open_url(_net: H, url: *const u16, _headers: *const u16, _len: u32, _flags: u32, _context: usize) -> H {
    let url = wide(url);
    blocked("WinINet open", url.split('?').next().unwrap_or_default());
    unsafe { SetLastError(ERROR_WINHTTP_NAME_NOT_RESOLVED) };
    std::ptr::null_mut()
}

/// Names (lower-case DLL base name, export) a lookup can ask for, and their blocks.
fn by_name(dll: &str, name: &str) -> Option<usize> {
    let ws = dll.starts_with("ws2_32");
    Some(match name {
        "connect" if ws => connect as *const () as usize,
        "sendto" if ws => sendto as *const () as usize,
        "getaddrinfo" if ws => getaddrinfo as *const () as usize,
        "GetAddrInfoW" if ws => getaddrinfo_w as *const () as usize,
        "gethostbyname" if ws => gethostbyname as *const () as usize,
        "WinHttpConnect" if dll.starts_with("winhttp") => winhttp_connect as *const () as usize,
        "InternetConnectW" if dll.starts_with("wininet") => internet_connect as *const () as usize,
        "InternetConnectA" if dll.starts_with("wininet") => internet_connect_a as *const () as usize,
        "InternetOpenUrlW" if dll.starts_with("wininet") => internet_open_url as *const () as usize,
        _ => return None,
    })
}

/// GetProcAddress: a title (or its crash reporter) can look the network
/// functions up instead of importing them; hand those lookups the blocks too.
unsafe extern "system" fn get_proc_address(module: H, name: *const u8) -> *const c_void {
    let real: unsafe extern "system" fn(H, *const u8) -> *const c_void = unsafe { std::mem::transmute(GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let found = unsafe { real(module, name) };
    // Ordinal lookups have the name pointer below 0x10000.
    if found.is_null() || (name as usize) < 0x10000 {
        return found;
    }
    let mut buf = [0u16; 512];
    let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]).to_ascii_lowercase();
    let dll = path.rsplit(['\\', '/']).next().unwrap_or_default();
    match by_name(dll, &ansi(name)) {
        Some(block) => {
            log(&format!("network: GetProcAddress {dll}!{} -> blocked version", ansi(name)));
            block as *const c_void
        }
        None => found,
    }
}

unsafe fn real(dll: &str, name: &[u8]) -> usize {
    let w: Vec<u16> = dll.encode_utf16().chain([0]).collect();
    unsafe {
        let m = GetModuleHandleW(w.as_ptr());
        if m.is_null() { 0 } else { GetProcAddress(m, name.as_ptr()) as usize }
    }
}

/// Called from loader::install (xmem DllMain), after the exe's imports are bound.
pub(crate) unsafe fn install() {
    if std::env::var("TUXBOX_NETWORK").is_ok_and(|v| v.trim() == "1") {
        log("network: TUXBOX_NETWORK=1, the title's network calls go through");
        return;
    }
    unsafe {
        WSA_SET_LAST_ERROR.store(real("ws2_32.dll", b"WSASetLastError\0"), Ordering::Relaxed);
        CONNECT.store(real("ws2_32.dll", b"connect\0"), Ordering::Relaxed);
        SENDTO.store(real("ws2_32.dll", b"sendto\0"), Ordering::Relaxed);
        GETADDRINFO.store(real("ws2_32.dll", b"getaddrinfo\0"), Ordering::Relaxed);
        GETADDRINFOW.store(real("ws2_32.dll", b"GetAddrInfoW\0"), Ordering::Relaxed);
        GETHOSTBYNAME.store(real("ws2_32.dll", b"gethostbyname\0"), Ordering::Relaxed);
        GET_PROC_ADDRESS.store(GetProcAddress as *const () as usize, Ordering::Relaxed);

        let names: &[(&str, usize)] = &[
            ("connect", connect as *const () as usize),
            ("sendto", sendto as *const () as usize),
            ("getaddrinfo", getaddrinfo as *const () as usize),
            ("GetAddrInfoW", getaddrinfo_w as *const () as usize),
            ("gethostbyname", gethostbyname as *const () as usize),
            ("WinHttpConnect", winhttp_connect as *const () as usize),
            ("InternetConnectW", internet_connect as *const () as usize),
            ("InternetConnectA", internet_connect_a as *const () as usize),
            ("InternetOpenUrlW", internet_open_url as *const () as usize),
            ("GetProcAddress", get_proc_address as *const () as usize),
        ];
        // ws2_32 ordinals: 4 connect, 20 sendto, 52 gethostbyname.
        let ordinals: &[(u16, usize)] = &[
            (4, connect as *const () as usize),
            (20, sendto as *const () as usize),
            (52, gethostbyname as *const () as usize),
        ];
        let net = |d: &str| d.starts_with("ws2_32") || d.starts_with("winhttp") || d.starts_with("wininet") || d.contains("libraryloader") || d == "kernel32.dll" || d == "kernelbase.dll";
        let mut total = 0;
        let mut modules = vec![("exe", GetModuleHandleW(std::ptr::null()))];
        for dll in ["libHttpClient.GDK.dll", "libHttpClient.dll", "XCurl.dll"] {
            let w: Vec<u16> = dll.encode_utf16().chain([0]).collect();
            let m = GetModuleHandleW(w.as_ptr());
            if !m.is_null() {
                modules.push((dll, m));
            }
        }
        for (label, m) in modules {
            // Only network DLLs are matched, except GetProcAddress (libraryloader/kernel32).
            let n = patch_module_imports(m, net, names, &[]);
            let o = patch_module_imports(m, |d| d.starts_with("ws2_32"), &[], ordinals);
            log(&format!("network: {} import(s) of {label} blocked", n + o));
            total += n + o;
        }
        log(&format!("network: offline, {total} import(s) blocked; TUXBOX_NETWORK=1 allows the title's network calls"));
    }
}
