//! XUser: one offline local user, signed in, never a guest, with every
//! privilege. Served as the user provider's objects from `QueryApiImpl`;
//! adding a user completes through `xasync` like any other async call. No
//! input device is associated with the user (there are none, `gameinput`).
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{c_char, c_void};

use victory_core::atomic;

use super::xasync::{self, AsyncBlock, ProviderData};
use super::xgameruntime::{
    Guid, HRESULT, Interface, Methods, Object, S_OK, add_ref, missing, put, put_string, text, token,
};

/// The user provider.
pub const PROVIDER: Guid = Guid {
    data1: 0x01ac_d177,
    data2: 0x91f9,
    data3: 0x4763,
    data4: [0xa3, 0x8e, 0xcc, 0xbb, 0x55, 0xce, 0x32, 0xe0],
};

/// Its main interface.
pub const IID: Guid = Guid {
    data1: 0xeb9b_f948,
    data2: 0x18dc,
    data3: 0x4d82,
    data4: [0xbb, 0xcc, 0x40, 0xe0, 0xa8, 0x09, 0xc4, 0xc0],
};

/// The interface titles ask for to read a gamertag.
pub const GAMERTAG_IID: Guid = Guid {
    data1: 0xcef4_fac0,
    data2: 0x7676,
    data3: 0x4a94,
    data4: [0xa1, 0x19, 0x4c, 0x43, 0xf9, 0xeb, 0x5b, 0x74],
};

/// The user's devices and audio endpoints.
pub const DEVICE_PROVIDER: Guid = Guid::new(
    0x7d82_4997,
    0x10dc,
    0x45ab,
    [0x86, 0xb7, 0x27, 0x37, 0x76, 0x7c, 0x0b, 0xf1],
);

/// The device interface as Minecraft (2022) asks for it.
pub const DEVICE_IID: Guid = Guid::new(
    0x0cc6_a956,
    0xe7e1,
    0x4fdf,
    [0x93, 0x41, 0x9d, 0x5d, 0xa6, 0x49, 0xeb, 0xc8],
);

/// The documented "no such user" error.
pub const USER_NOT_FOUND: HRESULT = 0x8924_5104_u32 as i32;

/// The id of the first local profile (kept from the time there was one fixed user, so its saves stay).
pub const XUID: u64 = 0x0009_0000_0000_0001;
pub const LOCAL_ID: u64 = 1;

/// A local profile: a gamertag and a stable id.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub gamertag: String,
    pub xuid: u64,
}

/// Parses `profiles.txt`: one profile per line, `<xuid hex> <gamertag>`; `#` starts a comment.
pub fn parse_profiles(text: &str) -> Vec<Profile> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let (id, name) = l.split_once(char::is_whitespace)?;
            let xuid = u64::from_str_radix(id.trim_start_matches("0x"), 16).ok()?;
            let gamertag: String = name.trim().chars().take(15).collect();
            (!gamertag.is_empty()).then_some(Profile { gamertag, xuid })
        })
        .collect()
}

/// A new profile's id: the first profile keeps [`XUID`], later ones get a hash of the name in the same range.
pub fn new_xuid(gamertag: &str, taken: &[Profile]) -> u64 {
    if taken.is_empty() {
        return XUID;
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in gamertag.to_lowercase().bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    let mut id = 0x0009_0000_0000_0000 | (h & 0x0000_ffff_ffff_fffe) | 2;
    while taken.iter().any(|p| p.xuid == id) {
        id += 1;
    }
    id
}

/// Picks the profile named `wanted` (any case), adding it when missing; else the first; else a new "Ziggy".
/// Returns the chosen profile and whether the list changed.
pub fn choose_profile(list: &mut Vec<Profile>, wanted: Option<&str>) -> (Profile, bool) {
    let name = wanted.map(str::trim).filter(|w| !w.is_empty());
    if let Some(name) = name {
        if let Some(p) = list.iter().find(|p| p.gamertag.eq_ignore_ascii_case(name)) {
            return (p.clone(), false);
        }
    } else if let Some(p) = list.first() {
        return (p.clone(), false);
    }
    let gamertag: String = name.unwrap_or("Ziggy").chars().take(15).collect();
    let p = Profile { xuid: new_xuid(&gamertag, list), gamertag };
    list.push(p.clone());
    (p, true)
}

fn profiles_file() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(std::path::Path::new(&base).join("VIctory").join("profiles.txt"))
}

