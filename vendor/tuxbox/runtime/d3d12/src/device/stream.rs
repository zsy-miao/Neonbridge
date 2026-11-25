//! Xbox device slot 86. Standard x64 state subobjects use desktop D3D12 layout;
//! the console also appends a stencil subobject and embeds guest object handles.
use super::{Device, Out, compute, identity, rectangle, shader_cache, stencil, wrap_pageable};
use crate::proxy::check_output;
use core::{ffi::c_void, mem::MaybeUninit, ptr, slice};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_POINTER};
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::*};
use windows::core::{GUID, HRESULT, Interface};

// In the packaged mesh builder at RVA 0x57577d0, Xbox AS and MS are one
// lower than the corresponding desktop subobject tags.
const AMPLIFICATION_X: i32 = 23;
const MESH_X: i32 = 24;
pub(super) const STENCIL_X: i32 = 26;

pub(super) mod background;
mod failures;
pub(crate) use background::Upgrade;

fn subobject<T>() -> (usize, usize) {
    let payload = size_of::<u32>().next_multiple_of(align_of::<T>());
    (
        payload,
        (payload + size_of::<T>()).next_multiple_of(align_of::<usize>()),
    )
}

pub(super) fn layout(tag: i32) -> Option<(usize, usize)> {
    Some(match tag {
        0 => subobject::<*mut c_void>(), // root signature
        1..=6 | AMPLIFICATION_X | MESH_X => subobject::<D3D12_SHADER_BYTECODE>(),
        7 => subobject::<D3D12_STREAM_OUTPUT_DESC>(),
        8 => subobject::<D3D12_BLEND_DESC>(),
        9 | 13 | 14 | 16 | 18 | 20 => subobject::<u32>(), // mask, strip cut, topology, DSV, node, flags
        10 => subobject::<D3D12_RASTERIZER_DESC>(),
        11 => subobject::<D3D12_DEPTH_STENCIL_DESC>(),
        12 => subobject::<D3D12_INPUT_LAYOUT_DESC>(),
        15 => subobject::<D3D12_RT_FORMAT_ARRAY>(),
        17 => subobject::<DXGI_SAMPLE_DESC>(),
        19 => subobject::<D3D12_CACHED_PIPELINE_STATE>(),
        21 => subobject::<D3D12_DEPTH_STENCIL_DESC1>(),
        22 => subobject::<D3D12_VIEW_INSTANCING_DESC>(),
        STENCIL_X => (8, 32),
        _ => return None,
    })
}

// The retail builder at RVA 0x57572f0 repeats depth/stencil masks and per-face
// enables at the end of the stream. Dropping that record preserves state only
// when it agrees with DEPTH_STENCIL1 and all extra state fields are inactive.
unsafe fn equivalent_stencil(payload: *const u8, depth: &D3D12_DEPTH_STENCIL_DESC1) -> bool {
    let enabled = u8::from(depth.StencilEnable.as_bool());
    let mut expected = [0; 18];
    expected[12..18].copy_from_slice(&[
        depth.StencilReadMask,
        depth.StencilWriteMask,
        0,
        0,
        enabled,
        enabled,
    ]);
    unsafe { payload.cast::<[u8; 18]>().read_unaligned() == expected }
}

/// A digest of everything a stream describes, stable across runs: shader and
/// root signature contents in place of pointers, and each state structure by
/// its fields (guest padding is uninitialized). `guest` reads Xbox tags and
/// guest roots; otherwise the stream is a desktop one made by `create`, whose
/// root pointer is left out (translated shaders already depend on it).
unsafe fn canonical(guest: Option<&Device>, desc: &D3D12_PIPELINE_STATE_STREAM_DESC) -> Option<String> {
    let (text, _) = unsafe { canonical_parts(guest, desc) }?;
    shader_cache::source_digest(text.as_bytes())
}

/// The canonical text of a stream and, per record, (tag, end of its text).
unsafe fn canonical_parts(guest: Option<&Device>, desc: &D3D12_PIPELINE_STATE_STREAM_DESC) -> Option<(String, Vec<(i32, usize)>)> {
    use core::fmt::Write;
    fn field<T: core::fmt::Debug>(out: &mut String, payload: *const u8) {
        let _ = write!(out, "{:?};", unsafe { &*payload.cast::<T>() });
    }
    fn bytes(out: &mut String, data: *const c_void, length: usize) -> Option<()> {
        if length == 0 {
            out.push_str("-;");
            return Some(());
        }
        if data.is_null() || length > isize::MAX as usize {
            return None;
        }
        let code = unsafe { slice::from_raw_parts(data.cast::<u8>(), length) };
        let _ = write!(out, "{};", shader_cache::source_digest(code)?);
        Some(())
    }
    fn name(out: &mut String, name: windows::core::PCSTR) {
        if !name.is_null() {
            out.push_str(&unsafe { core::ffi::CStr::from_ptr(name.0.cast()) }.to_string_lossy());
        }
        out.push(';');
    }
    let size = desc.SizeInBytes;
    let input = desc.pPipelineStateSubobjectStream.cast::<u8>();
    if input.is_null() || size == 0 || size > 8192 || !size.is_multiple_of(size_of::<usize>()) {
        return None;
    }
    let mut out = String::new();
    let mut parts = Vec::new();
    let mut at = 0;
    while at < size {
        let record = unsafe { input.add(at) };
        let tag = unsafe { record.cast::<i32>().read() };
        // Desktop AS and MS are one above the console tags.
        let (offset, bytes_used) = match (guest.is_some(), tag) {
            (false, 24 | 25) => subobject::<D3D12_SHADER_BYTECODE>(),
            (false, AMPLIFICATION_X | STENCIL_X..) => return None,
            _ => layout(tag)?,
        };
        if bytes_used > size - at {
            return None;
        }
        let payload = unsafe { record.add(offset) };
        let _ = write!(out, "{tag}:");
        match tag {
            0 => {
                if let Some(device) = guest {
                    let root = unsafe { compute::root_proxy(payload.cast::<*mut c_void>().read(), identity(device).as_raw()) }.ok()?;
                    match root {
                        Some(root) => {
                            let blob = root.state.payload();
                            bytes(&mut out, blob.as_ptr().cast(), blob.len())?;
                        }
                        None => out.push_str("-;"),
                    }
                }
            }
            1..=6 | 23..=25 => {
                let shader = unsafe { &*payload.cast::<D3D12_SHADER_BYTECODE>() };
                bytes(&mut out, shader.pShaderBytecode, shader.BytecodeLength)?;
            }
            7 => {
                let so = unsafe { &*payload.cast::<D3D12_STREAM_OUTPUT_DESC>() };
                if so.NumEntries > 4096 || so.NumStrides > 4096 { return None; }
                for i in 0..so.NumEntries as usize {
                    if so.pSODeclaration.is_null() { return None; }
                    let e = unsafe { &*so.pSODeclaration.add(i) };
                    name(&mut out, e.SemanticName);
                    let _ = write!(out, "{},{},{},{},{};", e.Stream, e.SemanticIndex, e.StartComponent, e.ComponentCount, e.OutputSlot);
                }
                for i in 0..so.NumStrides as usize {
                    if so.pBufferStrides.is_null() { return None; }
                    let _ = write!(out, "{},", unsafe { so.pBufferStrides.add(i).read() });
                }
                let _ = write!(out, "{};", so.RasterizedStream);
            }
            8 => field::<D3D12_BLEND_DESC>(&mut out, payload),
            9 | 13 | 14 | 16 | 18 | 20 => field::<u32>(&mut out, payload),
            10 => field::<D3D12_RASTERIZER_DESC>(&mut out, payload),
            11 => field::<D3D12_DEPTH_STENCIL_DESC>(&mut out, payload),
            12 => {
                let input = unsafe { &*payload.cast::<D3D12_INPUT_LAYOUT_DESC>() };
                if input.NumElements > 4096 { return None; }
                for i in 0..input.NumElements as usize {
                    if input.pInputElementDescs.is_null() { return None; }
                    let e = unsafe { &*input.pInputElementDescs.add(i) };
                    name(&mut out, e.SemanticName);
                    let _ = write!(out, "{},{},{},{},{},{};", e.SemanticIndex, e.Format.0, e.InputSlot, e.AlignedByteOffset, e.InputSlotClass.0, e.InstanceDataStepRate);
                }
            }
            15 => field::<D3D12_RT_FORMAT_ARRAY>(&mut out, payload),
            17 => field::<DXGI_SAMPLE_DESC>(&mut out, payload),
            19 => {}
            21 => field::<D3D12_DEPTH_STENCIL_DESC1>(&mut out, payload),
            22 => {
                let views = unsafe { &*payload.cast::<D3D12_VIEW_INSTANCING_DESC>() };
                if views.ViewInstanceCount > 4096 { return None; }
                for i in 0..views.ViewInstanceCount as usize {
                    if views.pViewInstanceLocations.is_null() { return None; }
                    let l = unsafe { views.pViewInstanceLocations.add(i).read() };
                    let _ = write!(out, "{},{};", l.ViewportArrayIndex, l.RenderTargetArrayIndex);
                }
                let _ = write!(out, "{};", views.Flags.0);
            }
            STENCIL_X => {
                // The console stencil record ends the stream (see `equivalent_stencil`).
                let state = unsafe { payload.cast::<[u8; 18]>().read_unaligned() };
                let _ = write!(out, "{state:?}");
                parts.push((tag, out.len()));
                break;
            }
            _ => return None,
        }
        parts.push((tag, out.len()));
        at += bytes_used;
    }
    Some((out, parts))
}

