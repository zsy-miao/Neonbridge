//! Xbox Device8::DeserializeStateObjectX consumes serialized native GPU programs.
use super::{Device, Out, compute::root_proxy, identity, shader_cache};
use crate::proxy::{self, Proxy, Slots};
use core::{ffi::c_void, ptr, slice};
use std::sync::{LazyLock, Once};
use tuxbox_dxbc::ray::State;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_POINTER, S_OK};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{GUID, HRESULT, Interface, PCWSTR, Result};
mod inspection;
use inspection::Inspection;

pub(super) const DEVICE8: GUID = GUID::from_u128(0x728f1fa2_ea11_4a9f_9b52_9b42a484d5c9);
type Object = Proxy<ID3D12StateObject, Inspection>;
static TABLE: Slots<9> = {
    let mut slots = proxy::child_slots::<ID3D12StateObject, Inspection, 9>();
    slots[0] = crate::slot!(query);
    slots[8] = crate::slot!(inspection::get_inspector);
    slots
};

unsafe extern "system" fn query(this: *mut Object, iid: *const GUID, out: Out) -> HRESULT {
    if !iid.is_null()
        && !out.is_null()
        && unsafe { iid.read_unaligned() } == ID3D12StateObjectProperties::IID
    {
        unsafe {
            proxy::add_ref(this);
            out.write(ptr::addr_of_mut!((*this).state).cast());
        }
        return S_OK;
    }
    unsafe { proxy::query(this, iid, out) }
}

pub(super) unsafe fn native<'a>(
    guest: *mut c_void,
    owner: *mut c_void,
) -> Option<&'a ID3D12StateObject> {
    unsafe { proxy::unwrap::<ID3D12StateObject, Inspection>(guest, owner, &TABLE) }
        .map(|s| &s.native)
}

pub(super) unsafe fn wrap_pipeline(
    device: &Device,
    native: ID3D12StateObject,
    desc: *const D3D12_STATE_OBJECT_DESC,
    previous_guest: *mut c_void,
    out: Out,
) -> HRESULT {
    let owner = identity(device).as_raw();
    let mut entries = Vec::new();
    let mut import = |guest, count, names| -> Result<()> {
        let obj = unsafe { proxy::unwrap::<ID3D12StateObject, Inspection>(guest, owner, &TABLE) }
            .ok_or(E_INVALIDARG)?;
        unsafe { obj.state.import(count, names, &mut entries) }
    };
    if !previous_guest.is_null()
        && let Err(e) = import(previous_guest, 0, ptr::null())
    {
        return e.code();
    }
    // link() has already validated the complete subobject array and its collection interfaces.
    let desc = unsafe { &*desc };
    if desc.NumSubobjects != 0 {
        for sub in unsafe { slice::from_raw_parts(desc.pSubobjects, desc.NumSubobjects as usize) } {
            if sub.Type == D3D12_STATE_SUBOBJECT_TYPE_EXISTING_COLLECTION {
                let collection = unsafe { &*sub.pDesc.cast::<D3D12_EXISTING_COLLECTION_DESC>() };
                let guest = collection
                    .pExistingCollection
                    .as_ref()
                    .map_or(ptr::null_mut(), Interface::as_raw);
                if let Err(e) = import(guest, collection.NumExports, collection.pExports) {
                    return e.code();
                }
            }
        }
    }
    let inspection = Inspection::from_exports(&native, entries);
    unsafe { wrap(device, native, inspection, out) }
}

unsafe fn wrap(
    device: &Device,
    native: ID3D12StateObject,
    inspection: Result<Inspection>,
    out: Out,
) -> HRESULT {
    match inspection {
        Ok(info) => unsafe { super::wrap_pageable(Ok(native), &TABLE, device, info, out) },
        Err(error) => error.code(),
    }
}

fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain([0]).collect()
}

fn subobject<T>(kind: D3D12_STATE_SUBOBJECT_TYPE, data: &T) -> D3D12_STATE_SUBOBJECT {
    D3D12_STATE_SUBOBJECT {
        Type: kind,
        pDesc: ptr::from_ref(data).cast(),
    }
}