/// The signed-in profile: `VICTORY_PROFILE` (a gamertag) from `%LOCALAPPDATA%\VIctory\profiles.txt`,
/// else the first profile there, else "Ziggy". New names are added to the file.
pub fn profile() -> &'static Profile {
    static P: std::sync::OnceLock<Profile> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let file = profiles_file();
        // The file is only ever replaced atomically, the previous one kept
        // as `.bak`; a missing, empty or damaged file reads as the `.bak`.
        let good = |d: &[u8]| std::str::from_utf8(d).is_ok_and(|t| !parse_profiles(t).is_empty());
        let found = file.as_ref().and_then(|f| atomic::read_good(f, good));
        if let (Some((_, atomic::Found::Backup)), Some(f)) = (&found, &file) {
            tracing::warn!(path = %f.display(), "the local profiles file was missing or damaged; using its last good copy");
        }
        let mut list = found
            .map(|(d, _)| parse_profiles(&String::from_utf8_lossy(&d)))
            .unwrap_or_default();
        let wanted = std::env::var("VICTORY_PROFILE").ok();
        let (p, changed) = choose_profile(&mut list, wanted.as_deref());
        if changed && let Some(f) = &file {
            let mut text = String::from("# VIctory local profiles: <xuid hex> <gamertag>. VICTORY_PROFILE=<gamertag> picks one.
");
            for q in &list {
                text += &format!("{:016X} {}
", q.xuid, q.gamertag);
            }
            if let Err(e) = atomic::replace(f, text.as_bytes()) {
                tracing::warn!(path = %f.display(), %e, "the local profiles could not be written");
            }
        }
        tracing::info!(gamertag = %p.gamertag, xuid = format!("{:016X}", p.xuid), "local profile");
        p
    })
}

/// The signed-in profile's id.
pub fn xuid() -> u64 {
    profile().xuid
}

/// The only user; a handle is its address, duplicating and closing are free.
pub struct User;
static USER: User = User;
type Handle = *const User;

fn user() -> Handle {
    &USER
}

fn found(found: bool, out: *mut Handle) -> HRESULT {
    if found {
        put(out, user())
    } else {
        USER_NOT_FOUND
    }
}

pub static OBJECT: Object = Object::new("user", &VTABLE);
pub static GAMERTAG_OBJECT: Object = Object::new("user gamertag", &GAMERTAG_VTABLE);
pub static DEVICE_OBJECT: Object = Object::new("user devices", &DEVICE_VTABLE);

pub const INTERFACES: &[Interface] = &[
    (PROVIDER, IID, &OBJECT),
    // The title (2026) asks with the provider's id; its thunks call
    // the same slots with the same arguments (0x108/0x110 change events).
    (PROVIDER, PROVIDER, &OBJECT),
    (PROVIDER, GAMERTAG_IID, &GAMERTAG_OBJECT),
    (DEVICE_PROVIDER, DEVICE_IID, &DEVICE_OBJECT),
];

type This = *const Object;
extern "system" fn duplicate(_: This, _: Handle, out: *mut Handle) -> HRESULT {
    put(out, user())
}
extern "system" fn close(_: This, _: Handle) {}
extern "system" fn compare(_: This, a: Handle, b: Handle) -> i32 {
    i32::from(a != b)
}

/// The async provider for adding a user: done at once, the result is a
/// handle.
extern "system" fn add_provider(op: u32, data: *const ProviderData) -> HRESULT {
    const BEGIN: u32 = 0;
    const GET_RESULT: u32 = 2;
    // SAFETY: the async runtime passes valid provider data.
    let data = unsafe { &*data };
    match op {
        BEGIN => xasync::XAsyncComplete(data.block, S_OK, size_of::<Handle>()),
        GET_RESULT => return put(data.buffer.cast::<Handle>(), user()),
        _ => {}
    }
    S_OK
}

/// Tells our add calls apart from other async calls on the same block.
static ADD: u8 = 0;

