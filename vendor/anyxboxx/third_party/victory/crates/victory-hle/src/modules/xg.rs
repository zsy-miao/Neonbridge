//! `xg_xs.dll`: the console's texture layout library. Titles ask it which
//! swizzle mode a texture should use, how large it is, where each mip and
//! slice lies, and to copy linear data into tiled memory and back. Served
//! from `victory_formats::texture`.
//!
//! The title imports `XGCreateTextureComputer` and
//! `XGSuggestSwizzleMode` and calls these computer methods: Release (0x08),
//! the layout (0x10), copies into (0x40) and out of (0x48) a subresource,
//! and the 64 KiB tiling (0x50). The other slots log and fail.
#![allow(non_snake_case)]
// Title code passes pointers the real library would trust.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use victory_formats::texture::{Dimension, Error, Layout, Swizzle, Texture, TextureDesc};

use super::xasync::{E_INVALIDARG, E_NOTIMPL};
use super::xg_oracle;
use super::xgameruntime::{HRESULT, S_OK};
use crate::{HleModule, hle_export};

const E_FAIL: HRESULT = 0x8000_4005_u32 as i32;

/// `D3D12_RESOURCE_DESC`, whose layout field carries `0x100 | swizzle
/// mode` on the console (1 row major, 0 "pick one").
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ResourceDesc {
    pub dimension: u32,
    pub alignment: u64,
    pub width: u64,
    pub height: u32,
    pub depth_or_array: u16,
    pub mips: u16,
    pub format: u32,
    pub samples: u32,
    pub quality: u32,
    pub layout: u32,
    pub flags: u32,
}
const _: () = assert!(size_of::<ResourceDesc>() == 0x38);

const SWIZZLED: u32 = 0x100;
const ROW_MAJOR: u32 = 1;
const ALLOW_RENDER_TARGET: u32 = 1;
const ALLOW_DEPTH_STENCIL: u32 = 2;
const TEXTURE_1D: u32 = 2;
const TEXTURE_3D: u32 = 4;

/// The mode the console's library picks for a description: linear for 1D,
/// the XOR render and depth modes for targets, else 4 KiB standard
/// (`.ttpf` textures without a mode of their own use it).
///
/// shortcut: the rule of the library's older optimal-mode function (any
/// size, 2D, 3D and cubes alike); `XGSuggestSwizzleMode` itself is newer
/// than every copy we could check. Revisit if streamed textures come out
/// scrambled.
fn suggested(desc: &ResourceDesc) -> Swizzle {
    if desc.dimension == TEXTURE_1D {
        Swizzle::LINEAR
    } else if desc.flags & ALLOW_DEPTH_STENCIL != 0 {
        Swizzle::SW_64KB_Z_X
    } else if desc.flags & ALLOW_RENDER_TARGET != 0 {
        Swizzle::SW_64KB_R_X
    } else {
        Swizzle::SW_4KB_S
    }
}

/// `(desc, flags, out layout)`; the title passes flags 0 and puts the
/// result in the description's layout field.
pub extern "system" fn XGSuggestSwizzleMode(
    desc: *const ResourceDesc,
    flags: u32,
    out: *mut u32,
) -> HRESULT {
    let oracle = xg_oracle::get();
    if let Some(o) = oracle
        && o.real
    {
        return o.suggest(desc, flags, out);
    }
    // SAFETY: the title passes a description and room for the layout
    // (either may be null).
    match unsafe { (desc.as_ref(), out.as_mut()) } {
        (Some(desc), Some(out)) => {
            *out = SWIZZLED | suggested(desc).0;
            if let Some(o) = oracle {
                o.compare_suggest(desc, *out);
            }
            S_OK
        }
        _ => E_INVALIDARG,
    }
}

#[repr(C)]
pub struct Computer {
    vtable: &'static Vtable,
    refs: AtomicU32,
    desc: ResourceDesc,
    mips: u32,
    texture: Texture,
    /// The real library's computer for the same description when
    /// `VICTORY_XG_DLL` asks for a comparison (else null), and its size.
    real: *mut c_void,
    real_size: u64,
}

type This = *mut Computer;

fn computer<'a>(this: This) -> &'a Computer {
    // SAFETY: called from computer methods only.
    unsafe { &*this }
}

