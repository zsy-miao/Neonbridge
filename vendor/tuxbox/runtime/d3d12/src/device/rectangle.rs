//! Rasterize Xbox screen rectangles from three corners. The fourth corner is
//! implicit, so forwarding the three vertices as a triangle loses half the tile.
use core::{ffi::c_void, fmt::Write, ptr, slice};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, FreeLibrary};
use windows::Win32::Graphics::Direct3D::Dxc::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    LoadLibraryExW,
};
use windows::core::{GUID, HRESULT, Interface, Result, s, w};

// Frosty's pipeline builder maps UE primitive 5 to the Xbox rectangle class at
// RVA 0x57a27c3. The IA packet table at 0x71c0500 maps 0x11 to DI_PT_RECTLIST.
pub(crate) const LIST: i32 = 0x11;
pub(super) const PIPELINE_TYPE: i32 = 0x13;

fn source(vertex: &[u8], primitive_id: bool) -> Result<String> {
    let elements = tuxbox_dxbc::raster_outputs(vertex).ok_or(E_INVALIDARG)?;
    if elements.len() > 64 {
        return Err(E_INVALIDARG.into());
    }
    let mut hlsl = String::from("struct Vertex {\n");
    let mut position = None;
    for (i, el) in elements.iter().enumerate() {
        // Only accept identifiers, scalar types and component masks that DXC can
        // reproduce in a single-stream pass-through declaration.
        if el.name.is_empty()
            || el.name.len() > 256
            || !el
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(E_INVALIDARG.into());
        }
        let ty = match el.comp_type {
            1 => "uint",
            2 => "int",
            3 => "float",
            _ => return Err(E_NOTIMPL.into()),
        };
        let width = el.mask.count_ones();
        if width == 0 || el.mask > 15 || el.mask >> el.mask.trailing_zeros() != (1 << width) - 1 {
            return Err(E_NOTIMPL.into());
        }
        if el.name.eq_ignore_ascii_case("SV_Position") && width == 4 && el.comp_type == 3 {
            position = Some(i);
        }
        // DXIL packs attributes with the same interpolation mode together.
        // Integer varyings are flat, so a float sharing their register is
        // flat as well. Preserve that layout in the pass-through GS.
        let flat = el.sysval == 0
            && elements.iter().any(|other| {
                other.sysval == 0 && other.register == el.register && other.comp_type != 3
            });
        let interpolation = if flat { "nointerpolation " } else { "" };
        writeln!(hlsl, "{interpolation}{ty}{width} v{i}: {}{};", el.name, el.index).unwrap();
    }
    let position = position.ok_or(E_NOTIMPL)?;
    // VIctory-zhai a64b6c6 (GPL-3.0, credit to its authors): channels the
    // vertex shader never writes (OSG1 NeverWrites, `rw_mask` here) stay
    // unread and unwritten in the geometry shader. Copying whole fields set
    // GS.ISG1 read bits VS.OSG1 marks never-written, and
    // CreateGraphicsPipelineState refused the rectangle pipeline
    // (E_INVALIDARG). TUXBOX_RECT_UNWRITTEN=0 copies every declared channel.
    let honour = crate::env_toggle!("TUXBOX_RECT_UNWRITTEN") != Some("0");
    let written: Vec<u8> = elements.iter().map(|el| if honour { el.mask & !el.rw_mask & 15 } else { el.mask }).collect();
    if written.iter().all(|&w| w == 0) || written[position] != 15 {
        return Err(E_NOTIMPL.into());
    }
    // The written channels of element `i`, as field components.
    let channels = |i: usize| {
        let (mask, low) = (written[i], elements[i].mask.trailing_zeros());
        (0..4u32).filter(move |bit| mask >> bit & 1 != 0).map(move |bit| ["x", "y", "z", "w"][(bit - low) as usize])
    };
    hlsl.push_str("};\n");
    hlsl.push_str("Vertex pass(Vertex v) { Vertex r;\n");
    for i in 0..elements.len() {
        for c in channels(i) {
            writeln!(hlsl, "r.v{i}.{c}=v.v{i}.{c};").unwrap();
        }
    }
    hlsl.push_str("return r; }\n");
    writeln!(hlsl, "#define POSITION_FIELD v{position}").unwrap();
    if primitive_id {
        hlsl.push_str("#define PIXEL_PRIMITIVE_ID\n");
    }
    hlsl.push_str(
        "Vertex missing_corner(Vertex prev, Vertex corner, Vertex next) { Vertex result=pass(corner);\n",
    );
    for (i, el) in elements.iter().enumerate() {
        if el.comp_type == 3 {
            for c in channels(i) {
                writeln!(hlsl, "result.v{i}.{c}=prev.v{i}.{c}+next.v{i}.{c}-corner.v{i}.{c};").unwrap();
            }
        }
    }
    hlsl.push_str("return result; }\n");
    hlsl.push_str(include_str!("rectangle.hlsl"));
    Ok(hlsl)
}