/// What identifies a guest stream across runs (see `pipeline_cache`).
unsafe fn guest_key(device: &Device, desc: &D3D12_PIPELINE_STATE_STREAM_DESC) -> Option<(String, String, Vec<(i32, usize)>)> {
    let (text, parts) = unsafe { canonical_parts(Some(device), desc) }?;
    Some((shader_cache::source_digest(text.as_bytes())?, text, parts))
}

/// Census of the guest pipelines the title creates (test139: 84,186 at
/// load): distinct programs, distinct program sets and distinct pipelines
/// (the guest key, which adds the state). The driver's compile cost goes by
/// the distinct sets and states, not the count. Logged at powers of two from
/// 1024 and every 20,000. TUXBOX_PIPELINE_CENSUS=0 off.
///
/// test140 had 40,000 distinct keys over 693 program sets. To name the state
/// that varies, each new key is compared record by record with the first
/// pipeline of its program set; the line counts the differing records by
/// tag, and the first few differences of each tag are logged in full.
unsafe fn census(desc: &D3D12_PIPELINE_STATE_STREAM_DESC, key: Option<&str>, text: Option<&(String, Vec<(i32, usize)>)>) {
    use std::collections::{HashMap, HashSet};
    use std::hash::{Hash, Hasher};
    #[derive(Default)]
    struct Census {
        by_blob: HashMap<(usize, usize), u64>,
        programs: HashSet<u64>,
        sets: HashSet<u64>,
        keys: HashSet<String>,
        count: u64,
        /// Per program set, the first pipeline's records as (tag, text).
        first: HashMap<u64, Vec<(i32, String)>>,
        /// Differing records by tag (-1: a different record list).
        differs: std::collections::BTreeMap<i32, u64>,
    }
    static CENSUS: std::sync::Mutex<Option<Census>> = std::sync::Mutex::new(None);
    if crate::env_toggle!("TUXBOX_PIPELINE_CENSUS") == Some("0") {
        return;
    }
    let (input, size) = (desc.pPipelineStateSubobjectStream.cast::<u8>().cast_const(), desc.SizeInBytes);
    if input.is_null() {
        return;
    }
    let mut blobs = Vec::new();
    let mut at = 0;
    while at < size {
        let record = unsafe { input.add(at) };
        let tag = unsafe { record.cast::<i32>().read() };
        let Some((offset, used)) = (match tag {
            24 | 25 => Some(subobject::<D3D12_SHADER_BYTECODE>()),
            AMPLIFICATION_X | STENCIL_X.. => None,
            _ => layout(tag),
        }) else { break };
        if used > size - at {
            break;
        }
        if matches!(tag, 1..=6 | 23..=25) {
            let shader = unsafe { &*record.add(offset).cast::<D3D12_SHADER_BYTECODE>() };
            if !shader.pShaderBytecode.is_null() && shader.BytecodeLength != 0 && shader.BytecodeLength <= isize::MAX as usize {
                blobs.push((tag, shader.pShaderBytecode as usize, shader.BytecodeLength));
            }
        }
        at += used;
    }
    let mut g = CENSUS.lock().unwrap_or_else(|e| e.into_inner());
    let c = g.get_or_insert_with(Default::default);
    let mut set = std::collections::hash_map::DefaultHasher::new();
    for (tag, ptr, len) in blobs {
        let program = *c.by_blob.entry((ptr, len)).or_insert_with(|| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            unsafe { slice::from_raw_parts(ptr as *const u8, len) }.hash(&mut h);
            h.finish()
        });
        c.programs.insert(program);
        (tag, program).hash(&mut set);
    }
    let set = set.finish();
    c.sets.insert(set);
    let new_key = key.is_some_and(|key| c.keys.insert(key.to_owned()));
    if let Some((text, parts)) = text {
        let records: Vec<(i32, String)> = parts.iter().scan(0, |start, &(tag, end)| {
            let piece = text.get(*start..end).unwrap_or_default().to_owned();
            *start = end;
            Some((tag, piece))
        }).collect();
        match c.first.get(&set) {
            None => { c.first.insert(set, records); }
            Some(first) if new_key => {
                let tags = |r: &[(i32, String)]| r.iter().map(|(t, _)| *t).collect::<Vec<_>>();
                let mut differing = Vec::new();
                if tags(first) != tags(&records) {
                    differing.push((-1, format!("{:?}", tags(first)), format!("{:?}", tags(&records))));
                } else {
                    for ((tag, a), (_, b)) in first.iter().zip(&records) {
                        if a != b {
                            differing.push((*tag, a.clone(), b.clone()));
                        }
                    }
                }
                for (tag, a, b) in differing {
                    let seen = c.differs.entry(tag).or_default();
                    *seen += 1;
                    if *seen <= 3 {
                        let cut = |t: &str| t.chars().take(600).collect::<String>();
                        crate::diagnostics::log_line(format_args!(
                            "pipeline census: record {tag} differs within a program set: first {} | this {}", cut(&a), cut(&b)));
                    }
                }
            }
            Some(_) => {}
        }
    }
    c.count += 1;
    let n = c.count;
    if (n >= 1024 && n.is_power_of_two()) || n.is_multiple_of(20_000) {
        crate::diagnostics::log_line(format_args!(
            "pipeline census: {n} created; {} distinct programs, {} distinct program sets, {} distinct pipelines (with state); records differing from the set's first pipeline, by tag: {:?} (TUXBOX_PIPELINE_CENSUS=0 off)",
            c.programs.len(), c.sets.len(), c.keys.len(), c.differs));
    }
}

