//! Local diagnosis against Microsoft's own `xg_xs.dll` (the GDK's offline
//! PC build), which is never shipped: `VICTORY_XG_DLL=<path>` loads it.
//!
//! - `VICTORY_XG_MODE=compare` (default): the title keeps our layouts; every
//!   suggestion, layout, CPU copy and 64 KiB tiling is also asked of the real
//!   library and each difference is logged once per description.
//! - `VICTORY_XG_MODE=real`: the title is served by the real library.
#![allow(non_snake_case)]
// Title code and the real library pass pointers they trust.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use super::xg::{LayoutOut, MipOut, PlaneOut, ResourceDesc};
use super::xgameruntime::{HRESULT, S_OK};

type Suggest = unsafe extern "system" fn(*const ResourceDesc, u32, *mut u32) -> HRESULT;
type Create = unsafe extern "system" fn(*const ResourceDesc, *mut *mut c_void) -> HRESULT;
type Release = unsafe extern "system" fn(*mut c_void) -> u32;
type GetLayout = unsafe extern "system" fn(*mut c_void, *mut LayoutOut) -> HRESULT;
pub type CopyInto =
    unsafe extern "system" fn(*mut c_void, *mut u8, u32, u32, *const u8, u32, u32) -> HRESULT;
pub type Tiling = unsafe extern "system" fn(
    *mut c_void,
    *mut u32,
    *mut c_void,
    *mut [u32; 3],
    *mut u32,
    u32,
    *mut c_void,
) -> HRESULT;

const SLOT_RELEASE: usize = 1;
const SLOT_LAYOUT: usize = 2;
pub const SLOT_COPY_INTO: usize = 8;
pub const SLOT_TILING: usize = 10;
/// Copies compared at most, so a long session stays playable.
const COPY_CHECKS: u64 = 4096;
/// A summary every this many computers.
const SUMMARY_EVERY: u64 = 512;
/// Field names listed per differing layout.
const FIELDS_SHOWN: usize = 16;

pub struct Oracle {
    suggest: Suggest,
    create: Create,
    pub real: bool,
    seen: Mutex<HashSet<(u8, Key)>>,
    computers: AtomicU64,
    layouts_differ: AtomicU64,
    suggestions_differ: AtomicU64,
    copies: AtomicU64,
    copies_differ: AtomicU64,
}

type Key = [u64; 8];

fn key(d: &ResourceDesc) -> Key {
    [
        u64::from(d.dimension),
        d.width,
        u64::from(d.height),
        u64::from(d.depth_or_array) | u64::from(d.mips) << 16 | u64::from(d.quality) << 32,
        u64::from(d.format),
        u64::from(d.samples),
        u64::from(d.layout),
        u64::from(d.flags),
    ]
}

/// The loaded library, if `VICTORY_XG_DLL` names one that loads.
pub fn get() -> Option<&'static Oracle> {
    static ORACLE: OnceLock<Option<Oracle>> = OnceLock::new();
    ORACLE.get_or_init(load).as_ref()
}

#[cfg(windows)]
fn load() -> Option<Oracle> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        LoadLibraryExW,
    };
    let path = std::env::var_os("VICTORY_XG_DLL")?;
    let wide: Vec<u16> = path.encode_wide().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated path; the library stays loaded for the
    // process.
    let module = unsafe {
        LoadLibraryExW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        )
    };
    if module.is_null() {
        // SAFETY: no precondition.
        let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        tracing::error!(?path, error, "xg oracle: the library did not load");
        return None;
    }
    // SAFETY: exports of the loaded module.
    let (suggest, create) = unsafe {
        (
            GetProcAddress(module, c"XGSuggestSwizzleMode".as_ptr().cast()),
            GetProcAddress(module, c"XGCreateTextureComputer".as_ptr().cast()),
        )
    };
    let (Some(suggest), Some(create)) = (suggest, create) else {
        tracing::error!(?path, "xg oracle: exports missing");
        return None;
    };
    let real = std::env::var("VICTORY_XG_MODE").is_ok_and(|m| m == "real");
    tracing::info!(?path, real, "xg oracle loaded");
    type Proc = unsafe extern "system" fn() -> isize;
    Some(Oracle {
        // SAFETY: the exports have the signatures the title calls them with.
        suggest: unsafe { std::mem::transmute::<Proc, Suggest>(suggest) },
        // SAFETY: as above.
        create: unsafe { std::mem::transmute::<Proc, Create>(create) },
        real,
        seen: Mutex::new(HashSet::new()),
        computers: AtomicU64::new(0),
        layouts_differ: AtomicU64::new(0),
        suggestions_differ: AtomicU64::new(0),
        copies: AtomicU64::new(0),
        copies_differ: AtomicU64::new(0),
    })
}

#[cfg(not(windows))]
fn load() -> Option<Oracle> {
    None
}