pub extern "system" fn XGCreateTextureComputer(
    desc: *const ResourceDesc,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description and room for a pointer.
    let oracle = xg_oracle::get();
    if let Some(o) = oracle
        && o.real
    {
        return o.create(desc, out.cast());
    }
    let (Some(&desc), false) = (unsafe { desc.as_ref() }, out.is_null()) else {
        return E_INVALIDARG;
    };
    let real = oracle.map_or(std::ptr::null_mut(), |o| o.computer(&desc));
    let Some(texture) = texture(&desc) else {
        tracing::warn!(?desc, "no texture layout");
        if let Some(o) = oracle {
            o.compare_layout(&desc, None, real);
            o.release(real);
        }
        return E_INVALIDARG;
    };
    tracing::debug!(
        width = desc.width,
        height = desc.height,
        format = desc.format,
        layout = format_args!("{:#x}", desc.layout),
        size = texture.size,
        "texture computer"
    );
    let mips = texture.planes[0].layout.mips.len() as u32;
    let computer = Box::new(Computer {
        vtable: &VTABLE,
        refs: AtomicU32::new(1),
        desc,
        mips,
        texture,
        real,
        real_size: 0,
    });
    let this = Box::into_raw(computer);
    if let Some(o) = oracle {
        // SAFETY: plain data, filled by the call.
        let mut ours: Box<LayoutOut> = Box::new(unsafe { std::mem::zeroed() });
        layout(this, &mut *ours);
        o.compare_layout(&desc, Some(&ours), real);
        if !real.is_null() {
            // SAFETY: plain data, filled by the call; a live real computer.
            let mut theirs: Box<LayoutOut> = Box::new(unsafe { std::mem::zeroed() });
            let get: extern "system" fn(*mut c_void, *mut LayoutOut) -> HRESULT =
                unsafe { xg_oracle::slot(real, 2) };
            get(real, &mut *theirs);
            // SAFETY: just made, not yet shared.
            unsafe { (*this).real_size = theirs.size };
        }
    }
    // SAFETY: checked non-null.
    unsafe { out.write(this) };
    S_OK
}

fn texture(desc: &ResourceDesc) -> Option<Texture> {
    let dimension = match desc.dimension {
        TEXTURE_1D => Dimension::Tex1d,
        3 => Dimension::Tex2d,
        TEXTURE_3D => Dimension::Tex3d,
        _ => return None,
    };
    let swizzle = match desc.layout {
        0 => suggested(desc),
        ROW_MAJOR => Swizzle::LINEAR,
        l if l & !0xff == SWIZZLED => Swizzle(l & 0xff),
        _ => return None,
    };
    let texture = Texture::new(&TextureDesc {
        dimension,
        width: u32::try_from(desc.width).ok()?,
        height: desc.height,
        depth_or_array: u32::from(desc.depth_or_array),
        mips: u32::from(desc.mips),
        format: desc.format,
        samples: desc.samples.max(1),
        swizzle,
    })
    .ok()?;
    // The layout struct has room for 15 mips.
    (texture.planes[0].layout.mips.len() <= MIPS).then_some(texture)
}

const MIPS: usize = 15;

