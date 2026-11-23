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
const STENCIL_X: i32 = 26;

fn subobject<T>() -> (usize, usize) {
    let payload = size_of::<u32>().next_multiple_of(align_of::<T>());
    (
        payload,
        (payload + size_of::<T>()).next_multiple_of(align_of::<usize>()),
    )
}

fn layout(tag: i32) -> Option<(usize, usize)> {
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
                        AMPLIFICATION_X => {
                            shader_cache::mesh_stage(code, shader_cache::Stage::Amplification)?
                        }
                        MESH_X => shader_cache::mesh_stage(code, shader_cache::Stage::Mesh)?,
                        1 | 2 => None, // Resolve these together after reading the root subobject.
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
            21 => depth = payload.cast(),
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

    let guest_pixel = original[1].map(|(pixel, _)| pixel);
    pipeline.draw_trace = crate::diagnostics::RasterDraw::trace(original[0].map(|(vertex, _)| vertex), guest_pixel, shader_cache::source_digest);
    let mut rooted = [None, None];
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
            TableLayout::Unsupported => None,
        };
        // A native root table can combine descriptors from both raster stages.
        if let Some(root) = root {
            for (i, source) in [vertex, pixel].into_iter().enumerate() {
                rooted[i] = shader_cache::raster_with_root(source, root.payload());
            }
        }
    }
    for (i, variant) in rooted.iter().enumerate() {
        let Some((code, _)) = original[i] else { continue; };
        let source = variant.as_deref().unwrap_or(code);
        let Some(compiled) = shader_cache::prepared(source) else {
            if variant.is_some() { return Err(E_NOTIMPL.into()); }
            continue;
        };
        let target = fields[i].ok_or(E_INVALIDARG)?;
        unsafe { target.write(rectangle::bytecode(&compiled)) };
        original[i] = Some((source, Some(stages.len())));
        stages.push(compiled);
    }
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
    // SAFETY: the stream storage, translated shaders, and guest-owned inputs
    // all stay alive throughout this synchronous call on the desktop Device2.
    let initial = unsafe { host.CreatePipelineState(&native_desc) };
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
    let producer = if fields[4].is_some() { 4 } else { 0 };
    let direct = fields[5..].iter().all(Option::is_none)
        && (producer == 4 || fields[2..4].iter().all(Option::is_none));
    if direct
        && let (Some(field), Some((source, _)), Some((pixel, _))) =
            (fields[producer], &hashes[producer], &hashes[1])
        && let Some(linked) = (if producer == 4 {
            shader_cache::linked_geometry(source, pixel)
        } else {
            shader_cache::linked_vertex(source, pixel)
        })
        .or_else(|| {
            if status != E_INVALIDARG {
                return None;
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
        let retry = unsafe { host.CreatePipelineState(&native_desc) };
        match &retry {
            Ok(_) => {
                static VERTEX: std::sync::Once = std::sync::Once::new();
                static GEOMETRY: std::sync::Once = std::sync::Once::new();
                let (name, site) = if producer == 4 {
                    ("GS", &GEOMETRY)
                } else {
                    ("VS", &VERTEX)
                };
                crate::diagnostics::log_once(site, || {
                    format!("linked slot 86 {name}={source} PS={pixel}")
                });
            }
            Err(error) => crate::diagnostics::pipeline_stages(&hashes, error.code().0, true),
        }
        if retry.is_ok() { return retry; }
        return unsafe { null_pipeline(host, &native_desc, &fields, stream_output, geometry_record) }.unwrap_or(retry);
    }
    crate::diagnostics::pipeline_stages(&hashes, status.0, false);
    unsafe { null_pipeline(host, &native_desc, &fields, stream_output, geometry_record) }.unwrap_or(initial)
}

/// Retry a rejected stream with resource-free shaders (see `fallback`). Mesh
/// and stream-output pipelines keep their original failure.
unsafe fn null_pipeline(
    host: &ID3D12Device2,
    desc: &D3D12_PIPELINE_STATE_STREAM_DESC,
    fields: &[Option<*mut D3D12_SHADER_BYTECODE>; 8],
    stream_output: bool,
    geometry: *mut D3D12_SHADER_BYTECODE,
) -> Option<windows::core::Result<ID3D12PipelineState>> {
    use super::fallback::{Stage, bytecode, enabled, shader, substituted};
    if !enabled() || stream_output || fields[6].is_some() || fields[7].is_some() {
        return None;
    }
    let compute = fields[5].is_some();
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
    let result = unsafe { host.CreatePipelineState::<ID3D12PipelineState>(desc) };
    if let Ok(pipeline) = &result {
        crate::diagnostics::display::mark_null_pipeline(pipeline.as_raw());
        substituted(if compute { "compute stream" } else { "raster stream" });
    }
    Some(result)
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
    let native = unsafe { create(device, desc, &mut state) };
    crate::diagnostics::pipeline_stream(
        desc.SizeInBytes,
        native.as_ref().err().map(|e| e.code().0),
    );
    unsafe { wrap_pageable(native, &compute::PIPELINE, device, state, out) }
}