/// Method `index` of a COM object.
///
/// # Safety
/// `this` is a live object whose slot `index` has type `T`.
pub unsafe fn slot<T: Copy>(this: *mut c_void, index: usize) -> T {
    // SAFETY: per the contract.
    unsafe {
        let vtable = *this.cast::<*const *const ()>();
        std::mem::transmute_copy(&*vtable.add(index))
    }
}

impl Oracle {
    /// True the first time `kind` is seen for this description.
    fn first(&self, kind: u8, d: &ResourceDesc) -> bool {
        self.seen.lock().is_ok_and(|mut s| s.insert((kind, key(d))))
    }

    pub fn suggest(&self, d: *const ResourceDesc, flags: u32, out: *mut u32) -> HRESULT {
        // SAFETY: the title's arguments, passed on unchanged.
        unsafe { (self.suggest)(d, flags, out) }
    }

    pub fn create(&self, d: *const ResourceDesc, out: *mut *mut c_void) -> HRESULT {
        // SAFETY: the title's arguments, passed on unchanged.
        unsafe { (self.create)(d, out) }
    }

    pub fn compare_suggest(&self, d: &ResourceDesc, ours: u32) {
        let mut real = 0u32;
        let hr = self.suggest(d, 0, &mut real);
        if (hr != S_OK || real != ours) && self.first(0, d) {
            self.suggestions_differ.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                hr = format_args!("{:#x}", hr as u32),
                ours = format_args!("{ours:#x}"),
                real = format_args!("{real:#x}"),
                ?d,
                "xg oracle: suggested mode differs"
            );
        }
    }

    /// A real computer for `d`, or null when the library refuses it.
    pub fn computer(&self, d: &ResourceDesc) -> *mut c_void {
        let mut c = std::ptr::null_mut();
        if self.create(d, &mut c) != S_OK {
            return std::ptr::null_mut();
        }
        c
    }

    pub fn release(&self, c: *mut c_void) {
        if !c.is_null() {
            // SAFETY: a live real computer.
            unsafe { slot::<Release>(c, SLOT_RELEASE)(c) };
        }
    }

    /// Our layout of a description against the real one; `ours` is None
    /// when we refused it.
    pub fn compare_layout(&self, d: &ResourceDesc, ours: Option<&LayoutOut>, real: *mut c_void) {
        let n = self.computers.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(SUMMARY_EVERY) {
            self.summary();
        }
        let theirs = if real.is_null() {
            None
        } else {
            // SAFETY: plain data, filled by the call.
            let mut l: Box<LayoutOut> = Box::new(unsafe { std::mem::zeroed() });
            // SAFETY: a live real computer and room for its layout.
            let hr = unsafe { slot::<GetLayout>(real, SLOT_LAYOUT)(real, &mut *l) };
            (hr == S_OK).then_some(l)
        };
        let diffs = match (ours, theirs) {
            (Some(a), Some(b)) => layout_diffs(a, &b),
            (None, None) => return,
            (Some(_), None) => vec!["real library refuses it".to_owned()],
            (None, Some(b)) => vec![format!(
                "we refuse it, real size {} mips {}",
                b.size, b.mips
            )],
        };
        if !diffs.is_empty() && self.first(1, d) {
            self.layouts_differ.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(?d, diffs = ?diffs, "xg oracle: layout differs");
        }
    }

    /// Whether this copy should be checked (first of its kind, under the cap).
    pub fn check_copy(&self, d: &ResourceDesc, subresource: u32) -> bool {
        let mut k = *d;
        k.quality = subresource;
        if self.copies.load(Ordering::Relaxed) >= COPY_CHECKS || !self.first(2, &k) {
            return false;
        }
        self.copies.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub fn report_copy(&self, d: &ResourceDesc, subresource: u32, ours: &[u8], theirs: &[u8]) {
        let differ = ours.iter().zip(theirs).filter(|(a, b)| a != b).count();
        if differ == 0 {
            return;
        }
        self.copies_differ.fetch_add(1, Ordering::Relaxed);
        let first = ours.iter().zip(theirs).position(|(a, b)| a != b);
        tracing::warn!(
            ?d,
            subresource,
            differ,
            of = ours.len(),
            first = ?first,
            "xg oracle: tiled bytes differ"
        );
    }

    pub fn report_tiling(&self, d: &ResourceDesc, what: &str) {
        if self.first(3, d) {
            tracing::warn!(?d, what, "xg oracle: 64 KiB tiling differs");
        }
    }

    pub fn summary(&self) {
        tracing::info!(
            computers = self.computers.load(Ordering::Relaxed),
            layouts_differ = self.layouts_differ.load(Ordering::Relaxed),
            suggestions_differ = self.suggestions_differ.load(Ordering::Relaxed),
            copies = self.copies.load(Ordering::Relaxed),
            copies_differ = self.copies_differ.load(Ordering::Relaxed),
            "xg oracle summary"
        );
    }
}