/// Whether a stream has a graphics stage (vertex, pixel, domain, hull,
/// geometry, amplification or mesh shader), not only a compute shader.
pub(super) unsafe fn is_graphics(desc: &D3D12_PIPELINE_STATE_STREAM_DESC) -> bool {
    let (input, size) = (desc.pPipelineStateSubobjectStream.cast::<u8>().cast_const(), desc.SizeInBytes);
    if input.is_null() {
        return false;
    }
    let mut at = 0;
    while at < size {
        let tag = unsafe { input.add(at).cast::<i32>().read() };
        if matches!(tag, 1..=5 | 23..=25) {
            return true;
        }
        let Some((_, used)) = (match tag {
            24 | 25 => Some(subobject::<D3D12_SHADER_BYTECODE>()),
            AMPLIFICATION_X | STENCIL_X.. => None,
            _ => layout(tag),
        }) else { break };
        if used == 0 || used > size - at {
            break;
        }
        at += used;
    }
    false
}

/// The DXBC digests of a stream's shaders (diagnostics capture).
pub(super) unsafe fn shader_digests(desc: &D3D12_PIPELINE_STATE_STREAM_DESC) -> Vec<String> {
    let (input, size) = (desc.pPipelineStateSubobjectStream.cast::<u8>().cast_const(), desc.SizeInBytes);
    let mut digests = Vec::new();
    if input.is_null() {
        return digests;
    }
    let mut at = 0;
    while at < size {
        let record = unsafe { input.add(at) };
        let tag = unsafe { record.cast::<i32>().read() };
        let Some((offset, used)) = (match tag {
            24 | 25 => Some(subobject::<D3D12_SHADER_BYTECODE>()),
            AMPLIFICATION_X | STENCIL_X.. => None,
            _ => layout(tag),
        }) else { break };
        if used > size - at {
            break;
        }
        if matches!(tag, 1..=6 | 23..=25) {
            let shader = unsafe { &*record.add(offset).cast::<D3D12_SHADER_BYTECODE>() };
            if !shader.pShaderBytecode.is_null() && shader.BytecodeLength != 0 {
                let bytes = unsafe { slice::from_raw_parts(shader.pShaderBytecode.cast::<u8>(), shader.BytecodeLength) };
                digests.extend(super::shader_cache::source_digest(bytes));
            }
        }
        at += used;
    }
    digests
}

/// The pipeline library name of a desktop stream made by `create`.
/// The DXIL bytes of a desktop stream's shaders (for the compile budget).
pub(super) unsafe fn shader_bytes(desc: &D3D12_PIPELINE_STATE_STREAM_DESC) -> u64 {
    let (input, size) = (desc.pPipelineStateSubobjectStream.cast::<u8>().cast_const(), desc.SizeInBytes);
    if input.is_null() {
        return 0;
    }
    let (mut at, mut total) = (0, 0u64);
    while at < size {
        let record = unsafe { input.add(at) };
        let tag = unsafe { record.cast::<i32>().read() };
        let Some((offset, used)) = (match tag {
            24 | 25 => Some(subobject::<D3D12_SHADER_BYTECODE>()),
            AMPLIFICATION_X | STENCIL_X.. => None,
            _ => layout(tag),
        }) else { break };
        if used > size - at {
            break;
        }
        if matches!(tag, 1..=6 | 23..=25) {
            let shader = unsafe { &*record.add(offset).cast::<D3D12_SHADER_BYTECODE>() };
            total += shader.BytecodeLength as u64;
        }
        at += used;
    }
    total
}

pub(super) unsafe fn native_name(desc: &D3D12_PIPELINE_STATE_STREAM_DESC) -> Option<String> {
    unsafe { canonical(None, desc) }
}

