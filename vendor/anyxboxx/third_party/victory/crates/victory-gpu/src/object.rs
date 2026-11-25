//! What every console graphics object starts with: a function table
//! pointer, bookkeeping bits and a reference count. Tables begin with
//! IUnknown's three slots. Our objects are static and never destroyed.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub type HRESULT = i32;
pub const S_OK: HRESULT = 0;
pub(crate) const S_FALSE: HRESULT = 1;
pub(crate) const E_NOTIMPL: HRESULT = 0x8000_4001_u32 as i32;
pub(crate) const E_NOINTERFACE: HRESULT = 0x8000_4002_u32 as i32;
pub(crate) const E_POINTER: HRESULT = 0x8000_4003_u32 as i32;
pub(crate) const E_INVALIDARG: HRESULT = 0x8007_0057_u32 as i32;
pub(crate) const E_OUTOFMEMORY: HRESULT = 0x8007_000E_u32 as i32;
/// What D3D and DXGI return for a missing item (adapter, output, private
/// data).
pub(crate) const NOT_FOUND: HRESULT = 0x887A_0002_u32 as i32;
pub(crate) const MORE_DATA: HRESULT = 0x887A_0003_u32 as i32;

pub type Iid = [u8; 16];

/// An interface id from its text form's fields, in memory order.
pub const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> Iid {
    let (a, b, c) = (a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes());
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6],
        d[7],
    ]
}

pub fn guid_text(g: &Iid) -> String {
    let a = u32::from_le_bytes([g[0], g[1], g[2], g[3]]);
    let b = u16::from_le_bytes([g[4], g[5]]);
    let c = u16::from_le_bytes([g[6], g[7]]);
    let d: String = g[8..].iter().map(|x| format!("{x:02x}")).collect();
    format!("{a:08x}-{b:04x}-{c:04x}-{}-{}", &d[..4], &d[4..])
}

#[repr(C)]
pub struct Object {
    functions: *const *const (),
    /// Device index (bits 0-2) and bookkeeping bits; all zero.
    bits: u32,
    refs: AtomicU32,
}
// SAFETY: the table is static and immutable; the count is atomic.
unsafe impl Sync for Object {}

impl Object {
    /// The object's function table.
    pub(crate) fn table(&self) -> *const *const () {
        self.functions
    }

    pub(crate) const fn new<const N: usize>(table: &'static Table<N>) -> Self {
        Object {
            functions: table.0.as_ptr(),
            bits: 0,
            refs: AtomicU32::new(0),
        }
    }
}

pub(crate) type This = *const Object;

#[repr(transparent)]
pub(crate) struct Table<const N: usize>(pub [*const (); N]);
// SAFETY: function addresses only.
unsafe impl<const N: usize> Sync for Table<N> {}

pub(crate) extern "system" fn add_ref(this: This) -> u32 {
    // SAFETY: `this` is one of our objects.
    unsafe { &*this }.refs.fetch_add(1, Ordering::Relaxed) + 1
}

pub(crate) extern "system" fn release(this: This) -> u32 {
    // SAFETY: `this` is one of our objects.
    let refs = &unsafe { &*this }.refs;
    let _ = refs.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
    refs.load(Ordering::Relaxed)
}

/// Answers an interface query (or any call handing out an object): a new
/// reference to `found`, or null and `E_NOINTERFACE`.
pub(crate) fn answer(kind: usize, iid: *const Iid, found: Option<This>, out: *mut This) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    let hr = match found {
        Some(object) => {
            add_ref(object);
            // SAFETY: checked non-null; the title passes room for a pointer.
            unsafe { out.write(object) };
            return S_OK;
        }
        None => E_NOINTERFACE,
    };
    // SAFETY: the title passes an interface id (null is tolerated).
    let iid = unsafe { iid.as_ref() }.map(guid_text).unwrap_or_default();
    tracing::warn!(object = KINDS[kind], %iid, "unknown interface");
    // SAFETY: checked non-null.
    unsafe { out.write(std::ptr::null()) };
    hr
}

/// Whether `iid` is one of `known`.
pub(crate) fn is(iid: *const Iid, known: &[Iid]) -> bool {
    // SAFETY: the title passes an interface id (null is tolerated).
    unsafe { iid.as_ref() }.is_some_and(|i| known.contains(i))
}

