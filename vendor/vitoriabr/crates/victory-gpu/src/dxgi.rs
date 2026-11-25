//! The DXGI side of the device: one adapter with one output, reached from
//! the device by interface query. Tables follow the PC interfaces' order
//! (IUnknown, the object methods, then each interface's methods).
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::object::{
    self, ADAPTER, DXGI_DEVICE, E_INVALIDARG, FACTORY, HRESULT, Iid, MORE_DATA, NOT_FOUND, OUTPUT,
    Object, S_OK, Table, This, guid,
};

/// The DXGI device interfaces, base first (as on PC).
pub(crate) const DEVICE_IIDS: [Iid; 3] = [
    guid(
        0x54ec77fa,
        0x1377,
        0x44e6,
        [0x8c, 0x32, 0x88, 0xfd, 0x5f, 0x44, 0xc8, 0x4c],
    ),
    guid(
        0x77db970f,
        0x6276,
        0x48ba,
        [0xba, 0x28, 0x07, 0x01, 0x43, 0xb4, 0x39, 0x2c],
    ),
    guid(
        0x05008617,
        0xfbfd,
        0x4051,
        [0xa7, 0x90, 0x14, 0x48, 0x84, 0xb4, 0xf6, 0xa9],
    ),
];
const ADAPTER_IIDS: [Iid; 3] = [
    guid(
        0x2411e7e1,
        0x12ac,
        0x4ccf,
        [0xbd, 0x14, 0x97, 0x98, 0xe8, 0x53, 0x4d, 0xc0],
    ),
    guid(
        0x29038f61,
        0x3839,
        0x4626,
        [0x91, 0xfd, 0x08, 0x68, 0x79, 0x01, 0x1a, 0x05],
    ),
    guid(
        0x0aa1ae0a,
        0xfa0e,
        0x4b84,
        [0x86, 0x44, 0xe0, 0x5f, 0xf8, 0xe5, 0xac, 0xb5],
    ),
];
const FACTORY_IIDS: [Iid; 3] = [
    guid(
        0x7b7166ec,
        0x21c7,
        0x44ae,
        [0xb2, 0x1a, 0xc9, 0xae, 0x32, 0x1a, 0xe3, 0x69],
    ),
    guid(
        0x770aae78,
        0xf26f,
        0x4dba,
        [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
    ),
    guid(
        0x50c83a1c,
        0xe072,
        0x4c48,
        [0x87, 0xb0, 0x36, 0x30, 0xfa, 0x36, 0xa6, 0xd0],
    ),
];
const OUTPUT_IIDS: [Iid; 1] = [guid(
    0xae02eedb,
    0xc735,
    0x4690,
    [0x8d, 0x52, 0x5a, 0x8d, 0xc2, 0x02, 0x13, 0xaa],
)];

static DEVICE: Object = Object::new(&DEVICE_FUNCTIONS);
static ADAPTER_OBJECT: Object = Object::new(&ADAPTER_FUNCTIONS);
static FACTORY_OBJECT: Object = Object::new(&FACTORY_FUNCTIONS);
static OUTPUT_OBJECT: Object = Object::new(&OUTPUT_FUNCTIONS);

pub(crate) fn device() -> This {
    &DEVICE
}

/// `found` if `iid` is one of `known`.
fn query(kind: usize, iid: *const Iid, known: &[Iid], found: This, out: *mut This) -> HRESULT {
    object::answer(kind, iid, object::is(iid, known).then_some(found), out)
}

/// The DXGI device also answers for the D3D12 device.
extern "system" fn device_query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    if crate::device::is_device(iid) {
        return object::answer(DXGI_DEVICE, iid, Some(crate::device::device()), out);
    }
    query(DXGI_DEVICE, iid, &DEVICE_IIDS, this, out)
}
extern "system" fn device_parent(_: This, iid: *const Iid, out: *mut This) -> HRESULT {
    query(DXGI_DEVICE, iid, &ADAPTER_IIDS, &ADAPTER_OBJECT, out)
}
extern "system" fn device_adapter(_: This, out: *mut This) -> HRESULT {
    object::answer(DXGI_DEVICE, &ADAPTER_IIDS[0], Some(&ADAPTER_OBJECT), out)
}