/// Any options add the one user, silently.
extern "system" fn add_async(_: This, options: u32, block: *mut AsyncBlock) -> HRESULT {
    // Options with the UI bit (the title's account picker) get the chosen profile too: it is the one
    // VICTORY_PROFILE names, picked before the run.
    tracing::info!(options, gamertag = %profile().gamertag, "XUserAddAsync: the local profile");
    xasync::XAsyncBegin(
        block,
        std::ptr::null_mut(),
        (&raw const ADD).cast(),
        std::ptr::null(),
        Some(add_provider),
    )
}
extern "system" fn add_result(_: This, block: *mut AsyncBlock, out: *mut Handle) -> HRESULT {
    xasync::XAsyncGetResult(
        block,
        (&raw const ADD).cast(),
        size_of::<Handle>(),
        out.cast(),
        std::ptr::null_mut(),
    )
}
extern "system" fn get_local_id(_: This, _: Handle, out: *mut u64) -> HRESULT {
    put(out, LOCAL_ID)
}
extern "system" fn find_by_local_id(_: This, id: u64, out: *mut Handle) -> HRESULT {
    found(id == LOCAL_ID, out)
}
extern "system" fn get_id(_: This, _: Handle, out: *mut u64) -> HRESULT {
    put(out, xuid())
}
extern "system" fn find_by_id(_: This, id: u64, out: *mut Handle) -> HRESULT {
    found(id == xuid(), out)
}
extern "system" fn get_is_guest(_: This, _: Handle, out: *mut bool) -> HRESULT {
    put(out, false)
}
/// 0: signed in.
extern "system" fn get_state(_: This, _: Handle, out: *mut u32) -> HRESULT {
    put(out, 0)
}
/// The user's gamertag as the component asked for (`XUserGamertagComponent`):
/// it has no suffix, so the modern suffix is empty and every other component
/// (classic, modern, unique modern) is the name itself.
extern "system" fn get_gamertag(
    _: This,
    _: Handle,
    component: u32,
    size: usize,
    buffer: *mut u8,
    used: *mut usize,
) -> HRESULT {
    const MODERN_SUFFIX: u32 = 2;
    let text = match component {
        MODERN_SUFFIX => "",
        _ => profile().gamertag.as_str(),
    };
    put_string(text, size, buffer, used)
}
/// Granted; deny reason 0 (none).
extern "system" fn check_privilege(
    _: This,
    _: Handle,
    _options: u32,
    _privilege: u32,
    has: *mut bool,
    reason: *mut u32,
) -> HRESULT {
    if !reason.is_null() {
        // SAFETY: checked non-null.
        unsafe { reason.write(0) };
    }
    put(has, true)
}

/// 3: adult.
extern "system" fn get_age_group(_: This, _: Handle, out: *mut u32) -> HRESULT {
    put(out, 3)
}

static RESOLVE: u8 = 0;
static TOKEN: u8 = 0;

/// Every privilege is granted, so there is nothing to resolve.
extern "system" fn resolve_privilege_async(
    _: This,
    _: Handle,
    _options: u32,
    _privilege: u32,
    block: *mut AsyncBlock,
) -> HRESULT {
    xasync::finished(block, (&raw const RESOLVE).cast(), S_OK)
}
extern "system" fn resolve_privilege_result(_: This, block: *mut AsyncBlock) -> HRESULT {
    let none = std::ptr::null_mut();
    xasync::XAsyncGetResult(block, (&raw const RESOLVE).cast(), 0, none, none.cast())
}

/// The documented "failed to get token": tokens come from the service.
pub const FAILED_TO_GET_TOKEN: HRESULT = 0x8924_5111_u32 as i32;

#[allow(clippy::too_many_arguments)]
extern "system" fn token_async(
    _: This,
    _: Handle,
    _options: u32,
    _method: *const c_char,
    url: *const c_char,
    _header_count: usize,
    _headers: *const c_void,
    _body_size: usize,
    _body: *const c_void,
    block: *mut AsyncBlock,
) -> HRESULT {
    tracing::info!(
        url = text(url).unwrap_or_default(),
        "token and signature: offline"
    );
    xasync::finished(block, (&raw const TOKEN).cast(), FAILED_TO_GET_TOKEN)
}
extern "system" fn token_size(_: This, block: *mut AsyncBlock, out: *mut usize) -> HRESULT {
    xasync::XAsyncGetResultSize(block, out)
}
extern "system" fn token_result(
    _: This,
    block: *mut AsyncBlock,
    size: usize,
    buffer: *mut c_void,
    _data: *mut *const c_void,
    used: *mut usize,
) -> HRESULT {
    xasync::XAsyncGetResult(block, (&raw const TOKEN).cast(), size, buffer, used)
}