/// Hands a new object to the title as one of `known`, or drops it. `T`
/// starts with its `Object`; its table's release slot is
/// [`release_boxed::<T>`].
pub(crate) fn hand_out<T>(
    kind: usize,
    object: Box<T>,
    iid: *const Iid,
    known: &[Iid],
    out: *mut This,
) -> HRESULT {
    if !is(iid, known) {
        return answer(kind, iid, None, out);
    }
    if out.is_null() {
        // Asked only whether creation would succeed.
        return S_FALSE;
    }
    answer(kind, iid, Some(Box::into_raw(object).cast()), out)
}

/// Release for objects made by [`hand_out`]: the last reference frees it.
pub(crate) fn release_boxed<T>(this: This) -> u32 {
    // SAFETY: `this` is the header of a boxed `T`.
    let refs = &unsafe { &*this }.refs;
    match refs.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1)) {
        Ok(1) => {
            private_data().retain(|&(object, _), _| object != this as usize);
            // SAFETY: that was the last reference to a `Box<T>`.
            drop(unsafe { Box::from_raw(this.cast_mut().cast::<T>()) });
            0
        }
        Ok(n) => n - 1,
        Err(_) => 0,
    }
}

/// Private data of every object, by object address and id.
static PRIVATE: Mutex<BTreeMap<(usize, Iid), Vec<u8>>> = Mutex::new(BTreeMap::new());

fn private_data() -> std::sync::MutexGuard<'static, BTreeMap<(usize, Iid), Vec<u8>>> {
    PRIVATE.lock().unwrap_or_else(|e| e.into_inner())
}

extern "system" fn get_private_data(
    this: This,
    id: *const Iid,
    size: *mut u32,
    data: *mut u8,
) -> HRESULT {
    // SAFETY: the title passes an id and its buffer size (null is tolerated).
    let (Some(id), Some(size)) = (unsafe { (id.as_ref(), size.as_mut()) }) else {
        return E_INVALIDARG;
    };
    let map = private_data();
    let Some(value) = map.get(&(this as usize, *id)) else {
        *size = 0;
        return NOT_FOUND;
    };
    let len = value.len() as u32;
    let room = std::mem::replace(size, len);
    if data.is_null() {
        return S_OK;
    }
    if room < len {
        return MORE_DATA;
    }
    // SAFETY: the buffer holds `room` >= `len` bytes.
    unsafe { data.copy_from_nonoverlapping(value.as_ptr(), value.len()) };
    S_OK
}

/// Null data removes the entry.
extern "system" fn set_private_data(
    this: This,
    id: *const Iid,
    size: u32,
    data: *const u8,
) -> HRESULT {
    // SAFETY: the title passes an id (null is tolerated).
    let Some(&id) = (unsafe { id.as_ref() }) else {
        return E_INVALIDARG;
    };
    let mut map = private_data();
    if data.is_null() {
        map.remove(&(this as usize, id));
    } else {
        // SAFETY: the title passes `size` bytes.
        let value = unsafe { std::slice::from_raw_parts(data, size as usize) };
        map.insert((this as usize, id), value.to_vec());
    }
    S_OK
}

/// Fills a D3D12 table's object methods: private data and name (setting
/// an interface as private data stays unwritten).
pub(crate) const fn d3d12_object<const N: usize>(mut f: [*const (); N]) -> [*const (); N] {
    f[3] = get_private_data as *const (); // 0x18
    f[4] = set_private_data as *const (); // 0x20
    f[6] = set_name as *const (); // 0x30
    f
}

/// The object methods' name setter: names only reach our log.
pub(crate) extern "system" fn set_name(this: This, name: *const u16) -> HRESULT {
    let name = if name.is_null() {
        String::new()
    } else {
        // SAFETY: a non-null name is NUL-terminated UTF-16.
        let len = (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count();
        // SAFETY: `len` units were just read.
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) })
    };
    tracing::debug!(object = ?this, %name, "named");
    S_OK
}

/// The device-child method: the device that made the object, as any of
/// the interfaces the device itself answers to.
pub(crate) extern "system" fn parent_device(_: This, iid: *const Iid, out: *mut This) -> HRESULT {
    let device = crate::device::device();
    answer(
        DEVICE,
        iid,
        crate::device::is_device(iid).then_some(device),
        out,
    )
}

