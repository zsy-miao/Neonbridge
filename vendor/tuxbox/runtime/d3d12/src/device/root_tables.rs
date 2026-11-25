//! Root SRVs and UAVs as one-view tables (VIctory-fxceless pipeline/host.rs
//! bounded_root_srvs, gap 5). A root descriptor has no size, so a mesh
//! shader's vertex fetch with no record limit reads past the buffer into
//! memory the host has freed and the device is removed (upstream PR #493).
//! A one-view table of a sized raw view bounds the access: reads past its
//! end return zero and writes are dropped.
//!
//! This is the root signature half. Each title root SRV/UAV parameter is
//! replaced in place by a one-range table at the same register and space,
//! so parameter indices and the shader's bindings are unchanged (a root
//! view is only ever a raw or structured buffer, which a table SRV/UAV
//! range covers). [`tabled`] names the replaced parameters for the bind
//! half: a SetGraphicsRoot/SetComputeRoot SRV or UAV call on one of them
//! must write a raw view sized to the buffer's import into a shader-visible
//! slot and bind that slot as the table.
//!
//! Opt in with TUXBOX_ROOT_VIEW_TABLES=1 until the bind half lands; with it
//! unset, signatures are passed through as before.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_ROOT_VIEW_TABLES") == Some("1"))
}

/// Our own root views (spaces at or above this) stay root descriptors.
const PRIVATE_SPACE: u32 = 0x7ff0;

static TABLED: Mutex<Option<HashMap<usize, Arc<[u32]>>>> = Mutex::new(None);

/// The title signature rewritten, and the indices of the parameters that
/// became tables, or None when it has no title root SRV/UAV.
pub(super) fn rewrite(bytes: &[u8]) -> Option<(Vec<u8>, Vec<u32>)> {
    let mut reader = None::<ID3D12VersionedRootSignatureDeserializer>;
    unsafe {
        D3D12CreateVersionedRootSignatureDeserializer(
            bytes.as_ptr().cast(),
            bytes.len(),
            &ID3D12VersionedRootSignatureDeserializer::IID,
            (&raw mut reader).cast(),
        )
        .ok()?;
        let reader = reader?;
        let original = reader.GetUnconvertedRootSignatureDesc().as_ref()?;
        let mut desc = match original.Version {
            D3D_ROOT_SIGNATURE_VERSION_1_0 => *reader.GetRootSignatureDescAtVersion(D3D_ROOT_SIGNATURE_VERSION_1_1).ok()?,
            D3D_ROOT_SIGNATURE_VERSION_1_1 | D3D_ROOT_SIGNATURE_VERSION_1_2 => *original,
            _ => return None,
        };
        let (count, ptr) = if desc.Version == D3D_ROOT_SIGNATURE_VERSION_1_2 {
            (desc.Anonymous.Desc_1_2.NumParameters, desc.Anonymous.Desc_1_2.pParameters)
        } else {
            (desc.Anonymous.Desc_1_1.NumParameters, desc.Anonymous.Desc_1_1.pParameters)
        };
        if ptr.is_null() || count == 0 {
            return None;
        }
        let mut params = core::slice::from_raw_parts(ptr, count as usize).to_vec();
        let mut ranges: Vec<Box<D3D12_DESCRIPTOR_RANGE1>> = Vec::new();
        let mut tabled = Vec::new();
        for (index, p) in params.iter_mut().enumerate() {
            let range = match p.ParameterType {
                D3D12_ROOT_PARAMETER_TYPE_SRV => D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
                D3D12_ROOT_PARAMETER_TYPE_UAV => D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
                _ => continue,
            };
            let d = p.Anonymous.Descriptor;
            if d.RegisterSpace >= PRIVATE_SPACE {
                continue;
            }
            ranges.push(Box::new(D3D12_DESCRIPTOR_RANGE1 {
                RangeType: range,
                NumDescriptors: 1,
                BaseShaderRegister: d.ShaderRegister,
                RegisterSpace: d.RegisterSpace,
                Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE | D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE,
                OffsetInDescriptorsFromTableStart: 0,
            }));
            let range: *const D3D12_DESCRIPTOR_RANGE1 = &**ranges.last()?;
            p.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
            p.Anonymous = D3D12_ROOT_PARAMETER1_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE1 { NumDescriptorRanges: 1, pDescriptorRanges: range },
            };
            tabled.push(index as u32);
        }
        if tabled.is_empty() {
            return None;
        }
        if desc.Version == D3D_ROOT_SIGNATURE_VERSION_1_2 {
            desc.Anonymous.Desc_1_2.pParameters = params.as_ptr();
        } else {
            desc.Anonymous.Desc_1_1.pParameters = params.as_ptr();
        }
        let mut serialized = None;
        D3D12SerializeVersionedRootSignature(&desc, &mut serialized, None).ok()?;
        let blob = serialized?;
        let out = core::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()).to_vec();
        Some((out, tabled))
    }
}

pub(super) fn remember(native: &ID3D12RootSignature, tabled: Vec<u32>) {
    TABLED.lock().unwrap().get_or_insert_with(HashMap::new).insert(native.as_raw() as usize, tabled.into());
}

/// The parameters of this signature that are one-view tables in place of
/// title root SRVs/UAVs (None: none are).
#[allow(dead_code)]
pub(crate) fn tabled(native: &ID3D12RootSignature) -> Option<Arc<[u32]>> {
    TABLED.lock().unwrap().as_ref()?.get(&(native.as_raw() as usize)).cloned()
}