fn collection(
    device: &Device,
    state: &State,
    root: Option<&ID3D12RootSignature>,
    dxil: &[u8],
) -> Result<ID3D12StateObject> {
    let device: ID3D12Device5 = device.native.cast()?;
    let name = wide(&state.name);
    let group = state.hit_group.as_deref().map(wide);
    let any = state.any_hit.as_deref().map(wide);
    let library = D3D12_DXIL_LIBRARY_DESC {
        DXILLibrary: D3D12_SHADER_BYTECODE {
            pShaderBytecode: dxil.as_ptr().cast(),
            BytecodeLength: dxil.len(),
        },
        ..Default::default()
    };
    let hit_group = D3D12_HIT_GROUP_DESC {
        HitGroupExport: PCWSTR(group.as_ref().map_or(ptr::null(), |w| w.as_ptr())),
        ClosestHitShaderImport: PCWSTR(name.as_ptr()),
        AnyHitShaderImport: PCWSTR(any.as_ref().map_or(ptr::null(), |w| w.as_ptr())),
        Type: D3D12_HIT_GROUP_TYPE_TRIANGLES,
        ..Default::default()
    };
    // Keep shader-config capacities compatible across linked collections.
    // The supported continuations carry up to sixteen dwords and triangle attributes.
    let config = D3D12_RAYTRACING_SHADER_CONFIG {
        MaxPayloadSizeInBytes: 64,
        MaxAttributeSizeInBytes: 8,
    };
    let pipeline = D3D12_RAYTRACING_PIPELINE_CONFIG {
        MaxTraceRecursionDepth: 1,
    };
    let flags = D3D12_STATE_OBJECT_CONFIG {
        Flags: D3D12_STATE_OBJECT_FLAG_ALLOW_STATE_OBJECT_ADDITIONS,
    };
    let mut subs = vec![
        subobject(D3D12_STATE_SUBOBJECT_TYPE_DXIL_LIBRARY, &library),
        subobject(D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_SHADER_CONFIG, &config),
        subobject(
            D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_PIPELINE_CONFIG,
            &pipeline,
        ),
        subobject(D3D12_STATE_SUBOBJECT_TYPE_STATE_OBJECT_CONFIG, &flags),
    ];
    let root = root.map_or(ptr::null_mut(), Interface::as_raw);
    if !root.is_null() {
        subs.push(subobject(
            D3D12_STATE_SUBOBJECT_TYPE_GLOBAL_ROOT_SIGNATURE,
            &root,
        ));
    }
    if group.is_some() {
        subs.push(subobject(D3D12_STATE_SUBOBJECT_TYPE_HIT_GROUP, &hit_group));
    }
    let desc = D3D12_STATE_OBJECT_DESC {
        Type: D3D12_STATE_OBJECT_TYPE_COLLECTION,
        NumSubobjects: subs.len() as u32,
        pSubobjects: subs.as_ptr(),
    };
    let native: ID3D12StateObject = unsafe { super::ray_compile::compile(&device, &desc, None)? };
    let props: ID3D12StateObjectProperties = native.cast()?;
    let export = group.as_ref().unwrap_or(&name);
    if unsafe { props.GetShaderIdentifier(PCWSTR(export.as_ptr())) }.is_null() {
        return Err(E_NOTIMPL.into());
    }
    Ok(native)
}

pub(super) unsafe extern "system" fn deserialize(
    device: &Device,
    blob: *const u8,
    size: u32,
    guest_root: *mut c_void,
    flags: u32,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let checked = unsafe { proxy::check_output::<ID3D12StateObject>(iid, out) };
    if checked.is_err() {
        return checked;
    }
    if blob.is_null() {
        return E_POINTER;
    }
    if size == 0 || size > 64 * 1024 * 1024 {
        return E_INVALIDARG;
    }
    let bytes = unsafe { slice::from_raw_parts(blob, size as usize) };
    let state = State::parse(bytes);
    let digest = shader_cache::source_digest(bytes).unwrap_or_default();
    crate::diagnostics::log_once(&Once::new(), || {
        let export = state.as_ref().map_or("<unrecognized>", |s| s.name.as_str());
        format!(
            "DeserializeStateObjectX bytes={size} root={} flags={flags:#x} export={export} SHA-256 {digest}",
            !guest_root.is_null()
        )
    });
    let root = match unsafe { root_proxy(guest_root, identity(device).as_raw()) } {
        Ok(root) => root,
        Err(error) => return error,
    };
    let Some(state) = state else { return E_NOTIMPL };
    if flags != 0 || digest.is_empty() {
        return E_NOTIMPL;
    }
    static TRACE: LazyLock<bool> =
        LazyLock::new(|| std::env::var("TUXBOX_TRACE_RAYTRACING").as_deref() == Ok("1"));
    if *TRACE && let Some(root) = root {
        crate::diagnostics::log_once(&Once::new(), || {
            let bytes = root.state.serialized();
            let hash = shader_cache::source_digest(bytes).unwrap_or_default();
            // A researcher can replay the same host collection in a separate
            // process using the title's actual global root. Normal launches
            // do not write any diagnostic files.
            if let Some(dir) = std::env::var_os("TUXBOX_CAPTURE_RAY_ROOTS")
                && !hash.is_empty()
            {
                let path = std::path::Path::new(&dir).join(format!("{hash}.dxbc"));
                if !path.exists() {
                    let _ = std::fs::write(path, bytes);
                }
            }
            format!("ray collection root {hash} bytes={} shader={digest}", bytes.len())
        });
    }
    let Some(dxil) = shader_cache::ray_library(bytes, &digest) else {
        return E_NOTIMPL;
    };
    let result = match collection(device, &state, root.map(|r| &r.native), &dxil) {
        Ok(native) => {
            let info = Inspection::from_state(&native, &state);
            unsafe { wrap(device, native, info, out) }
        }
        Err(error) => error.code(),
    };
    crate::diagnostics::log_once(&Once::new(), || {
        format!("ray collection {} host result {result:?}", state.name)
    });
    result
}