unsafe fn create(
    device: &Device,
    desc: &D3D12_PIPELINE_STATE_STREAM_DESC,
    pipeline: &mut compute::PipelineData,
) -> windows::core::Result<ID3D12PipelineState> {
    let host = device.state.pipeline_device.as_ref().ok_or(E_NOINTERFACE)?;
    let mut size = desc.SizeInBytes;
    let input = desc.pPipelineStateSubobjectStream.cast::<u8>();
    if input.is_null() {
        return Err(E_POINTER.into());
    }
    // Even a stream containing every supported subobject once fits in 4 KiB.
    if size == 0
        || size > 4096
        || !size.is_multiple_of(size_of::<usize>())
        || !input.cast::<usize>().is_aligned()
    {
        return Err(E_INVALIDARG.into());
    }
    // Guest structures can have uninitialized alignment padding. Copy their
    // representation verbatim and only interpret the fields selected by tags.
    let (gs_offset, gs_size) = subobject::<D3D12_SHADER_BYTECODE>();
    let mut storage = vec![MaybeUninit::<usize>::uninit(); (size + gs_size) / size_of::<usize>()];
    let output = storage.as_mut_ptr().cast::<u8>();
    unsafe { ptr::copy_nonoverlapping(input, output, size) };
    let mut depth: *mut D3D12_DEPTH_STENCIL_DESC1 = ptr::null_mut();
    let mut formats: *mut D3D12_RT_FORMAT_ARRAY = ptr::null_mut();
    let mut blend: *mut D3D12_BLEND_DESC = ptr::null_mut();
    let mut samples = 1;
    let mut rectangles = false;
    let mut stream_output = false;
    let mut geometry_record = ptr::null_mut::<D3D12_SHADER_BYTECODE>();
    let mut stages = Vec::new();
    let mut original = [None; 8];
    let mut fields = [None; 8];
    let mut root_record = None;
    let mut guest_root = None;
    let mut node_mask = 0;
    let mut seen = 0u32;
    let mut at = 0;
    while at < size {
        // Each validated subobject begins on an x64 pointer boundary.
        let record = unsafe { output.add(at) };
        let tag = unsafe { record.cast::<i32>().read() };
        let (offset, bytes) = layout(tag).ok_or(E_NOTIMPL)?;
        if bytes > size - at || seen & (1 << tag) != 0 {
            return Err(E_INVALIDARG.into());
        }
        seen |= 1 << tag;
        let payload = unsafe { record.add(offset) };
        match tag {
            0 => {
                let handle = payload.cast::<*mut c_void>();
                let root =
                    unsafe { compute::root_proxy(handle.read(), identity(device).as_raw()) }?;
                root_record = Some(handle);
                guest_root = root.map(|p| &p.state);
                // The guest keeps its root (and therefore this native interface)
                // alive until CreatePipelineState returns.
                unsafe { handle.write(root.map_or(ptr::null_mut(), |p| p.native.as_raw())) };
            }
            1..=6 | AMPLIFICATION_X | MESH_X => {
                let index = match tag {
                    AMPLIFICATION_X | MESH_X => {
                        let native = if tag == AMPLIFICATION_X {
                            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_AS
                        } else {
                            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_MS
                        };
                        unsafe { record.cast::<i32>().write(native.0) };
                        (6 + tag - AMPLIFICATION_X) as usize
                    }
                    _ => (tag - 1) as usize,
                };
                let shader = unsafe { &mut *payload.cast::<D3D12_SHADER_BYTECODE>() };
                if tag == 5 {
                    geometry_record = shader;
                }
                if shader.BytecodeLength != 0 {
                    if shader.pShaderBytecode.is_null()
                        || shader.BytecodeLength > isize::MAX as usize
                    {
                        return Err(E_INVALIDARG.into());
                    }
                    let code = unsafe {
                        slice::from_raw_parts(shader.pShaderBytecode.cast(), shader.BytecodeLength)
                    };
                    fields[index] = Some(ptr::from_mut(shader));
                    let compiled = match tag {
                        1 | 2 | AMPLIFICATION_X | MESH_X => None,
                        _ => shader_cache::prepared(code),
                    };
                    original[index] = Some((code, compiled.as_ref().map(|_| stages.len())));
                    if let Some(compiled) = compiled {
                        shader.pShaderBytecode = compiled.as_ptr().cast();
                        shader.BytecodeLength = compiled.len();
                        stages.push(compiled);
                    }
                }
            }
            16 => {
                // Depth-stencil format; Scarlett-only depth formats use a host family.
                let format = payload.cast::<windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT>();
                unsafe { format.write(super::format::host_format(format.read())) };
            }
            7 => {
                stream_output =
                    unsafe { (*payload.cast::<D3D12_STREAM_OUTPUT_DESC>()).NumEntries != 0 }
            }
            8 => blend = payload.cast(),
            14 if unsafe { payload.cast::<i32>().read() } == rectangle::PIPELINE_TYPE => {
                rectangles = true;
                unsafe {
                    payload
                        .cast::<i32>()
                        .write(D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE.0)
                };
            }
            15 => {
                formats = payload.cast();
                let targets = unsafe { &mut *formats };
                let mut none = windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
                super::format::host_targets(&mut targets.RTFormats, &mut none);
            }
            17 => samples = unsafe { (*payload.cast::<DXGI_SAMPLE_DESC>()).Count },
            18 => node_mask = unsafe { payload.cast::<u32>().read() },
            // Cached native Xbox PSOs are hints, specific to the console driver.
            19 => unsafe {
                payload
                    .cast::<D3D12_CACHED_PIPELINE_STATE>()
                    .write(Default::default())
            },
            21 => {
                depth = payload.cast();
                // c96b4f2: a GPU without the depth bounds test makes the
                // pipeline without it (its draws pass at every depth).
                if let Some(d) = unsafe { depth.as_mut() }
                    && d.DepthBoundsTestEnable.as_bool()
                    && !super::features::depth_bounds_supported(device)
                {
                    d.DepthBoundsTestEnable = false.into();
                    static ONCE: std::sync::Once = std::sync::Once::new();
                    crate::diagnostics::log_once(&ONCE, || "a pipeline enables the depth bounds test, which the host GPU does not support: made without it (VIctory-fxceless c96b4f2)".to_string());
                }
            }
            STENCIL_X => {
                let depth = unsafe { depth.as_ref() }.ok_or(E_NOTIMPL)?;
                if at + bytes != size || !unsafe { equivalent_stencil(payload, depth) } {
                    return Err(E_NOTIMPL.into());
                }
                size = at;
                break;
            }
            _ => {}
        }
        at += bytes;
    }

    if let (Some((mesh, _)), Some(root), Some(field)) = (original[7], guest_root, root_record)
        && original[2..7].iter().all(Option::is_none)
        && original[0].is_none()
        && let Some(table) = super::mesh_table::reflected(
            &device.native, root, mesh, original[1].map(|(pixel, _)| pixel), node_mask,
        )?
    {
        unsafe { field.write(table.native.as_raw()) };
        pipeline.raster_table = Some(table);
    }
    for (index, stage) in [(6, shader_cache::Stage::Amplification), (7, shader_cache::Stage::Mesh)] {
        let Some((code, _)) = original[index] else { continue };
        let compiled = shader_cache::mesh_stage(code, stage, guest_root.map(|root| root.payload()));
        if compiled.is_err() && let Some(root) = guest_root {
            super::compute_table::capture_stage_root(code, root);
        }
        if let Some(compiled) = compiled? {
            let field = fields[index].ok_or(E_INVALIDARG)?;
            unsafe { field.write(rectangle::bytecode(&compiled)) };
            original[index] = Some((code, Some(stages.len())));
            stages.push(compiled);
        }
    }

    let guest_pixel = original[1].map(|(pixel, _)| pixel);
    pipeline.draw_trace = crate::diagnostics::RasterDraw::trace(original[0].map(|(vertex, _)| vertex), guest_pixel, shader_cache::source_digest);
    if let Some(pixel) = guest_pixel
        && let Some(vertex) = original[0].map(|(code, _)| code)
        && let Some(root) = guest_root
        && let Some(filter) = std::env::var_os("TUXBOX_CAPTURE_RASTER_PAIR")
        && let Some(filter) = filter.to_str()
        && shader_cache::source_digest(pixel).is_some_and(|digest| filter.split(',').any(|prefix| !prefix.is_empty() && digest.starts_with(prefix)))
    {
        super::compute_table::capture_stage_root(vertex, root);
        super::compute_table::capture_stage_root(pixel, root);
    }
    let mut rooted = [None, None];
    if let Some(root) = guest_root && original[0].is_some() && original[1].is_some()
        && (root_record.is_none() || original[2..].iter().any(Option::is_some))
    {
        raster_root_diagnostic(root.payload(), if root_record.is_none() { "no root record" } else { "extra stages" });
    }
    // The root signature the stages' registers follow (for typed null views).
    let mut binding_root: Option<Vec<u8>> = guest_root.map(|root| root.payload().to_vec());
    if let (Some(root), Some((vertex, _)), Some((pixel, _)), Some(field)) =
        (guest_root, original[0], original[1], root_record)
        && original[2..].iter().all(Option::is_none)
    {
        use super::compute_table::{Program, TableLayout};
        let layout = super::compute_table::reflected(
            &device.native, root, Program::Raster { vertex, pixel }, node_mask,
        )?;
        let root = match layout {
            TableLayout::Original => Some(root),
            TableLayout::Reflected(table) => {
                unsafe { field.write(table.native.as_raw()) };
                Some(&pipeline.raster_table.insert(table).layout)
            }
            TableLayout::Unsupported => {
                raster_root_diagnostic(root.payload(), "unsupported table layout");
                // Experimental: translate against the guest root unchanged.
                std::env::var_os("TUXBOX_RASTER_ROOT_FALLBACK").map(|_| root)
            }
        };
        // A native root table can combine descriptors from both raster stages.
        if let Some(root) = root {
            binding_root = Some(root.payload().to_vec());
            for (i, source) in [vertex, pixel].into_iter().enumerate() {
                rooted[i] = shader_cache::raster_with_root(source, root.payload());
            }
        }
    }
    for (i, variant) in rooted.iter().enumerate() {
        let Some((code, _)) = original[i] else { continue; };
        let source = variant.as_deref().unwrap_or(code);
        let quad = i == 1 && QUAD_PS.with(|q| q.get());
        let compiled = if quad { shader_cache::quad_mask_variant(source) } else { shader_cache::prepared(source) };
        let Some(compiled) = compiled else {
            if variant.is_some() || quad { return Err(E_NOTIMPL.into()); }
            continue;
        };
        let target = fields[i].ok_or(E_INVALIDARG)?;
        unsafe { target.write(rectangle::bytecode(&compiled)) };
        original[i] = Some((source, Some(stages.len())));
        stages.push(compiled);
    }
    // Literal samplers a translated stage looks up in tb_desc_table: reserve
    // their host samplers and add the table's root SRV.
    let declaring: Vec<&[u8]> = original.iter().enumerate()
        .filter(|(i, _)| *i != 5)
        .filter_map(|(_, stage)| match *stage { Some((code, Some(index))) => Some((code, index)), _ => None })
        .filter(|&(_, index)| super::desc_table::declared(&stages[index]))
        .map(|(code, _)| code)
        .collect();
    // A stage that reads typed buffer formats gets the view_formats root SRV.
    let reads_formats = original.iter().enumerate()
        .filter(|(i, _)| *i != 5)
        .filter_map(|(_, stage)| match *stage { Some((_, Some(index))) => Some(index), _ => None })
        .any(|index| super::view_formats::declared(&stages[index]));
    let reads_slots = original.iter().enumerate()
        .filter(|(i, _)| *i != 5)
        .filter_map(|(_, stage)| match *stage { Some((_, Some(index))) => Some(index), _ => None })
        .any(|index| super::slot_views::declared(&stages[index]));
    // Write marks (view_marks): any stage marking gets the table and bitmap;
    // a pipeline writing views without marking turns page write-backs off.
    let translated = || original.iter().enumerate()
        .filter(|(i, _)| *i != 5)
        .filter_map(|(_, stage)| match *stage { Some((_, Some(index))) => Some(index), _ => None });
    let marks = translated().any(|index| super::view_marks::declared(&stages[index]));
    let reads_words = translated().any(|index| super::slot_words::declared(&stages[index]));
    let table_words = translated().fold(Vec::new(), |all, index| super::table_words::union(all, super::table_words::declared(&stages[index])));
    // SetSampleMaskX's quad mask (quad_mask): a stage declaring it gets the constants.
    let quad = translated().any(|index| super::quad_mask::declared(&stages[index]));
    super::view_marks::note_pipeline(translated().any(|index| super::view_marks::writes(&stages[index])), marks);
    if let (true, Some(root), Some(field)) = (!declaring.is_empty() || reads_formats || reads_slots || marks || quad || reads_words || !table_words.is_empty(), guest_root, root_record) {
        let words: Vec<_> = declaring.iter().flat_map(|code| tuxbox_dxbc::inline_sampler::literal_words(code)).collect();
        if !words.is_empty() {
            device.state.desc_table.literals(&device.native, &words);
        }
        let table = super::compute_table::with_runtime_tables(
            &device.native, pipeline.raster_table.as_ref(), root, node_mask, !declaring.is_empty(), reads_formats, reads_slots, marks, quad, false, false, reads_words, &table_words)?;
        unsafe { field.write(table.native.as_raw()) };
        binding_root = Some(table.layout.payload().to_vec());
        pipeline.raster_table = Some(table);
    }
    pipeline.null_kinds = binding_root.as_deref().and_then(|root| {
        super::null_kinds::reflect(root, original.iter().filter_map(|stage| Some(stages[(*stage)?.1?].as_slice())))
    });
    if let Some((code, Some(index))) = original[5]
        && let Some(root) =
            super::compute_scene::private_root(&device.native, code, &stages[index], guest_root, node_mask)?
    {
        if fields
            .iter()
            .enumerate()
            .any(|(i, field)| i != 5 && field.is_some())
        {
            return Err(E_INVALIDARG.into());
        }
        let field = root_record.unwrap_or_else(|| {
            // A compute stream may rely solely on its embedded root. The spare
            // subobject storage reserved above also fits a root signature pointer.
            let (offset, bytes) = subobject::<*mut c_void>();
            let record = unsafe { output.add(size) };
            unsafe { record.cast::<i32>().write(0) };
            size += bytes;
            unsafe { record.add(offset).cast() }
        });
        unsafe { field.write(root.native.as_raw()) };
        pipeline.scene_root = Some(root);
    }
    if let (Some(blend), Some(formats)) = (unsafe { blend.as_mut() }, unsafe { formats.as_ref() }) {
        super::cb_blend::integer_targets_unblended(blend, &formats.RTFormats, formats.NumRenderTargets);
    }
    let stencil_code;
    if let (Some(depth), Some(formats), Some(blend), Some((ps, Some(_))), Some(field)) = (
        unsafe { depth.as_mut() },
        unsafe { formats.as_mut() },
        unsafe { blend.as_mut() },
        original[1],
        fields[1],
    ) && let Some((slot, code)) = stencil::redirect(device, ps, samples, depth, formats, blend)?
    {
        stencil_code = code;
        unsafe {
            field.write(D3D12_SHADER_BYTECODE {
                pShaderBytecode: stencil_code.as_ptr().cast(),
                BytecodeLength: stencil_code.len(),
            });
        }
        pipeline.stencil_slot = Some(slot);
    }
    let pixel_bytecode = || {
        fields[1]
            .and_then(|p| unsafe { p.as_ref() })
            .copied()
            .unwrap_or_default()
    };
    // A mesh pipeline's topology type, given when the guest stream has none:
    // the host derives it from the mesh shader anyway, but a pipeline library
    // compares descriptions with it and loads no mesh pipeline stored from a
    // stream without it (VIctory 9c58a1c, GPL-3.0, with credit).
    // TUXBOX_MESH_TOPOLOGY=0 off.
    if fields[7].is_some()
        && seen & (1 << 14) == 0
        && crate::env_toggle!("TUXBOX_MESH_TOPOLOGY") != Some("0")
    {
        let (offset, bytes) = subobject::<D3D12_PRIMITIVE_TOPOLOGY_TYPE>();
        // The storage reserves one extra subobject (a shader record's size).
        debug_assert!(bytes <= gs_size);
        let record = unsafe { output.add(size) };
        unsafe { ptr::write_bytes(record, 0, bytes) };
        unsafe { record.cast::<i32>().write(14) };
        // The guest stream gave no topology, so the type is the guest's default,
        // UNDEFINED (VIctory pipeline/host.rs:81 and host_topology, GPL-3.0,
        // credit to its authors). TUXBOX_MESH_TOPOLOGY_UNDEFINED=0 writes
        // TRIANGLE as before.
        let topology = if crate::env_toggle!("TUXBOX_MESH_TOPOLOGY_UNDEFINED") == Some("0") {
            D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE
        } else {
            D3D12_PRIMITIVE_TOPOLOGY_TYPE_UNDEFINED
        };
        unsafe { record.add(offset).cast::<i32>().write(topology.0) };
        size += bytes;
    }
    let rectangle_geometry;
    if rectangles {
        if fields[2..].iter().any(Option::is_some) || stream_output {
            return Err(E_NOTIMPL.into());
        }
        let vertex = unsafe { fields[0].ok_or(E_INVALIDARG)?.as_ref() }.ok_or(E_INVALIDARG)?;
        rectangle_geometry = unsafe { rectangle::geometry(vertex, &pixel_bytecode()) }?;
        if geometry_record.is_null() {
            // The storage reserves one extra subobject before taking field pointers.
            let record = unsafe { output.add(size) };
            unsafe { record.cast::<i32>().write(5) };
            geometry_record = unsafe { record.add(gs_offset).cast() };
            size += gs_size;
        }
        unsafe { geometry_record.write(rectangle::bytecode(&rectangle_geometry)) };
    }
    let native_desc = D3D12_PIPELINE_STATE_STREAM_DESC {
        SizeInBytes: size,
        pPipelineStateSubobjectStream: output.cast(),
    };
    // TUXBOX_NULL_SHADERS / TUXBOX_NULL_BISECT: null chosen pipelines (see `fallback`).
    if super::fallback::forcing() {
        let digests: Vec<String> = original.iter().flatten().filter_map(|(code, _)| shader_cache::source_digest(code)).collect();
        if let Some(hash) = super::fallback::forced_null(digests.iter().map(String::as_str)).map(str::to_owned)
            && let Some(result) = unsafe { null_pipeline(host, &native_desc, &fields, stream_output, geometry_record, false) }
        {
            static FORCED: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&FORCED, || format!("forced a null pipeline for shader {hash} (TUXBOX_NULL_SHADERS / TUXBOX_NULL_BISECT; further ones are counted as substituted)"));
            return result;
        }
    }
    // A mesh pipeline with a cached link to its pixel shader (or signatures
    // that do not match) takes the linked mesh shader before the create,
    // instead of a refused driver compile first (five-star-recomp
    // ProducerLinks, 1236bfa, GPL-3.0, credit to its authors).
    // TUXBOX_MESH_LINK_FIRST=0 links only after a refusal, as before.
    let mesh_pre_linked = if fields[7].is_some()
        && fields[0].is_none()
        && fields[2..6].iter().all(Option::is_none)
        && crate::env_toggle!("TUXBOX_MESH_LINK") != Some("0")
        && crate::env_toggle!("TUXBOX_MESH_LINK_FIRST") != Some("0")
        && let (Some((mesh_code, Some(i))), Some((pixel_code, Some(j))), Some(field)) = (original[7], original[1], fields[7])
        && let Some(pixel) = shader_cache::source_digest(pixel_code)
        && let Some(linked) = shader_cache::mesh_link_before(
            mesh_code, guest_root.map(|root| root.payload()), (&pixel, pixel_code), &stages[i], &stages[j], true,
        ) {
        unsafe {
            field.write(D3D12_SHADER_BYTECODE { pShaderBytecode: linked.as_ptr().cast(), BytecodeLength: linked.len() })
        };
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || format!("linked mesh shader to PS={pixel} before the create (TUXBOX_MESH_LINK_FIRST=0 off)"));
        Some(linked)
    } else {
        None
    };
    // SAFETY: the stream storage, translated shaders, and guest-owned inputs
    // all stay alive throughout this synchronous call on the desktop Device2.
    let initial = unsafe { super::pipeline_cache::stream(host, &native_desc) };
    let Err(error) = &initial else {
        if rectangles {
            static FIRST_RECTANGLE: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&FIRST_RECTANGLE, || {
                "created Xbox screen-rectangle pipeline with three-vertex input".into()
            });
        }
        if let Some(table) = &pipeline.raster_table {
            static FIRST: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&FIRST, || format!(
                "created raster pipeline with {} reflected FSSM descriptor slots", table.lengths.iter().map(|&v| u64::from(v)).sum::<u64>()
            ));
        }
        if fields[7].is_some() {
            static FIRST_MESH: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&FIRST_MESH, || {
                format!(
                    "created Xbox mesh pipeline ({}-byte stream, amplification={})",
                    desc.SizeInBytes,
                    fields[6].is_some()
                )
            });
        }
        return initial;
    };
    let status = error.code();
    if let (Some(pixel), Some(root)) = (guest_pixel, guest_root) {
        super::compute_table::capture_stage_root(pixel, root);
    }
    // Do not hash normal successful submissions twice. The consumer layout
    // only matters when the desktop driver rejects the ordinary stage pair.
    let hashes = original.map(|stage| {
        stage.and_then(|(code, prepared)| {
            shader_cache::source_digest(code).map(|hash| (hash, prepared.is_some()))
        })
    });
    // Pixel inputs come from the last active raster stage. Geometry outputs
    // take precedence over vertex outputs, even in a stream that has both.
    // A mesh producer links too (five-star-recomp ddcaa54 and 1236bfa,
    // GPL-3.0, with credit: refused G-buffer mesh pipelines). xshader then
    // lays the mesh outputs out for this pixel shader and drops the ones it
    // does not read (TB_MESH_DROP_UNREAD), which also frees mesh output
    // budget. TUXBOX_MESH_LINK=0 leaves a refused mesh pipeline refused.
    let mesh_link = crate::env_toggle!("TUXBOX_MESH_LINK") != Some("0");
    let producer = if fields[7].is_some() { 7 } else if fields[4].is_some() { 4 } else { 0 };
    let direct = if producer == 7 {
        mesh_link && fields[0].is_none() && fields[2..6].iter().all(Option::is_none)
    } else {
        fields[5..].iter().all(Option::is_none)
            && (producer == 4 || fields[2..4].iter().all(Option::is_none))
    };
    if direct
        && let (Some(field), Some((source, _)), Some((pixel, _))) =
            (fields[producer], &hashes[producer], &hashes[1])
        && let Some(linked) = (match producer {
            4 => shader_cache::linked_geometry(source, pixel),
            // Already in the stream when linked before the create.
            7 if mesh_pre_linked.is_some() => None,
            7 => (|| {
                let (mesh_code, i) = original[7].and_then(|(c, i)| Some((c, i?)))?;
                let (pixel_code, j) = original[1].and_then(|(c, j)| Some((c, j?)))?;
                shader_cache::mesh_link_before(
                    mesh_code, guest_root.map(|root| root.payload()), (pixel, pixel_code), &stages[i], &stages[j], false,
                )
            })(),
            _ => shader_cache::linked_vertex(source, pixel),
        })
        .or_else(|| {
            if status != E_INVALIDARG {
                return None;
            }
            if producer == 7 {
                // The linked form was refused already.
                if mesh_pre_linked.is_some() {
                    return None;
                }
                let (mesh_code, Some(i)) = original[7]? else { return None };
                let (pixel_code, Some(j)) = original[1]? else { return None };
                return shader_cache::link_mesh(
                    mesh_code, guest_root.map(|root| root.payload()), (pixel, pixel_code), &stages[i], &stages[j],
                );
            }
            let (producer_code, Some(i)) = original[producer]? else {
                return None;
            };
            let (pixel_code, Some(j)) = original[1]? else {
                return None;
            };
            shader_cache::link_on_demand(
                (source, producer_code),
                (pixel, pixel_code),
                &stages[i],
                &stages[j],
            )
        })
    {
        // Use this producer only for this exact consumer. It retains the
        // semantics of the original raster producer with a compatible output signature.
        unsafe {
            field.write(D3D12_SHADER_BYTECODE {
                pShaderBytecode: linked.as_ptr().cast(),
                BytecodeLength: linked.len(),
            })
        };
        let linked_rectangle;
        if rectangles {
            linked_rectangle = unsafe { rectangle::geometry(&*field, &pixel_bytecode())? };
            unsafe { geometry_record.write(rectangle::bytecode(&linked_rectangle)) };
        }
        let retry = unsafe { super::pipeline_cache::stream(host, &native_desc) };
        match &retry {
            Ok(_) => {
                static VERTEX: std::sync::Once = std::sync::Once::new();
                static GEOMETRY: std::sync::Once = std::sync::Once::new();
                static MESH: std::sync::Once = std::sync::Once::new();
                let (name, site) = match producer {
                    4 => ("GS", &GEOMETRY),
                    7 => ("MS", &MESH),
                    _ => ("VS", &VERTEX),
                };
                crate::diagnostics::log_once(site, || {
                    format!("linked slot 86 {name}={source} PS={pixel}")
                });
            }
            Err(error) => crate::diagnostics::pipeline_stages(&hashes, error.code().0, true),
        }
        if retry.is_ok() { return retry; }
        let code = retry.as_ref().err().map_or(status.0, |e| e.code().0);
        let result = unsafe { null_pipeline(host, &native_desc, &fields, stream_output, geometry_record, true) }.unwrap_or(retry);
        failures::record("slot86 linked", code, result.is_ok(), &hashes);
        return result;
    }
    crate::diagnostics::pipeline_stages(&hashes, status.0, false);
    let result = unsafe { null_pipeline(host, &native_desc, &fields, stream_output, geometry_record, true) }.unwrap_or(initial);
    failures::record("slot86", status.0, result.is_ok(), &hashes);
    result
}