/// One mip in the layout the title reads (offsets 0x00, 0x08, 0x10 and
/// 0x1c are read by the title; the rest follow the library's
/// older public shape).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct MipOut {
    pub size: u64,
    pub offset: u64,
    pub slice_size: u64,
    pub pitch_texels: u32,
    pub pitch_bytes: u32,
    pub alignment: u32,
    /// Width, height in elements, depth or array size.
    pub padded: [u32; 3],
    pub elements: [u32; 2],
    pub texels: [u32; 2],
    pub depth_or_array: u32,
    pub samples: u32,
    pub swizzle: u32,
    pub bank_rotation: [u64; 2],
    pub slice_depth: u32,
    pub in_tail: u32,
    /// Where a tail mip starts in elements: x bits 0-10, y 11-21.
    pub tail_position: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PlaneOut {
    /// 1 data, 7 depth, 8 stencil (metadata planes are not made).
    pub usage: u32,
    pub size: u64,
    pub slice_size: u64,
    pub offset: u64,
    pub alignment: u64,
    pub element_bytes: u32,
    pub standard_mips: u32,
    pub tail_mips: u32,
    pub mips: [MipOut; MIPS],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LayoutOut {
    pub size: u64,
    pub alignment: u64,
    pub mips: u32,
    pub planes: u32,
    pub plane: [PlaneOut; 4],
    pub dimension: u32,
}
const _: () = assert!(size_of::<MipOut>() == 0x70);
const _: () = assert!(size_of::<PlaneOut>() == 0x6c8);
const _: () = assert!(size_of::<LayoutOut>() == 0x1b40);

fn mip_out(desc: &ResourceDesc, l: &Layout, i: usize) -> MipOut {
    let m = &l.mips[i];
    let bpe = u64::from(l.desc.element_bytes);
    let block = l.desc.block;
    let three_d = desc.dimension == TEXTURE_3D;
    let array = u32::from(desc.depth_or_array).max(1);
    let mut out = MipOut {
        pitch_texels: m.pitch * block,
        pitch_bytes: m.pitch * l.desc.element_bytes,
        padded: [m.pitch, m.rows, if three_d { m.depth_pitch } else { array }],
        elements: [m.width, m.height],
        texels: [((desc.width >> i) as u32).max(1), (desc.height >> i).max(1)],
        depth_or_array: if three_d { m.depth } else { array },
        samples: l.desc.samples,
        slice_depth: l.block[2],
        ..MipOut::default()
    };
    let slice = u64::from(m.pitch) * u64::from(m.rows) * bpe;
    let depth = u64::from(if three_d { m.depth_pitch } else { 1 });
    match m.tail {
        None => {
            out.offset = m.offset;
            out.slice_size = slice * u64::from(l.desc.samples);
            out.size = out.slice_size * depth;
            out.alignment = l.alignment as u32;
            out.swizzle = l.desc.swizzle.0;
        }
        Some(t) => {
            // As Microsoft's xg_xs.dll reports a tail mip (measured against
            // its offline PC build, 10.0.26100.6247): in the texture's own
            // mode, at its element position in the tail block.
            out.in_tail = 1;
            out.alignment = 256;
            out.swizzle = l.desc.swizzle.0;
            out.tail_position = t.x | t.y << 11;
            if l.thick {
                // A 3D tail mip is placed by position, not by offset.
                out.slice_size = l.alignment / u64::from(l.block[2]);
            } else {
                out.offset = u64::from(t.offset);
                out.slice_size = slice;
            }
            out.size = out.slice_size * depth;
        }
    }
    out
}

extern "system" fn add_ref(this: This) -> u32 {
    computer(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn release(this: This) -> u32 {
    let left = computer(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        // SAFETY: made by `Box::into_raw` in `XGCreateTextureComputer`; the
        // last reference is gone.
        let c = unsafe { Box::from_raw(this) };
        if let Some(o) = xg_oracle::get() {
            o.release(c.real);
        }
    }
    left
}

extern "system" fn layout(this: This, out: *mut LayoutOut) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let c = computer(this);
    // SAFETY: the title passes room for the layout; every field is plain
    // data, so zeroes are a valid start.
    let out = unsafe {
        out.write_bytes(0, 1);
        &mut *out
    };
    out.size = c.texture.size;
    out.alignment = c.texture.alignment;
    out.mips = c.mips;
    out.planes = c.texture.planes.len() as u32;
    out.dimension = c.desc.dimension;
    // The library calls a depth plane "depth" only beside stencil or
    // without mips.
    let depth =
        c.desc.flags & ALLOW_DEPTH_STENCIL != 0 && (c.texture.planes.len() > 1 || c.mips == 1);
    for (plane, p) in out.plane.iter_mut().zip(&c.texture.planes) {
        let l = &p.layout;
        plane.usage = match (p.stencil, depth) {
            (true, _) => 8,
            (false, true) => 7,
            (false, false) => 1,
        };
        plane.size = l.size;
        plane.slice_size = l.slice_size;
        plane.offset = p.offset;
        plane.alignment = l.alignment;
        plane.element_bytes = l.desc.element_bytes;
        plane.standard_mips = l.first_tail_mip;
        plane.tail_mips = c.mips - l.first_tail_mip;
        for (i, mip) in plane.mips.iter_mut().take(l.mips.len()).enumerate() {
            *mip = mip_out(&c.desc, l, i);
        }
    }
    S_OK
}

/// The plane, its mip and array slice for a subresource index, and the
/// tiled bytes and the linear bytes the copy touches.
struct CopyArgs<'a> {
    layout: &'a Layout,
    mip: u32,
    slice: u32,
    tiled: usize,
    linear: usize,
}

fn copy_args(
    c: &Computer,
    plane: u32,
    subresource: u32,
    row: u32,
    depth: u32,
) -> Option<CopyArgs<'_>> {
    let p = c.texture.planes.get(plane as usize)?;
    let l = &p.layout;
    let (mip, slice) = (subresource % c.mips, subresource / c.mips);
    let m = l.mips.get(mip as usize)?;
    let bytes = m.width as usize * l.desc.element_bytes as usize;
    let depths = l.subresource_depth(mip) as usize;
    let linear = (depths - 1) * depth as usize + (m.height as usize - 1) * row as usize + bytes;
    Some(CopyArgs {
        layout: l,
        mip,
        slice,
        tiled: (p.offset + l.size) as usize,
        linear,
    })
}

fn result(r: Result<(), Error>) -> HRESULT {
    match r {
        Ok(()) => S_OK,
        Err(Error::Unsupported) => E_NOTIMPL,
        Err(_) => E_INVALIDARG,
    }
}