extern "system" fn adapter_query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    query(ADAPTER, iid, &ADAPTER_IIDS, this, out)
}
extern "system" fn adapter_parent(_: This, iid: *const Iid, out: *mut This) -> HRESULT {
    query(ADAPTER, iid, &FACTORY_IIDS, &FACTORY_OBJECT, out)
}
extern "system" fn adapter_outputs(_: This, index: u32, out: *mut This) -> HRESULT {
    if index != 0 {
        return NOT_FOUND;
    }
    object::answer(ADAPTER, &OUTPUT_IIDS[0], Some(&OUTPUT_OBJECT), out)
}

#[repr(C)]
struct AdapterDescription {
    name: [u16; 128],
    vendor: u32,
    device: u32,
    subsystem: u32,
    revision: u32,
    dedicated_video_memory: usize,
    dedicated_system_memory: usize,
    shared_system_memory: usize,
    luid: [u32; 2],
}
const _: () = assert!(size_of::<AdapterDescription>() == 0x130);

const AMD: u32 = 0x1002;

extern "system" fn adapter_description(_: This, out: *mut AdapterDescription) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let mut name = [0; 128];
    for (slot, unit) in name.iter_mut().zip("VItoriaBr console GPU".encode_utf16()) {
        *slot = unit;
    }
    // shortcut: memory sizes are the title's split (GPU-optimal, standard),
    // not measured; revisit once memory is emulated.
    let (gpu_optimal, standard) = crate::console::current().title_memory();
    let description = AdapterDescription {
        name,
        vendor: AMD,
        device: 0,
        subsystem: 0,
        revision: 0,
        dedicated_video_memory: gpu_optimal,
        dedicated_system_memory: 0,
        shared_system_memory: standard,
        luid: [1, 0],
    };
    // SAFETY: checked non-null; the title passes room for the description.
    unsafe { out.write(description) };
    S_OK
}

extern "system" fn factory_query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    query(FACTORY, iid, &FACTORY_IIDS, this, out)
}
extern "system" fn factory_adapters(_: This, index: u32, out: *mut This) -> HRESULT {
    if index != 0 {
        return NOT_FOUND;
    }
    object::answer(FACTORY, &ADAPTER_IIDS[0], Some(&ADAPTER_OBJECT), out)
}

extern "system" fn output_query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    query(OUTPUT, iid, &OUTPUT_IIDS, this, out)
}
extern "system" fn output_parent(_: This, iid: *const Iid, out: *mut This) -> HRESULT {
    query(OUTPUT, iid, &ADAPTER_IIDS, &ADAPTER_OBJECT, out)
}

/// IUnknown 3, object 4, device 5 + 2 + 3.
static DEVICE_FUNCTIONS: Table<32> = {
    let mut f = object::missing_table::<DXGI_DEVICE, 32>();
    f[0] = device_query as *const ();
    f[6] = device_parent as *const (); // 0x30
    f[7] = device_adapter as *const (); // 0x38
    Table(f)
};

/// IUnknown 3, object 4, adapter 3 + 1 + 1.
static ADAPTER_FUNCTIONS: Table<24> = {
    let mut f = object::missing_table::<ADAPTER, 24>();
    f[0] = adapter_query as *const ();
    f[6] = adapter_parent as *const (); // 0x30
    f[7] = adapter_outputs as *const (); // 0x38
    f[8] = adapter_description as *const (); // 0x40
    Table(f)
};

/// IUnknown 3, object 4, factory 5 + 2 + 11.
static FACTORY_FUNCTIONS: Table<32> = {
    let mut f = object::missing_table::<FACTORY, 32>();
    f[0] = factory_query as *const ();
    f[7] = factory_adapters as *const (); // 0x38
    f[12] = factory_adapters as *const (); // 0x60, the same for version 1
    Table(f)
};

/// A display mode as the console lists it: size, refresh rate (numerator,
/// denominator), format, scanline order, scaling, flags.
#[repr(C)]
struct Mode {
    width: u32,
    height: u32,
    refresh: [u32; 2],
    format: u32,
    scanlines: u32,
    scaling: u32,
    flags: u32,
}
const _: () = assert!(size_of::<Mode>() == 32);