/// Retry a rejected stream with resource-free shaders (see `fallback`). Mesh
/// and stream-output pipelines keep their original failure. With `flat` and
/// TUXBOX_FALLBACK=flat, a raster stream first keeps its own vertex stages and
/// draws flat magenta.
unsafe fn null_pipeline(
    host: &ID3D12Device2,
    desc: &D3D12_PIPELINE_STATE_STREAM_DESC,
    fields: &[Option<*mut D3D12_SHADER_BYTECODE>; 8],
    stream_output: bool,
    geometry: *mut D3D12_SHADER_BYTECODE,
    flat: bool,
) -> Option<windows::core::Result<ID3D12PipelineState>> {
    use super::fallback::{Stage, bytecode, enabled, flat_enabled, flat_pixel, shader, substituted};
    if !enabled() || stream_output || fields[6].is_some() || fields[7].is_some() {
        return None;
    }
    let compute = fields[5].is_some();
    if flat
        && !compute
        && flat_enabled()
        && let (Some(pixel), Some(code)) = (fields[1], flat_pixel())
    {
        let kept = unsafe { pixel.read() };
        unsafe { pixel.write(bytecode(code)) };
        let result = crate::big_stack::run(|| unsafe { host.CreatePipelineState::<ID3D12PipelineState>(desc) });
        if result.is_ok() {
            substituted("flat-colour pixel");
            return Some(result);
        }
        unsafe { pixel.write(kept) };
    }
    let replacement = |index: usize| match index {
        0 => shader(Stage::Vertex).map(bytecode),
        1 => shader(Stage::Pixel).map(bytecode),
        5 => shader(Stage::Compute).map(bytecode),
        _ => Some(D3D12_SHADER_BYTECODE::default()),
    };
    // Compute shaders first, so a missing compiler leaves the stream untouched.
    let codes: Vec<_> = (0..6).map(|i| fields[i].map(|_| replacement(i))).collect();
    if codes.iter().any(|c| matches!(c, Some(None))) {
        return None;
    }
    for (i, code) in codes.into_iter().enumerate() {
        if let (Some(field), Some(Some(code))) = (fields[i], code) {
            unsafe { field.write(code) };
        }
    }
    if !geometry.is_null() {
        unsafe { geometry.write(D3D12_SHADER_BYTECODE::default()) };
    }
    let result = crate::big_stack::run(|| unsafe { host.CreatePipelineState::<ID3D12PipelineState>(desc) });
    if let Ok(pipeline) = &result {
        background::mark_nulled();
        crate::diagnostics::display::mark_null_pipeline(pipeline.as_raw());
        substituted(if compute { "compute stream" } else { "raster stream" });
    }
    Some(result)
}