/// `(tiled base, plane, subresource, linear, row pitch, depth pitch)`.
extern "system" fn copy_into(
    this: This,
    tiled: *mut u8,
    plane: u32,
    subresource: u32,
    linear: *const u8,
    row: u32,
    depth: u32,
) -> HRESULT {
    let c = computer(this);
    let Some(a) = copy_args(c, plane, subresource, row, depth) else {
        return E_INVALIDARG;
    };
    if tiled.is_null() || linear.is_null() {
        return E_INVALIDARG;
    }
    let offset = c.texture.planes[plane as usize].offset as usize;
    // SAFETY: the title's memory holds the whole texture and its rows.
    let (tiled, linear) = unsafe {
        (
            std::slice::from_raw_parts_mut(tiled.add(offset), a.tiled - offset),
            std::slice::from_raw_parts(linear, a.linear),
        )
    };
    let r = a
        .layout
        .tile(tiled, a.mip, a.slice, linear, row as usize, depth as usize);
    if r.is_err() {
        tracing::warn!(?r, desc = ?c.desc, subresource, "copy into a texture failed");
    } else if !c.real.is_null()
        && let Some(o) = xg_oracle::get()
        && o.check_copy(&c.desc, subresource)
    {
        compare_copy(o, c, &a, plane, subresource, linear, row, depth);
    }
    result(r)
}

/// The same copy through our layout and the real library, into scratch
/// memory, compared byte for byte.
#[allow(clippy::too_many_arguments)]
fn compare_copy(
    o: &xg_oracle::Oracle,
    c: &Computer,
    a: &CopyArgs<'_>,
    plane: u32,
    subresource: u32,
    linear: &[u8],
    row: u32,
    depth: u32,
) {
    let offset = c.texture.planes[plane as usize].offset as usize;
    let mut ours = vec![0u8; a.tiled];
    let mut theirs = vec![0u8; a.tiled.max(c.real_size as usize) + 4096];
    let r = a.layout.tile(
        &mut ours[offset..],
        a.mip,
        a.slice,
        linear,
        row as usize,
        depth as usize,
    );
    // SAFETY: a live real computer; scratch memory holds the whole texture
    // as the real library sizes it.
    let hr = unsafe {
        xg_oracle::slot::<xg_oracle::CopyInto>(c.real, xg_oracle::SLOT_COPY_INTO)(
            c.real,
            theirs.as_mut_ptr(),
            plane,
            subresource,
            linear.as_ptr(),
            row,
            depth,
        )
    };
    if r.is_ok() && hr == S_OK {
        o.report_copy(&c.desc, subresource, &ours, &theirs[..a.tiled]);
    } else {
        tracing::warn!(?r, hr = format_args!("{:#x}", hr as u32), desc = ?c.desc, subresource, "xg oracle: copy refused");
    }
}