/// The size of the display the console reports: `VICTORY_DISPLAY_MODE=WxH`
/// if set (as a console reports the television it is plugged into: a
/// Series X on a 1080p set lists a 1080p mode), else 3840x2160.
/// shortcut: not yet the host display's own size by default; make it so
/// once a title is seen to render for the mode it is given.
fn display_size() -> (u32, u32) {
    static SIZE: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();
    *SIZE.get_or_init(|| {
        let asked = std::env::var("VICTORY_DISPLAY_MODE").ok().and_then(|v| {
            let (w, h) = v.trim().split_once(['x', 'X'])?;
            Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?))
        });
        let size = match asked {
            Some((w, h)) if (640..=7680).contains(&w) && (360..=4320).contains(&h) => (w, h),
            _ => (3840, 2160),
        };
        tracing::info!(width = size.0, height = size.1, "display mode reported to the title");
        size
    })
}

/// The console's mode list: one 4K mode at 60 Hz (the title looks
/// for a 120 Hz one to allow 120 Hz frame intervals).
/// shortcut: fixed mode, not the host display's; report the host's rates if
/// a title paces by them.
extern "system" fn modes(
    _: This,
    _format: u32,
    _flags: u32,
    count: *mut u32,
    out: *mut Mode,
) -> HRESULT {
    // SAFETY: the title passes its count (null is tolerated).
    let Some(count) = (unsafe { count.as_mut() }) else {
        return E_INVALIDARG;
    };
    if out.is_null() {
        *count = 1;
        return S_OK;
    }
    if *count < 1 {
        return MORE_DATA;
    }
    *count = 1;
    let (width, height) = display_size();
    let mode = Mode {
        width,
        height,
        refresh: [60, 1],
        format: 0,
        scanlines: 1, // progressive
        scaling: 0,
        flags: 0,
    };
    // SAFETY: checked non-null; room for `count` >= 1 modes.
    unsafe { out.write(mode) };
    S_OK
}

/// IUnknown 3, object 4, output 15 (12 as on PC, then 3 console ones).
static OUTPUT_FUNCTIONS: Table<32> = {
    let mut f = object::missing_table::<OUTPUT, 32>();
    f[0] = output_query as *const ();
    f[6] = output_parent as *const (); // 0x30
    f[21] = modes as *const (); // 0xa8
    Table(f)
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{E_NOINTERFACE, slot};

    type Query = extern "system" fn(This, *const Iid, *mut This) -> HRESULT;
    type Get = extern "system" fn(This, *mut This) -> HRESULT;
    type Indexed = extern "system" fn(This, u32, *mut This) -> HRESULT;
    type Describe = extern "system" fn(This, *mut AdapterDescription) -> HRESULT;

    /// Minecraft's startup: device → DXGI device (newest first) → adapter →
    /// description, factory, first output.
    #[test]
    fn startup_walks_from_device_to_output() {
        let d3d = crate::device::device();
        let newest_pc = guid(
            0x6007896c,
            0x3244,
            0x4afd,
            [0xbf, 0x18, 0xa6, 0xd3, 0xbe, 0xda, 0x50, 0x23],
        );
        let mut dxgi = std::ptr::null();
        let qi = slot::<Query>(d3d, 0);
        assert_eq!(qi(d3d, &newest_pc, &mut dxgi), E_NOINTERFACE);
        assert!(dxgi.is_null());
        assert_eq!(qi(d3d, &DEVICE_IIDS[2], &mut dxgi), S_OK);
        assert_eq!(dxgi, device());

        let mut adapter = std::ptr::null();
        assert_eq!(slot::<Get>(dxgi, 0x38)(dxgi, &mut adapter), S_OK);
        let mut description = std::mem::MaybeUninit::<AdapterDescription>::uninit();
        assert_eq!(
            slot::<Describe>(adapter, 0x40)(adapter, description.as_mut_ptr()),
            S_OK
        );
        // SAFETY: written above.
        assert_eq!(unsafe { description.assume_init() }.vendor, AMD);

        let mut factory = std::ptr::null();
        let parent = slot::<Query>(adapter, 0x30);
        assert_eq!(parent(adapter, &FACTORY_IIDS[2], &mut factory), S_OK);
        let mut again = std::ptr::null();
        assert_eq!(slot::<Indexed>(factory, 0x38)(factory, 0, &mut again), S_OK);
        assert_eq!(again, adapter);

        let mut output = std::ptr::null();
        let outputs = slot::<Indexed>(adapter, 0x38);
        assert_eq!(outputs(adapter, 0, &mut output), S_OK);
        assert!(!output.is_null());
        assert_eq!(outputs(adapter, 1, &mut output), NOT_FOUND);
    }
}