fn compile(hlsl: &str) -> Result<Arc<[u8]>> {
    type Create = unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;
    // Keep DXC optional for applications that only submit desktop topologies.
    // Both the compiler and all blobs are released before unloading the module.
    let module = unsafe {
        LoadLibraryExW(
            w!("dxcompiler.dll"),
            None,
            LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        ).inspect_err(|error| {
            static FAILED: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&FAILED, || format!(
                "rectangle shader: dxcompiler.dll load failed {:#010x}; stage matching x64 dxcompiler.dll and dxil.dll",
                error.code().0
            ));
        })?
    };
    let result = (|| unsafe {
        let entry = GetProcAddress(module, s!("DxcCreateInstance")).ok_or(E_NOTIMPL)?;
        let create: Create = core::mem::transmute(entry);
        let mut raw = ptr::null_mut();
        create(&CLSID_DxcCompiler, &IDxcCompiler3::IID, &mut raw).ok()?;
        let compiler = IDxcCompiler3::from_raw(raw);
        let source = DxcBuffer {
            Ptr: hlsl.as_ptr().cast(),
            Size: hlsl.len(),
            Encoding: DXC_CP_UTF8.0,
        };
        let result: IDxcResult = compiler.Compile(
            &source,
            Some(&[w!("-T"), w!("gs_6_0"), w!("-E"), w!("main")]),
            None,
        )?;
        result.GetStatus()?.ok()?;
        let blob = result.GetResult()?;
        Ok(Arc::from(slice::from_raw_parts(
            blob.GetBufferPointer().cast::<u8>(),
            blob.GetBufferSize(),
        )))
    })();
    let _ = unsafe { FreeLibrary(module) };
    result
}

// The caller owns each descriptor's storage and shader blob during PSO creation.
unsafe fn shader_bytes(code: &D3D12_SHADER_BYTECODE) -> Result<&[u8]> {
    if code.BytecodeLength == 0 {
        return Ok(&[]);
    }
    if code.BytecodeLength > isize::MAX as usize || code.pShaderBytecode.is_null() {
        return Err(E_INVALIDARG.into());
    }
    Ok(unsafe { slice::from_raw_parts(code.pShaderBytecode.cast(), code.BytecodeLength) })
}

pub(super) unsafe fn geometry(
    vertex: &D3D12_SHADER_BYTECODE,
    pixel: &D3D12_SHADER_BYTECODE,
) -> Result<Arc<[u8]>> {
    let primitive_id = tuxbox_dxbc::pixel_input_order(unsafe { shader_bytes(pixel)? })
        .iter()
        .any(|s| s.eq_ignore_ascii_case("SV_PrimitiveID0"));
    let hlsl = source(unsafe { shader_bytes(vertex)? }, primitive_id)?;
    // Source depends only on the raster interface. Materials sharing a layout
    // share one compiled geometry program, even with concurrent PSO creation.
    static CACHE: LazyLock<Mutex<HashMap<String, Arc<[u8]>>>> = LazyLock::new(Default::default);
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(code) = cache.get(&hlsl) {
        return Ok(code.clone());
    }
    let code = compile(&hlsl)?;
    if cache.len() < 256 {
        cache.insert(hlsl, code.clone());
    }
    Ok(code)
}

pub(super) fn bytecode(bytes: &[u8]) -> D3D12_SHADER_BYTECODE {
    D3D12_SHADER_BYTECODE {
        pShaderBytecode: bytes.as_ptr().cast(),
        BytecodeLength: bytes.len(),
    }
}

// The descriptor API accepts Xbox shaders as well as portable bytecode. Keep the
// translated stages alive until the host has consumed the synchronous PSO call.
pub(super) unsafe fn create_pipeline(
    device: &ID3D12Device,
    desc: &mut D3D12_GRAPHICS_PIPELINE_STATE_DESC,
) -> Result<ID3D12PipelineState> {
    if [&desc.DS, &desc.HS, &desc.GS]
        .iter()
        .any(|stage| stage.BytecodeLength != 0)
        || desc.StreamOutput.NumEntries != 0
    {
        return Err(E_NOTIMPL.into());
    }
    let vertex = super::shader_cache::prepared(unsafe { shader_bytes(&desc.VS)? });
    let pixel = super::shader_cache::prepared(unsafe { shader_bytes(&desc.PS)? });
    if let Some(code) = &vertex {
        desc.VS = bytecode(code);
    }
    if let Some(code) = &pixel {
        desc.PS = bytecode(code);
    }
    let shader = unsafe { geometry(&desc.VS, &desc.PS) }?;
    desc.GS = bytecode(&shader);
    desc.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
    unsafe { device.CreateGraphicsPipelineState(desc) }
}