/// `(linear, plane, subresource, sample, tiled base, row pitch, depth
/// pitch)`.
#[allow(clippy::too_many_arguments)]
extern "system" fn copy_from(
    this: This,
    linear: *mut u8,
    plane: u32,
    subresource: u32,
    _sample: u32,
    tiled: *const u8,
    row: u32,
    depth: u32,
) -> HRESULT {
    let c = computer(this);
    let Some(a) = copy_args(c, plane, subresource, row, depth) else {
        return E_INVALIDARG;
    };
    if tiled.is_null() || linear.is_null() {
        return E_INVALIDARG;
    }
    let offset = c.texture.planes[plane as usize].offset as usize;
    // SAFETY: the title's memory holds the whole texture and its rows.
    let (tiled, linear) = unsafe {
        (
            std::slice::from_raw_parts(tiled.add(offset), a.tiled - offset),
            std::slice::from_raw_parts_mut(linear, a.linear),
        )
    };
    let r = a
        .layout
        .detile(tiled, a.mip, a.slice, linear, row as usize, depth as usize);
    if r.is_err() {
        tracing::warn!(?r, desc = ?c.desc, subresource, "copy out of a texture failed");
    }
    result(r)
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackedMips {
    pub standard: u8,
    pub packed: u8,
    pub tiles: u32,
    pub start: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubresourceTiling {
    pub width: u32,
    pub height: u16,
    pub depth: u16,
    pub start: u32,
}

const TILE: u64 = 64 << 10;

/// `(total tiles, packed mips, tile shape in texels, in: room / out:
/// count, first, subresource tilings)`, in 64 KiB tiles of plane 0, for 2D
/// 64 KiB modes only (others fail, as on the console). The mip tail is the
/// packed part, tile 0 of each slice; tilings list the standard mips of
/// subresource (`mip + slice * mips`, as D3D12 counts them), zeroed for the
/// packed mips. Measured against Microsoft's xg_xs.dll (offline PC build
/// 10.0.26100.6247): it fills the room from `first` on and counts the
/// standard mips whose subresource falls in it (with 16 entries of room, as
/// the title passes, two slices of an array with a tail).
extern "system" fn tiling_ours(
    this: This,
    tiles: *mut u32,
    packed: *mut PackedMips,
    shape: *mut [u32; 3],
    count: *mut u32,
    first: u32,
    out: *mut SubresourceTiling,
) -> HRESULT {
    let c = computer(this);
    let l = &c.texture.planes[0].layout;
    if l.alignment != TILE {
        return E_FAIL;
    }
    if l.thick {
        return E_NOTIMPL;
    }
    let standard = l.first_tail_mip;
    let tail_tiles = match standard {
        0 => l.slice_size / TILE,
        s if s < c.mips => l.mips[s as usize - 1].offset / TILE,
        _ => 0,
    };
    let block = l.desc.block;
    // SAFETY: each output is null or room for its value; `out` has room
    // for `*count` entries.
    unsafe {
        if let Some(t) = tiles.as_mut() {
            *t = (l.size / TILE) as u32;
        }
        if let Some(p) = packed.as_mut() {
            *p = PackedMips {
                standard: standard as u8,
                packed: (c.mips - standard) as u8,
                tiles: tail_tiles as u32,
                start: 0,
            };
        }
        if let Some(s) = shape.as_mut() {
            *s = [l.block[0] * block, l.block[1] * block, l.block[2]];
        }
        if let Some(n) = count.as_mut() {
            let room = (c.mips * l.slices).saturating_sub(first).min(*n);
            let mut standard_in_room = 0;
            for i in 0..room {
                let (slice, mip) = ((first + i) / c.mips, (first + i) % c.mips);
                let entry = if mip < standard && slice < l.slices {
                    standard_in_room += 1;
                    let m = &l.mips[mip as usize];
                    SubresourceTiling {
                        width: m.pitch / l.block[0],
                        height: (m.rows / l.block[1]) as u16,
                        depth: 1,
                        start: ((u64::from(slice) * l.slice_size + m.offset) / TILE) as u32,
                    }
                } else {
                    SubresourceTiling {
                        width: 0,
                        height: 0,
                        depth: 0,
                        start: 0,
                    }
                };
                out.add(i as usize).write(entry);
            }
            *n = standard_in_room;
        }
    }
    S_OK
}

/// Slot 0x50 as the title calls it: ours, then the real library's answer
/// to the same question compared when a comparison is on.
extern "system" fn tiling(
    this: This,
    tiles: *mut u32,
    packed: *mut PackedMips,
    shape: *mut [u32; 3],
    count: *mut u32,
    first: u32,
    out: *mut SubresourceTiling,
) -> HRESULT {
    // SAFETY: null or room for the count.
    let room = unsafe { count.as_ref().copied() }.unwrap_or(0);
    let hr = tiling_ours(this, tiles, packed, shape, count, first, out);
    let c = computer(this);
    if let Some(o) = xg_oracle::get()
        && !c.real.is_null()
    {
        compare_tiling(o, c, hr, room, first, out);
    }
    hr
}

fn compare_tiling(
    o: &xg_oracle::Oracle,
    c: &Computer,
    hr: HRESULT,
    room: u32,
    first: u32,
    ours_out: *const SubresourceTiling,
) {
    let zero = SubresourceTiling {
        width: 0,
        height: 0,
        depth: 0,
        start: 0,
    };
    let pz = PackedMips {
        standard: 0,
        packed: 0,
        tiles: 0,
        start: 0,
    };
    let (mut a_tiles, mut a_packed, mut a_shape, mut a_n) = (0u32, pz, [0u32; 3], room);
    let (mut b_tiles, mut b_packed, mut b_shape, mut b_n) = (0u32, pz, [0u32; 3], room);
    let mut a_out = vec![zero; room as usize];
    let mut b_out = vec![zero; room as usize];
    let this = std::ptr::from_ref(c).cast_mut();
    let a_hr = tiling_ours(
        this,
        &mut a_tiles,
        &mut a_packed,
        &mut a_shape,
        &mut a_n,
        first,
        a_out.as_mut_ptr(),
    );
    // SAFETY: a live real computer and room for every output.
    let b_hr = unsafe {
        xg_oracle::slot::<xg_oracle::Tiling>(c.real, xg_oracle::SLOT_TILING)(
            c.real,
            &mut b_tiles,
            std::ptr::from_mut(&mut b_packed).cast(),
            &mut b_shape,
            &mut b_n,
            first,
            b_out.as_mut_ptr().cast(),
        )
    };
    let _ = (hr, ours_out);
    let what = if a_hr != b_hr {
        format!("hr ours {:#x} real {:#x}", a_hr as u32, b_hr as u32)
    } else if a_hr != S_OK {
        return;
    } else if (a_tiles, a_packed, a_shape, a_n) != (b_tiles, b_packed, b_shape, b_n) {
        format!(
            "ours {a_tiles} {a_packed:?} {a_shape:?} n={a_n} real {b_tiles} {b_packed:?} {b_shape:?} n={b_n}"
        )
    } else if a_out[..a_n as usize] != b_out[..b_n as usize] {
        let i = a_out
            .iter()
            .zip(&b_out)
            .position(|(x, y)| x != y)
            .unwrap_or(0);
        format!("entry {i}: ours {:?} real {:?}", a_out[i], b_out[i])
    } else {
        return;
    };
    o.report_tiling(&c.desc, &what);
}

/// Slot 0x30 `(plane, mip, x, y, z or slice, sample, bit position)`: an
/// element's byte offset from the texture's start. The title reads
/// texels of a 128x4 `4KB_S` texture on the CPU through it (title+0x5151c90);
/// an error returned as an offset made it read wild memory. 0 if outside.
extern "system" fn element_offset(
    this: This,
    plane: u32,
    mip: u32,
    x: u64,
    y: u32,
    z: u32,
    _sample: u32,
    bit: *mut u32,
) -> u64 {
    if !bit.is_null() {
        // SAFETY: the caller passes room for the bit position or null.
        unsafe { bit.write(0) };
    }
    let Some(p) = computer(this).texture.planes.get(plane as usize) else {
        return 0;
    };
    let x = u32::try_from(x).unwrap_or(u32::MAX);
    match p.layout.offset(mip, x, y, z) {
        Ok(at) => p.offset + at,
        Err(e) => {
            tracing::warn!(?e, mip, x, y, z, "texel outside its texture");
            0
        }
    }
}

/// Slots the title does not call: size, alignment, offsets, coordinates,
/// view descriptor, tiled-resource query.
extern "system" fn unused(this: This) -> HRESULT {
    tracing::warn!(desc = ?computer(this).desc, "unimplemented texture computer method");
    E_NOTIMPL
}

#[repr(C)]
struct Vtable([*const (); 13]);
// SAFETY: function addresses only.
unsafe impl Sync for Vtable {}

static VTABLE: Vtable = Vtable([
    add_ref as *const (),        // 0x00
    release as *const (),        // 0x08
    layout as *const (),         // 0x10
    unused as *const (),         // 0x18 size
    unused as *const (),         // 0x20 alignment
    unused as *const (),         // 0x28 mip offset
    element_offset as *const (), // 0x30
    unused as *const (),         // 0x38 element coordinate
    copy_into as *const (),      // 0x40
    copy_from as *const (),      // 0x48
    tiling as *const (),         // 0x50
    unused as *const (),         // 0x58 view descriptor
    unused as *const (),         // 0x60 tiled resource?
]);

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "xg",
        dlls: &["xg_xs.dll"],
        exports: vec![
            hle_export!(XGCreateTextureComputer),
            hle_export!(XGSuggestSwizzleMode),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(width: u64, height: u32, mips: u16, format: u32, layout: u32) -> ResourceDesc {
        ResourceDesc {
            dimension: 3,
            alignment: 0,
            width,
            height,
            depth_or_array: 1,
            mips,
            format,
            samples: 1,
            quality: 0,
            layout,
            flags: 0,
        }
    }

    /// Method `index` of a computer, as the title calls it.
    fn slot<T: Copy>(c: This, index: usize) -> T {
        // SAFETY: the computer is live; T is the slot's function type.
        unsafe { std::mem::transmute_copy(&(*c).vtable.0[index]) }
    }

    fn create(d: &ResourceDesc) -> This {
        let mut c = std::ptr::null_mut();
        assert_eq!(XGCreateTextureComputer(d, &mut c), S_OK);
        c
    }

    /// Element offsets: linear rows of 1 KiB (128 elements), 0 outside.
    #[test]
    fn element_offsets_follow_the_layout() {
        type Offset = extern "system" fn(This, u32, u32, u64, u32, u32, u32, *mut u32) -> u64;
        let c = create(&desc(128, 4, 1, 11, 0x100));
        let offset: Offset = slot(c, 6);
        let mut bit = 7;
        assert_eq!(offset(c, 0, 0, 3, 1, 0, 0, &mut bit), (128 + 3) * 8);
        assert_eq!(bit, 0);
        assert_eq!(offset(c, 0, 0, 128, 0, 0, 0, std::ptr::null_mut()), 0);
        slot::<extern "system" fn(This) -> u32>(c, 1)(c);
    }

    #[test]
    fn suggestions_follow_the_use() {
        let mut out = 0;
        let mut d = desc(256, 256, 1, 98, 0);
        assert_eq!(XGSuggestSwizzleMode(&d, 0, &mut out), S_OK);
        assert_eq!(out, 0x105);
        d.flags = ALLOW_RENDER_TARGET;
        XGSuggestSwizzleMode(&d, 0, &mut out);
        assert_eq!(out, 0x11b);
        d.flags = ALLOW_DEPTH_STENCIL;
        XGSuggestSwizzleMode(&d, 0, &mut out);
        assert_eq!(out, 0x118);
    }

    /// The title's calls: layout, then a copy in and out of each mip.
    #[test]
    fn layout_and_copies_through_the_object() {
        // 256x256 BC7, full chain, 64 KiB display mode.
        let c = create(&desc(256, 256, 0, 98, 0x10a));
        // SAFETY: plain data, filled by the call.
        let mut l: Box<LayoutOut> = Box::new(unsafe { std::mem::zeroed() });
        let get: extern "system" fn(This, *mut LayoutOut) -> HRESULT = slot(c, 2);
        assert_eq!(get(c, &mut *l), S_OK);
        assert_eq!(
            (l.size, l.alignment, l.mips, l.planes),
            (131072, 65536, 9, 1)
        );
        let p = &l.plane[0];
        assert_eq!((p.standard_mips, p.tail_mips, p.element_bytes), (1, 8, 16));
        assert_eq!((p.mips[0].offset, p.mips[0].pitch_bytes), (65536, 1024));
        assert_eq!((p.mips[1].offset, p.mips[1].in_tail), (32768, 1));
        // A tail mip in the texture's own mode, at x 32 of the tail block.
        assert_eq!((p.mips[1].swizzle, p.mips[1].tail_position), (10, 32));
        assert_eq!((p.mips[2].swizzle, p.mips[2].tail_position), (10, 32 << 11));

        type Into = extern "system" fn(This, *mut u8, u32, u32, *const u8, u32, u32) -> HRESULT;
        type From =
            extern "system" fn(This, *mut u8, u32, u32, u32, *const u8, u32, u32) -> HRESULT;
        let (into, from): (Into, From) = (slot(c, 8), slot(c, 9));
        let mut tiled = vec![0u8; l.size as usize];
        for mip in 0..9u32 {
            let m = &p.mips[mip as usize];
            let row = m.elements[0] * 16;
            let rows: Vec<u8> = (0..row * m.elements[1]).map(|i| (i ^ mip) as u8).collect();
            assert_eq!(
                into(c, tiled.as_mut_ptr(), 0, mip, rows.as_ptr(), row, 0),
                S_OK
            );
            let mut back = vec![0u8; rows.len()];
            assert_eq!(
                from(c, back.as_mut_ptr(), 0, mip, 0, tiled.as_ptr(), row, 0),
                S_OK
            );
            assert_eq!(back, rows, "mip {mip}");
        }
        // 0x50: one tile for the tail, one for mip 0.
        type Tiling = extern "system" fn(
            This,
            *mut u32,
            *mut PackedMips,
            *mut [u32; 3],
            *mut u32,
            u32,
            *mut SubresourceTiling,
        ) -> HRESULT;
        let tiling: Tiling = slot(c, 10);
        let (mut tiles, mut shape, mut n) = (0, [0; 3], 4);
        let mut packed = PackedMips {
            standard: 0,
            packed: 0,
            tiles: 0,
            start: 0,
        };
        let mut subs: Vec<SubresourceTiling> = (0..4)
            .map(|_| SubresourceTiling {
                width: 0,
                height: 0,
                depth: 0,
                start: 0,
            })
            .collect();
        assert_eq!(
            tiling(
                c,
                &mut tiles,
                &mut packed,
                &mut shape,
                &mut n,
                0,
                subs.as_mut_ptr()
            ),
            S_OK
        );
        assert_eq!(
            (tiles, packed.standard, packed.packed, packed.tiles),
            (2, 1, 8, 1)
        );
        assert_eq!((shape, n, subs[0].start), ([256, 256, 1], 1, 1));
        assert_eq!(release(c), 0);

        // 4 KiB modes have no 64 KiB tiling.
        let null = std::ptr::null_mut::<u8>();
        let c = create(&desc(256, 256, 0, 98, 0x105));
        assert_eq!(
            tiling(
                c,
                &mut tiles,
                null.cast(),
                null.cast(),
                null.cast(),
                0,
                null.cast()
            ),
            E_FAIL
        );
        assert_eq!(release(c), 0);
    }

    /// An array with a mip tail, as Microsoft's library lists it.
    #[test]
    fn tiling_counts_subresources_and_zeroes_packed_mips() {
        let mut d = desc(256, 256, 7, 28, 0x10a);
        d.depth_or_array = 3;
        let c = create(&d);
        let zero = SubresourceTiling {
            width: 0,
            height: 0,
            depth: 0,
            start: 0,
        };
        let mut subs = [zero; 8];
        let (mut n, null) = (8u32, std::ptr::null_mut::<u8>());
        let hr = tiling(
            c,
            null.cast(),
            null.cast(),
            null.cast(),
            &mut n,
            0,
            subs.as_mut_ptr(),
        );
        // Indexed by subresource: slice 0's two standard mips, then its
        // packed mips, zeroed.
        // Room for 8 of the 21 subresources: slice 0's standard mips (0, 1),
        // its packed mips zeroed, slice 1's mip 0 (7): three standard mips.
        assert_eq!((hr, n), (S_OK, 3));
        assert!(subs[..2].iter().all(|s| s.depth == 1));
        assert!(subs[2..7].iter().all(|s| s.depth == 0 && s.start == 0));
        assert_eq!(subs[7].depth, 1);
        assert_eq!(release(c), 0);
    }

    /// With `VICTORY_XG_DLL` naming Microsoft's library, our layouts and
    /// tilings of the descriptions Grand Theft Auto VI uses match its own.
    #[test]
    fn matches_microsofts_library_when_present() {
        let Some(o) = xg_oracle::get() else {
            return;
        };
        let mut descs = Vec::new();
        for (w, h, mips, format, layout) in [
            (256, 256, 9, 98, 0x10a),
            (256, 256, 9, 83, 0x10a),
            (256, 256, 9, 71, 0x10a),
            (512, 512, 10, 98, 0x10a),
            (2048, 1024, 9, 98, 0x10a),
            (1024, 2048, 9, 99, 0x10a),
            (512, 512, 10, 98, 0x105),
            (128, 128, 6, 71, 0x105),
            (256, 256, 7, 28, 0x10a),
        ] {
            for array in [1, 2, 3, 4, 8, 32] {
                let mut d = desc(w, h, mips, format, layout);
                d.depth_or_array = array;
                descs.push(d);
            }
        }
        for d in &descs {
            let c = create(d);
            let real = o.computer(d);
            assert!(!real.is_null(), "{d:?}");
            // SAFETY: plain data, filled by the calls.
            let (mut a, mut b): (Box<LayoutOut>, Box<LayoutOut>) =
                unsafe { (Box::new(std::mem::zeroed()), Box::new(std::mem::zeroed())) };
            layout(c, &mut *a);
            // SAFETY: a live real computer.
            let get: extern "system" fn(*mut c_void, *mut LayoutOut) -> HRESULT =
                unsafe { xg_oracle::slot(real, 2) };
            get(real, &mut *b);
            assert_eq!(
                xg_oracle::layout_diffs(&a, &b),
                Vec::<String>::new(),
                "{d:?}"
            );
            if a.alignment == TILE {
                for (first, room) in (0..3).flat_map(|f| [(f, 16), (f, 64)]) {
                    let ((hr, n, ours), (real_hr, real_n, theirs)) =
                        (tilings(c, first, room), real_tilings(real, first, room));
                    let common = n.min(real_n) as usize;
                    assert_eq!(hr, real_hr, "{d:?} first {first} room {room}");
                    assert_eq!(
                        ours[..common],
                        theirs[..common],
                        "{d:?} first {first} room {room}"
                    );
                    assert_eq!(n, real_n, "{d:?} first {first} room {room}");
                }
            }
            o.release(real);
            assert_eq!(release(c), 0);
        }
        eprintln!("{} descriptions match xg_xs.dll", descs.len());
    }

    type Tilings = (HRESULT, u32, Vec<SubresourceTiling>);

    fn tilings(c: This, first: u32, room: u32) -> Tilings {
        let zero = SubresourceTiling {
            width: 0,
            height: 0,
            depth: 0,
            start: 0,
        };
        let (mut subs, mut n) = (vec![zero; room as usize], room);
        let null = std::ptr::null_mut::<u8>();
        let hr = tiling_ours(
            c,
            null.cast(),
            null.cast(),
            null.cast(),
            &mut n,
            first,
            subs.as_mut_ptr(),
        );
        subs.truncate(n as usize);
        (hr, n, subs)
    }

    fn real_tilings(real: *mut c_void, first: u32, room: u32) -> Tilings {
        let zero = SubresourceTiling {
            width: 0,
            height: 0,
            depth: 0,
            start: 0,
        };
        let (mut subs, mut n) = (vec![zero; room as usize], room);
        let null = std::ptr::null_mut::<u8>();
        // SAFETY: a live real computer and room for `room` entries.
        let hr = unsafe {
            xg_oracle::slot::<xg_oracle::Tiling>(real, xg_oracle::SLOT_TILING)(
                real,
                null.cast(),
                null.cast(),
                null.cast(),
                &mut n,
                first,
                subs.as_mut_ptr().cast(),
            )
        };
        subs.truncate(n as usize);
        (hr, n, subs)
    }

    #[test]
    fn bad_descriptions_fail() {
        let mut c = std::ptr::null_mut();
        let bad = desc(256, 256, 1, 0, 0x10a);
        assert_eq!(XGCreateTextureComputer(&bad, &mut c), E_INVALIDARG);
        let bad = desc(256, 256, 1, 28, 0x103);
        assert_eq!(XGCreateTextureComputer(&bad, &mut c), E_INVALIDARG);
        assert!(module().exports.len() == 2);
    }
}