/// The user never changes and no device is ever associated, so these
/// callbacks are never called.
extern "system" fn register(
    _: This,
    _queue: *const c_void,
    _context: *mut c_void,
    _callback: *const c_void,
    out: *mut u64,
) -> HRESULT {
    put(out, token())
}
extern "system" fn unregister(_: This, _token: u64, _wait: bool) -> bool {
    true
}

/// No input device belongs to a user.
extern "system" fn find_for_device(_: This, _device: *const c_void, _: *mut Handle) -> HRESULT {
    USER_NOT_FOUND
}

// Unset slots (no title seen calls them): maximum users, gamer pictures,
// the UTF-16 token calls, issue resolution.
static VTABLE: Methods<35> = Methods([
    missing::<0> as *const (),             // 0x00 QueryInterface
    add_ref as *const (),                  // 0x08 AddRef
    add_ref as *const (),                  // 0x10 Release: the object is static
    duplicate as *const (),                // 0x18
    close as *const (),                    // 0x20
    compare as *const (),                  // 0x28
    missing::<6> as *const (),             // 0x30 maximum users
    add_async as *const (),                // 0x38
    add_result as *const (),               // 0x40
    get_local_id as *const (),             // 0x48
    find_by_local_id as *const (),         // 0x50
    get_id as *const (),                   // 0x58
    find_by_id as *const (),               // 0x60
    get_is_guest as *const (),             // 0x68
    get_state as *const (),                // 0x70
    get_gamertag as *const (),             // 0x78 older gamertag slot
    missing::<16> as *const (),            // 0x80 gamer picture: begin
    missing::<17> as *const (),            // 0x88 result size
    missing::<18> as *const (),            // 0x90 result
    get_age_group as *const (),            // 0x98
    check_privilege as *const (),          // 0xa0
    resolve_privilege_async as *const (),  // 0xa8 with UI
    resolve_privilege_result as *const (), // 0xb0
    token_async as *const (),              // 0xb8 token and signature
    token_size as *const (),               // 0xc0
    token_result as *const (),             // 0xc8
    missing::<26> as *const (),            // 0xd0 the same, UTF-16: begin
    missing::<27> as *const (),            // 0xd8 result size
    missing::<28> as *const (),            // 0xe0 result
    missing::<29> as *const (),            // 0xe8 resolve issue with UI: begin
    missing::<30> as *const (),            // 0xf0 result
    missing::<31> as *const (),            // 0xf8 the same, UTF-16: begin
    missing::<32> as *const (),            // 0x100 result
    register as *const (),                 // 0x108 change event
    unregister as *const (),               // 0x110
]);

static GAMERTAG_VTABLE: Methods<4> = Methods([
    missing::<0> as *const (),
    add_ref as *const (),
    add_ref as *const (),
    get_gamertag as *const (), // 0x18
]);