fn push<T: PartialEq + std::fmt::Debug>(out: &mut Vec<String>, name: &str, a: T, b: T) {
    if a != b && out.len() < FIELDS_SHOWN {
        out.push(format!("{name}: ours {a:?} real {b:?}"));
    }
}

fn mip_diffs(out: &mut Vec<String>, at: &str, a: &MipOut, b: &MipOut) {
    push(out, &format!("{at}.size"), a.size, b.size);
    push(out, &format!("{at}.offset"), a.offset, b.offset);
    push(out, &format!("{at}.slice_size"), a.slice_size, b.slice_size);
    push(
        out,
        &format!("{at}.pitch_texels"),
        a.pitch_texels,
        b.pitch_texels,
    );
    push(
        out,
        &format!("{at}.pitch_bytes"),
        a.pitch_bytes,
        b.pitch_bytes,
    );
    push(out, &format!("{at}.alignment"), a.alignment, b.alignment);
    push(out, &format!("{at}.padded"), a.padded, b.padded);
    push(out, &format!("{at}.elements"), a.elements, b.elements);
    push(out, &format!("{at}.texels"), a.texels, b.texels);
    push(
        out,
        &format!("{at}.depth_or_array"),
        a.depth_or_array,
        b.depth_or_array,
    );
    push(out, &format!("{at}.samples"), a.samples, b.samples);
    push(out, &format!("{at}.swizzle"), a.swizzle, b.swizzle);
    push(
        out,
        &format!("{at}.bank_rotation"),
        a.bank_rotation,
        b.bank_rotation,
    );
    push(
        out,
        &format!("{at}.slice_depth"),
        a.slice_depth,
        b.slice_depth,
    );
    push(out, &format!("{at}.in_tail"), a.in_tail, b.in_tail);
    push(
        out,
        &format!("{at}.tail_position"),
        a.tail_position,
        b.tail_position,
    );
}

fn plane_diffs(out: &mut Vec<String>, i: usize, mips: usize, a: &PlaneOut, b: &PlaneOut) {
    let at = format!("plane{i}");
    push(out, &format!("{at}.usage"), a.usage, b.usage);
    push(out, &format!("{at}.size"), a.size, b.size);
    push(out, &format!("{at}.slice_size"), a.slice_size, b.slice_size);
    push(out, &format!("{at}.offset"), a.offset, b.offset);
    push(out, &format!("{at}.alignment"), a.alignment, b.alignment);
    push(
        out,
        &format!("{at}.element_bytes"),
        a.element_bytes,
        b.element_bytes,
    );
    push(
        out,
        &format!("{at}.standard_mips"),
        a.standard_mips,
        b.standard_mips,
    );
    push(out, &format!("{at}.tail_mips"), a.tail_mips, b.tail_mips);
    for m in 0..mips {
        mip_diffs(out, &format!("{at}.mip{m}"), &a.mips[m], &b.mips[m]);
    }
}

/// Field-by-field differences, the first few named.
pub fn layout_diffs(a: &LayoutOut, b: &LayoutOut) -> Vec<String> {
    let mut out = Vec::new();
    push(&mut out, "size", a.size, b.size);
    push(&mut out, "alignment", a.alignment, b.alignment);
    push(&mut out, "mips", a.mips, b.mips);
    push(&mut out, "planes", a.planes, b.planes);
    push(&mut out, "dimension", a.dimension, b.dimension);
    let planes = a.planes.max(b.planes).min(4) as usize;
    let mips = (a.mips.max(b.mips) as usize).min(a.plane[0].mips.len());
    for i in 0..planes {
        plane_diffs(&mut out, i, mips, &a.plane[i], &b.plane[i]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Box<LayoutOut> {
        // SAFETY: plain data; zeroes are a valid layout.
        Box::new(unsafe { std::mem::zeroed() })
    }

    #[test]
    fn identical_layouts_have_no_differences() {
        let a = empty();
        assert!(layout_diffs(&a, &a.clone()).is_empty());
    }

    /// The difference measured in Grand Theft Auto VI: tail mips of 64 KiB
    /// textures, reported in their own mode with a position by the library.
    #[test]
    fn tail_mip_differences_are_named() {
        let mut a = empty();
        a.mips = 2;
        a.planes = 1;
        a.plane[0].mips[1].swizzle = 2;
        let mut b = a.clone();
        b.plane[0].mips[1].swizzle = 10;
        b.plane[0].mips[1].tail_position = 32;
        assert_eq!(
            layout_diffs(&a, &b),
            [
                "plane0.mip1.swizzle: ours 2 real 10",
                "plane0.mip1.tail_position: ours 0 real 32"
            ]
        );
    }
}
