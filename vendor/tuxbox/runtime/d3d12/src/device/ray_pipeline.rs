//! Link Xbox ray collections through the host device. Associations refer to
//! elements of the caller's array and must point into the translated array.
use super::{Device, Out, compute::root_signature, identity, shader_cache, state_object};
use core::{ffi::c_void, ptr, slice};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{GUID, HRESULT, Interface, Result};

#[repr(C)]
struct Collection {
    object: *mut c_void,
    count: u32,
    exports: *const D3D12_EXPORT_DESC,
}
const _: () = assert!(size_of::<Collection>() == size_of::<D3D12_EXISTING_COLLECTION_DESC>());

enum Payload {
    Root(*mut c_void),
    Collection(Collection),
    Association(D3D12_SUBOBJECT_TO_EXPORTS_ASSOCIATION),
    Guest(*const c_void),
}
impl Payload {
    fn address(&self) -> *const c_void {
        match self {
            Self::Root(v) => ptr::from_ref(v).cast(),
            Self::Collection(v) => ptr::from_ref(v).cast(),
            Self::Association(v) => ptr::from_ref(v).cast(),
            Self::Guest(v) => *v,
        }
    }
}

unsafe fn link(
    device: &Device,
    desc: *const D3D12_STATE_OBJECT_DESC,
    previous: Option<&ID3D12StateObject>,
) -> Result<ID3D12StateObject> {
    let desc = unsafe { desc.as_ref() }.ok_or(E_POINTER)?;
    let n = desc.NumSubobjects as usize;
    if !matches!(
        desc.Type,
        D3D12_STATE_OBJECT_TYPE_COLLECTION | D3D12_STATE_OBJECT_TYPE_RAYTRACING_PIPELINE
    ) || (n != 0 && desc.pSubobjects.is_null())
    {
        return Err(E_INVALIDARG.into());
    }
    let inputs = if n == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(desc.pSubobjects, n) }
    };
    let mut host = Vec::new();
    host.try_reserve_exact(n).map_err(|_| E_OUTOFMEMORY)?;
    host.extend_from_slice(inputs);
    // Associations can target later subobjects. The array has its final size,
    // and all pointers into it share the allocation used for the host call.
    let base = host.as_mut_ptr();
    let mut payloads = Vec::new();
    payloads.try_reserve_exact(n).map_err(|_| E_OUTOFMEMORY)?;
    let owner = identity(device).as_raw();
    for sub in inputs {
        let data = sub.pDesc;
        if data.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let body = match sub.Type {
            D3D12_STATE_SUBOBJECT_TYPE_GLOBAL_ROOT_SIGNATURE
            | D3D12_STATE_SUBOBJECT_TYPE_LOCAL_ROOT_SIGNATURE => {
                let guest = unsafe { *data.cast::<*mut c_void>() };
                let native = unsafe { root_signature(guest, owner) }?;
                Payload::Root(native.map_or(ptr::null_mut(), Interface::as_raw))
            }
            D3D12_STATE_SUBOBJECT_TYPE_EXISTING_COLLECTION => {
                let guest = unsafe { &*data.cast::<Collection>() };
                let object =
                    unsafe { state_object::native(guest.object, owner) }.ok_or(E_INVALIDARG)?;
                if guest.count != 0 && guest.exports.is_null() {
                    return Err(E_INVALIDARG.into());
                }
                Payload::Collection(Collection {
                    object: object.as_raw(),
                    count: guest.count,
                    exports: guest.exports,
                })
            }
            D3D12_STATE_SUBOBJECT_TYPE_SUBOBJECT_TO_EXPORTS_ASSOCIATION => {
                let mut assoc = unsafe { *data.cast::<D3D12_SUBOBJECT_TO_EXPORTS_ASSOCIATION>() };
                let offset = (assoc.pSubobjectToAssociate as usize)
                    .checked_sub(inputs.as_ptr() as usize)
                    .ok_or(E_INVALIDARG)?;
                let stride = size_of::<D3D12_STATE_SUBOBJECT>();
                if !offset.is_multiple_of(stride)
                    || offset / stride >= n
                    || (assoc.NumExports != 0 && assoc.pExports.is_null())
                {
                    return Err(E_INVALIDARG.into());
                }
                assoc.pSubobjectToAssociate = base.wrapping_add(offset / stride);
                Payload::Association(assoc)
            }
            D3D12_STATE_SUBOBJECT_TYPE_DXIL_LIBRARY => {
                let library = unsafe { &*data.cast::<D3D12_DXIL_LIBRARY_DESC>() };
                let code = library.DXILLibrary;
                if code.pShaderBytecode.is_null()
                    || code.BytecodeLength == 0
                    || (library.NumExports != 0 && library.pExports.is_null())
                {
                    return Err(E_INVALIDARG.into());
                }
                if !shader_cache::portable_library(unsafe {
                    slice::from_raw_parts(code.pShaderBytecode.cast(), code.BytecodeLength)
                }) {
                    return Err(E_NOTIMPL.into());
                }
                Payload::Guest(data)
            }
            D3D12_STATE_SUBOBJECT_TYPE_STATE_OBJECT_CONFIG
            | D3D12_STATE_SUBOBJECT_TYPE_NODE_MASK
            | D3D12_STATE_SUBOBJECT_TYPE_DXIL_SUBOBJECT_TO_EXPORTS_ASSOCIATION
            | D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_SHADER_CONFIG
            | D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_PIPELINE_CONFIG
            | D3D12_STATE_SUBOBJECT_TYPE_HIT_GROUP
            | D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_PIPELINE_CONFIG1 => Payload::Guest(data),
            _ => return Err(E_NOTIMPL.into()),
        };
        payloads.push(body);
    }
    for (i, payload) in payloads.iter().enumerate() {
        unsafe { (*base.add(i)).pDesc = payload.address() };
    }
    let translated = D3D12_STATE_OBJECT_DESC {
        pSubobjects: host.as_ptr(),
        ..*desc
    };
    crate::diagnostics::log_once(&std::sync::Once::new(), || {
        let mut collections = 0;
        let mut root_count = 0;
        let mut payload_bytes = None;
        let mut attribute_bytes = None;
        let mut recursion = None;
        for sub in inputs {
            match sub.Type {
                D3D12_STATE_SUBOBJECT_TYPE_EXISTING_COLLECTION => collections += 1,
                D3D12_STATE_SUBOBJECT_TYPE_GLOBAL_ROOT_SIGNATURE => root_count += 1,
                D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_SHADER_CONFIG => {
                    let config = unsafe { &*sub.pDesc.cast::<D3D12_RAYTRACING_SHADER_CONFIG>() };
                    payload_bytes = Some(config.MaxPayloadSizeInBytes);
                    attribute_bytes = Some(config.MaxAttributeSizeInBytes);
                }
                D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_PIPELINE_CONFIG => {
                    recursion = Some(unsafe {
                        (*sub.pDesc.cast::<D3D12_RAYTRACING_PIPELINE_CONFIG>())
                            .MaxTraceRecursionDepth
                    });
                }
                D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_PIPELINE_CONFIG1 => {
                    recursion = Some(unsafe {
                        (*sub.pDesc.cast::<D3D12_RAYTRACING_PIPELINE_CONFIG1>())
                            .MaxTraceRecursionDepth
                    });
                }
                _ => {}
            }
        }
        format!(
            "DXR state link type={} add={} collections={collections} global_roots={root_count} payload_bytes={payload_bytes:?} attribute_bytes={attribute_bytes:?} recursion={recursion:?}",
            desc.Type.0,
            previous.is_some()
        )
    });
    let result =
        unsafe { super::ray_compile::compile(&device.native.cast()?, &translated, previous) };
    if let Err(e) = &result {
        crate::diagnostics::recording_error(format_args!(
            "DXR pipeline link failed ({:#x}); subobject types {:?}",
            e.code().0,
            inputs.iter().map(|s| s.Type.0).collect::<Vec<_>>()
        ));
    }
    result
}

pub(super) unsafe extern "system" fn create(
    device: &Device,
    desc: *const D3D12_STATE_OBJECT_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let check = unsafe { crate::proxy::check_output::<ID3D12StateObject>(iid, out) };
    if check.is_err() {
        return check;
    }
    match unsafe { link(device, desc, None) } {
        Ok(native) => unsafe {
            state_object::wrap_pipeline(device, native, desc, ptr::null_mut(), out)
        },
        Err(err) => err.code(),
    }
}

pub(super) unsafe extern "system" fn add(
    device: &Device,
    desc: *const D3D12_STATE_OBJECT_DESC,
    previous: *mut c_void,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let check = unsafe { crate::proxy::check_output::<ID3D12StateObject>(iid, out) };
    if check.is_err() {
        return check;
    }
    let previous_guest = previous;
    let Some(previous) =
        (unsafe { state_object::native(previous_guest, identity(device).as_raw()) })
    else {
        return E_INVALIDARG;
    };
    match unsafe { link(device, desc, Some(previous)) } {
        Ok(native) => unsafe {
            state_object::wrap_pipeline(device, native, desc, previous_guest, out)
        },
        Err(err) => err.code(),
    }
}