pub(crate) const DEVICE: usize = 0;
pub(crate) const DXGI_DEVICE: usize = 1;
pub(crate) const ADAPTER: usize = 2;
pub(crate) const FACTORY: usize = 3;
pub(crate) const OUTPUT: usize = 4;
pub(crate) const HEAP: usize = 5;
pub(crate) const RESOURCE: usize = 6;
pub(crate) const FENCE: usize = 7;
pub(crate) const QUEUE: usize = 8;
pub(crate) const ALLOCATOR: usize = 9;
pub(crate) const LIST: usize = 10;
pub(crate) const DESCRIPTOR_HEAP: usize = 11;
pub(crate) const ROOT_SIGNATURE: usize = 12;
pub(crate) const QUERY_HEAP: usize = 13;
pub(crate) const COMMAND_SIGNATURE: usize = 14;
pub(crate) const PIPELINE: usize = 15;
const KINDS: [&str; 16] = [
    "device",
    "DXGI device",
    "adapter",
    "factory",
    "output",
    "heap",
    "resource",
    "fence",
    "command queue",
    "command allocator",
    "command list",
    "descriptor heap",
    "root signature",
    "query heap",
    "command signature",
    "pipeline state",
];

const MAX_SLOTS: usize = 160;

/// Which unwritten slots have been reported; each is logged once.
static REPORTED: [[AtomicBool; MAX_SLOTS]; KINDS.len()] =
    [const { [const { AtomicBool::new(false) }; MAX_SLOTS] }; KINDS.len()];

/// Logged once per slot, with the caller's offset in its module (the title
/// image for title calls). Wrong for a slot that returns a value instead of
/// an `HRESULT`.
extern "system" fn missing<const KIND: usize, const SLOT: usize>() -> HRESULT {
    if !REPORTED[KIND][SLOT].swap(true, Ordering::Relaxed) {
        tracing::warn!(
            object = KINDS[KIND],
            slot = format_args!("{:#x}", SLOT * 8),
            caller = caller(),
            "method not implemented"
        );
    }
    E_NOTIMPL
}

/// The return address of whoever called our caller, as `+offset` into its
/// module.
#[inline(never)]
fn caller() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
        use windows_sys::Win32::System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            GetModuleHandleExW,
        };
        let mut frame = std::ptr::null_mut();
        // SAFETY: room for one frame; skips this function and its caller.
        if unsafe { RtlCaptureStackBackTrace(2, 1, &mut frame, std::ptr::null_mut()) } == 1 {
            let mut module = std::ptr::null_mut();
            // SAFETY: looks up the module holding the address, no reference.
            let found = unsafe {
                GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                        | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                    frame as *const u16,
                    &mut module,
                )
            };
            if found != 0 {
                return format!("+{:#x}", frame as usize - module as usize);
            }
            return format!("{frame:?}");
        }
    }
    String::new()
}

/// Accepts and ignores (debug-only settings).
pub(crate) extern "system" fn ignore() -> HRESULT {
    S_OK
}

macro_rules! missing {
    ($kind:ident; $($slot:literal)*) => { [$(missing::<$kind, $slot> as *const ()),*] };
}

/// A table of `N` slots that log and fail; callers fill in what they have.
/// Tables are made longer than the interfaces we know, so that a method a
/// newer title adds logs instead of jumping into whatever follows the table.
pub(crate) const fn missing_table<const KIND: usize, const N: usize>() -> [*const (); N] {
    let all = missing!(KIND;
        0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31
        32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60
        61 62 63 64 65 66 67 68 69 70 71 72 73 74 75 76 77 78 79 80 81 82 83 84 85 86 87 88 89
        90 91 92 93 94 95 96 97 98 99 100 101 102 103 104 105 106 107 108 109 110 111 112 113
        114 115 116 117 118 119 120 121 122 123 124 125 126 127 128 129 130 131 132 133 134 135
        136 137 138 139 140 141 142 143 144 145 146 147 148 149 150 151 152 153 154 155 156 157
        158 159
    );
    assert!(N <= all.len());
    let mut table = [std::ptr::null(); N];
    let mut i = 0;
    while i < N {
        table[i] = all[i];
        i += 1;
    }
    table[1] = add_ref as *const ();
    table[2] = release as *const ();
    table
}

#[cfg(test)]
pub(crate) fn slot<F: Copy>(object: This, offset: usize) -> F {
    // SAFETY: `object` is ours and its table has the slot.
    let f = unsafe { *(*object).functions.add(offset / 8) };
    // SAFETY: the caller names the slot's signature.
    unsafe { std::mem::transmute_copy(&f) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_round_trip_through_text() {
        let g = guid(
            0x189819f1,
            0x1db6,
            0x4b57,
            [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7],
        );
        assert_eq!(g[..4], [0xf1, 0x19, 0x98, 0x18]);
        assert_eq!(guid_text(&g), "189819f1-1db6-4b57-be54-1821339b85f7");
    }
}