thread_local! {
    /// Set by a background worker building a quad-mask variant (draws under a
    /// SetSampleMaskX mask that is not full; commands/graphics.rs draw_quad):
    /// the pixel stage is translated with xshader's tb_quad_mask discard.
    pub(super) static QUAD_PS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The quad-mask variant of `host` (built in the background from the stream
/// it was made from), None while it is being made or when it cannot be.
pub(crate) unsafe fn quad_variant(device: *mut c_void, host: &compute::Pipeline) -> Option<&'static compute::Pipeline> {
    let device = unsafe { &*device.cast::<Device>() };
    use std::sync::{Arc, Mutex};
    static VARIANTS: Mutex<Vec<(usize, Arc<background::Upgrade>)>> = Mutex::new(Vec::new());
    let key = host as *const compute::Pipeline as usize;
    let slot = {
        let mut variants = VARIANTS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, slot)) = variants.iter().find(|(k, _)| *k == key) {
            slot.clone()
        } else {
            let base = host.state.derive_from.as_ref()?;
            // A deep copy of the kept stream (patched with nothing points
            // into `base`, which outlives the copy here).
            let mut storage = base.patched(&[])?;
            let desc = D3D12_PIPELINE_STATE_STREAM_DESC {
                SizeInBytes: storage.len() * size_of::<usize>(),
                pPipelineStateSubobjectStream: storage.as_mut_ptr().cast(),
            };
            let mut stream = unsafe { background::OwnedStream::copy(&desc) }?;
            stream.quad = true;
            let slot = background::lazy((*identity(device)).clone(), background::Work::Stream(stream));
            // Kept for the process (no eviction: draws hold `&'static` to them).
            if variants.len() >= 4096 {
                return None;
            }
            variants.push((key, slot.clone()));
            slot
        }
    };
    let built = background::wait_for(&slot, std::time::Duration::from_millis(200))?;
    // SAFETY: `slot` stays in VARIANTS for the process, so its pipeline does too.
    Some(unsafe { &*(built.pipeline() as *const compute::Pipeline) })
}