static DEVICE_VTABLE: Methods<6> = Methods([
    missing::<0> as *const (),
    add_ref as *const (),
    add_ref as *const (),
    find_for_device as *const (), // 0x18
    register as *const (),        // 0x20 device association changed
    unregister as *const (),      // 0x28
]);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::xasync::E_NOT_SUFFICIENT_BUFFER;
    use crate::modules::xgameruntime::method as slot;

    #[test]
    fn profiles_are_read_chosen_and_added() {
        let mut list = parse_profiles("# c
0009000000000001 Ziggy

0009AB0000000002 Sam Two
bad
");
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].gamertag, "Sam Two");
        assert_eq!(choose_profile(&mut list, None), (list[0].clone(), false));
        assert_eq!(choose_profile(&mut list, Some("sam two")).0.xuid, 0x0009_AB00_0000_0002);
        let (p, changed) = choose_profile(&mut list, Some("New"));
        assert!(changed && list.len() == 3 && p.xuid != XUID && p.xuid >> 48 == 9);
        assert_eq!(choose_profile(&mut list, Some("New")), (p, false));
        let mut empty = Vec::new();
        let (z, changed) = choose_profile(&mut empty, None);
        assert!(changed);
        assert_eq!((z.gamertag.as_str(), z.xuid), ("Ziggy", XUID));
    }

    #[test]
    fn a_gamertag_without_a_suffix_has_an_empty_modern_suffix() {
        type Gamertag =
            extern "system" fn(This, Handle, u32, usize, *mut u8, *mut usize) -> HRESULT;
        let this: This = &OBJECT;
        let user = std::ptr::null();
        // Classic, modern, modern suffix, unique modern; through both slots.
        let tag = format!("{}\0", profile().gamertag);
        let expected: [&[u8]; 4] = [tag.as_bytes(), tag.as_bytes(), b"\0", tag.as_bytes()];
        for (object, at) in [(&GAMERTAG_OBJECT, 0x18), (&OBJECT, 0x78)] {
            let get = slot::<Gamertag>(object, at);
            for (component, expected) in (0..).zip(expected) {
                let (mut name, mut used) = ([0xff_u8; 32], 0);
                let hr = get(this, user, component, 32, name.as_mut_ptr(), &mut used);
                assert_eq!(hr, S_OK);
                assert_eq!(&name[..used], expected, "component {component}");
            }
        }
    }

    #[test]
    fn startup_adds_the_user_and_reads_its_id() {
        type Add = extern "system" fn(This, u32, *mut AsyncBlock) -> HRESULT;
        type AddResult = extern "system" fn(This, *mut AsyncBlock, *mut Handle) -> HRESULT;
        type GetId = extern "system" fn(This, Handle, *mut u64) -> HRESULT;
        type Gamertag =
            extern "system" fn(This, Handle, u32, usize, *mut u8, *mut usize) -> HRESULT;
        let this: This = &OBJECT;
        let mut block = AsyncBlock {
            queue: std::ptr::null(),
            context: std::ptr::null_mut(),
            callback: None,
            internal: [0; 4],
        };
        assert_eq!(slot::<Add>(&OBJECT, 0x38)(this, 1, &mut block), S_OK);
        assert_eq!(xasync::XAsyncGetStatus(&mut block, true), S_OK);
        let mut handle = std::ptr::null();
        assert_eq!(
            slot::<AddResult>(&OBJECT, 0x40)(this, &mut block, &mut handle),
            S_OK
        );
        assert!(!handle.is_null());
        let mut xuid = 0;
        assert_eq!(slot::<GetId>(&OBJECT, 0x58)(this, handle, &mut xuid), S_OK);
        assert_eq!(xuid, super::xuid());

        let mut name = [0u8; 16];
        let mut used = 0;
        let get = slot::<Gamertag>(&GAMERTAG_OBJECT, 0x18);
        assert_eq!(get(this, handle, 0, 16, name.as_mut_ptr(), &mut used), S_OK);
        assert_eq!(&name[..used], format!("{}\0", profile().gamertag).as_bytes());
        assert_eq!(
            get(this, handle, 0, 2, name.as_mut_ptr(), &mut used),
            E_NOT_SUFFICIENT_BUFFER
        );

        type Age = extern "system" fn(This, Handle, *mut u32) -> HRESULT;
        let mut age = 0;
        assert_eq!(slot::<Age>(&OBJECT, 0x98)(this, handle, &mut age), S_OK);
        assert_eq!(age, 3, "adult");
        type Token = extern "system" fn(
            This,
            Handle,
            u32,
            *const c_char,
            *const c_char,
            usize,
            *const c_void,
            usize,
            *const c_void,
            *mut AsyncBlock,
        ) -> HRESULT;
        let (null, url) = (std::ptr::null(), c"https://example.invalid/");
        let token = slot::<Token>(&OBJECT, 0xb8);
        assert_eq!(
            token(
                this,
                handle,
                0,
                c"GET".as_ptr(),
                url.as_ptr(),
                0,
                null,
                0,
                null,
                &mut block
            ),
            S_OK
        );
        assert_eq!(
            xasync::XAsyncGetStatus(&mut block, true),
            FAILED_TO_GET_TOKEN
        );
    }
}
