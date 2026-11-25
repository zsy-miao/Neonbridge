//! XNetworking as an offline console answers it: the network stack is up
//! (titles wait for that before anything else) but reaches nothing, the
//! connectivity hint never changes, and the title's security allow list,
//! which comes from the service, cannot be fetched.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{c_char, c_void};

use super::xasync::{self, AsyncBlock};
use super::xgameruntime::{
    Guid, HRESULT, Interface, Methods, Object, add_ref, missing, put, text, token,
};

pub const PROVIDER: Guid = Guid::new(
    0x37e5_6907,
    0x2f10,
    0x41e8,
    [0xb7, 0x2f, 0x36, 0xed, 0xb1, 0x85, 0x33, 0x1a],
);

/// As Grand Theft Auto VI (2026) asks for it; the slots it calls (0x18-0x88)
/// take the same arguments as under the provider's own id (older titles).
pub const IID_2026: Guid = Guid::new(
    0xbf23_46b2,
    0x39af,
    0x4658,
    [0xb5, 0xea, 0x44, 0x71, 0x3c, 0x7e, 0x83, 0xb3],
);

pub static OBJECT: Object = Object::new("networking", &VTABLE);
pub const INTERFACES: &[Interface] =
    &[(PROVIDER, PROVIDER, &OBJECT), (PROVIDER, IID_2026, &OBJECT)];

/// `HRESULT_FROM_WIN32(ERROR_NETWORK_UNREACHABLE)`: what needs the service.
pub const UNREACHABLE: HRESULT = 0x8007_04CF_u32 as i32;

/// The documented default for the preferred local UDP multiplayer port.
const UDP_PORT: u16 = 3074;

/// The connectivity hint: level, cost, interface type, then four flags.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectivityHint {
    pub level: u32,
    pub cost: u32,
    pub interface_type: u32,
    pub initialized: bool,
    pub approaching_data_limit: bool,
    pub over_data_limit: bool,
    pub roaming: bool,
}

/// Initialized, no connectivity (level 1), cost unknown (0).
const OFFLINE: ConnectivityHint = ConnectivityHint {
    level: 1,
    cost: 0,
    interface_type: 0,
    initialized: true,
    approaching_data_limit: false,
    over_data_limit: false,
    roaming: false,
};

type This = *const Object;

extern "system" fn udp_port(_: This, out: *mut u16) -> HRESULT {
    put(out, UDP_PORT)
}

/// Tells our port queries apart from other calls on the same block.
static PORT: u8 = 0;

extern "system" fn udp_port_async(_: This, block: *mut AsyncBlock) -> HRESULT {
    xasync::finished_with(block, (&raw const PORT).cast(), Ok(xasync::bytes(UDP_PORT)))
}
extern "system" fn udp_port_result(_: This, block: *mut AsyncBlock, out: *mut u16) -> HRESULT {
    xasync::result(block, (&raw const PORT).cast(), out)
}

static SECURITY: u8 = 0;

/// The allow list comes from the service: unreachable offline.
extern "system" fn security_info_async(
    _: This,
    url: *const c_char,
    block: *mut AsyncBlock,
) -> HRESULT {
    tracing::debug!(
        url = text(url).unwrap_or_default(),
        "security information: offline"
    );
    xasync::finished(block, (&raw const SECURITY).cast(), UNREACHABLE)
}
extern "system" fn security_info_size(_: This, block: *mut AsyncBlock, out: *mut usize) -> HRESULT {
    xasync::XAsyncGetResultSize(block, out)
}
extern "system" fn security_info_result(
    _: This,
    block: *mut AsyncBlock,
    size: usize,
    used: *mut usize,
    buffer: *mut u8,
    _info: *mut *const c_void,
) -> HRESULT {
    xasync::XAsyncGetResult(
        block,
        (&raw const SECURITY).cast(),
        size,
        buffer.cast(),
        used,
    )
}

/// No security information can exist offline, so no certificate passes.
extern "system" fn verify_certificate(
    _: This,
    _request: *mut c_void,
    _info: *const c_void,
) -> HRESULT {
    UNREACHABLE
}

extern "system" fn connectivity_hint(_: This, out: *mut ConnectivityHint) -> HRESULT {
    put(out, OFFLINE)
}

/// The hint never changes offline, so the callback never runs.
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

static VTABLE: Methods<18> = Methods([
    missing::<0> as *const (),
    add_ref as *const (),
    add_ref as *const (),
    udp_port as *const (),             // 0x18
    udp_port_async as *const (),       // 0x20
    udp_port_result as *const (),      // 0x28
    missing::<6> as *const (),         // 0x30 port changed
    missing::<7> as *const (),         // 0x38
    security_info_async as *const (),  // 0x40
    security_info_size as *const (),   // 0x48
    security_info_result as *const (), // 0x50
    missing::<11> as *const (),        // 0x58 the same for UTF-16 URLs
    missing::<12> as *const (),        // 0x60
    missing::<13> as *const (),        // 0x68
    verify_certificate as *const (),   // 0x70
    connectivity_hint as *const (),    // 0x78
    register as *const (),             // 0x80 hint changed (never)
    unregister as *const (),           // 0x88
]);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::xgameruntime::{S_OK, method};

    fn block() -> AsyncBlock {
        AsyncBlock {
            queue: std::ptr::null(),
            context: std::ptr::null_mut(),
            callback: None,
            internal: [0; 4],
        }
    }

    #[test]
    fn offline_but_initialized() {
        let this: This = &OBJECT;
        type Hint = extern "system" fn(This, *mut ConnectivityHint) -> HRESULT;
        let mut hint = OFFLINE;
        hint.initialized = false;
        assert_eq!(method::<Hint>(&OBJECT, 0x78)(this, &mut hint), S_OK);
        assert!(hint.initialized && hint.level == 1);
        assert_eq!(size_of::<ConnectivityHint>(), 16);

        type Begin = extern "system" fn(This, *mut AsyncBlock) -> HRESULT;
        type Port = extern "system" fn(This, *mut AsyncBlock, *mut u16) -> HRESULT;
        let mut b = block();
        assert_eq!(method::<Begin>(&OBJECT, 0x20)(this, &mut b), S_OK);
        assert_eq!(xasync::XAsyncGetStatus(&mut b, true), S_OK);
        let mut port = 0;
        assert_eq!(method::<Port>(&OBJECT, 0x28)(this, &mut b, &mut port), S_OK);
        assert_eq!(port, 3074);

        type Url = extern "system" fn(This, *const c_char, *mut AsyncBlock) -> HRESULT;
        type Size = extern "system" fn(This, *mut AsyncBlock, *mut usize) -> HRESULT;
        let mut b = block();
        let url = c"https://example.invalid/";
        assert_eq!(
            method::<Url>(&OBJECT, 0x40)(this, url.as_ptr(), &mut b),
            S_OK
        );
        assert_eq!(xasync::XAsyncGetStatus(&mut b, true), UNREACHABLE);
        let mut size = 0;
        assert_eq!(
            method::<Size>(&OBJECT, 0x48)(this, &mut b, &mut size),
            UNREACHABLE
        );
    }
}