pub(super) unsafe extern "system" fn create_pipeline(
    device: &Device,
    desc: *const D3D12_PIPELINE_STATE_STREAM_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12PipelineState>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_POINTER;
    };
    let mut state = compute::PipelineData::default();
    let worker = background::on_worker();
    let mut census_text = None;
    if !worker {
        let keyed = unsafe { guest_key(device, desc) };
        state.key = keyed.as_ref().map(|(key, _, _)| key.clone().into());
        census_text = keyed.map(|(_, text, parts)| (text, parts));
        if let Some(key) = state.key.as_deref()
            && crate::diagnostics::capture::shaders_on()
        {
            crate::diagnostics::capture::pipeline_shaders(key, unsafe { shader_digests(desc) });
        }
    }
    // A pipeline no earlier run bound is made on its first bind (VIctory's
    // lazy pipelines); the ones an earlier run bound are handed to the
    // workers now (see `background::eager`).
    if !worker {
        unsafe { census(desc, state.key.as_deref(), census_text.as_ref()) };
    }
    let used_before = state.key.as_deref().is_some_and(|key| super::pipeline_cache::used_before(key) == Some(true));
    if !worker
        && (!used_before || !background::eager_on_title())
        && let Some(placeholder) = background::placeholder(device)
        && let Some(stream) = unsafe { background::OwnedStream::copy(desc) }
    {
        state.lazy = true;
        state.derive_from = super::derived::keep(desc);
        background::journal::stream(device, state.key.as_deref(), &stream);
        let work = background::Work::Stream(stream);
        let owner = (*identity(device)).clone();
        let _ = state.upgrade.set(if used_before { background::queue(owner, work) } else { background::lazy(owner, work) });
        crate::diagnostics::pipeline_stream(desc.SizeInBytes, None);
        return unsafe { wrap_pageable(Ok(placeholder), &compute::PIPELINE, device, state, out) };
    }
    if !worker {
        state.derive_from = super::derived::keep(desc);
    }
    let _ = shader_cache::take_missed();
    let _ = shader_cache::take_misses();
    let native = unsafe { create(device, desc, &mut state) };
    // A translation missed the wait budget: the title gets this pipeline now
    // and a worker makes the real one from a private copy of the stream.
    if shader_cache::take_missed()
        && native.is_ok()
        && !worker
        && let Some(stream) = unsafe { background::OwnedStream::copy(desc) }
    {
        let _ = state.upgrade.set(background::queue(
            (*identity(device)).clone(),
            background::Work::Stream(stream),
        ));
    }
    crate::diagnostics::pipeline_stream(
        desc.SizeInBytes,
        native.as_ref().err().map(|e| e.code().0),
    );
    if let Err(error) = &native {
        crate::diagnostics::frame_work::add(crate::diagnostics::frame_work::Work::PipelinesRefused);
        if !worker
            && background::refused_placeholder_on()
            && let Some(placeholder) = background::placeholder(device)
        {
            state.lazy = true;
            let _ = state.upgrade.set(background::refused("graphics pipeline", error.code()));
            return unsafe { wrap_pageable(Ok(placeholder), &compute::PIPELINE, device, state, out) };
        }
    }
    unsafe { wrap_pageable(native, &compute::PIPELINE, device, state, out) }
}

/// With TUXBOX_TRACE_RASTER_ROOTS=1, describe each distinct raster root
/// signature that does not reach a rooted shader translation.
fn raster_root_diagnostic(payload: &[u8], reason: &str) {
    use std::collections::HashSet;
    use std::sync::{LazyLock, Mutex};
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("TUXBOX_TRACE_RASTER_ROOTS").is_some_and(|v| !v.is_empty()));
    static SEEN: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
    if !*ON { return; }
    let summary = match tuxbox_dxbc::rootsig::parse_rts0(payload) {
        Ok(sig) => format!("v{} flags={:#x} static_samplers={} params={:?}", sig.version, sig.flags, sig.static_samplers.len(), sig.params.iter().map(|p| &p.kind).collect::<Vec<_>>()),
        Err(e) => format!("unparsed ({e})"),
    };
    let line = format!("raster root without translation ({reason}): {summary}");
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.len() < 40 && seen.insert(line.clone()) {
        crate::diagnostics::log_once(&std::sync::Once::new(), || line);
    }
}
