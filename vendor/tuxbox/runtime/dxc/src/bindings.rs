//! The Series compiler accepts either one export association or a named array.
//! Desktop DXR resolves global registers when creating the state object and
//! local root descriptors from the shader record belonging to that export.
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_POINTER};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{PCSTR, PCWSTR, Result};

#[repr(C)]
pub(super) struct Association {
    name: PCWSTR,
    local: *const D3D12_VERSIONED_ROOT_SIGNATURE_DESC,
}

#[repr(C)]
pub(super) struct Single {
    name: PCWSTR,
    global: *const D3D12_VERSIONED_ROOT_SIGNATURE_DESC,
    local: *const D3D12_VERSIONED_ROOT_SIGNATURE_DESC,
    sampler_bytes: u32,
    view_bytes: u32,
    identifier_bytes: u32,
}

#[repr(C)]
pub(super) struct Multiple {
    entries: *const Association,
    count: u32,
    reserved: u32,
    global: *const D3D12_VERSIONED_ROOT_SIGNATURE_DESC,
    sampler_bytes: u32,
    view_bytes: u32,
    identifier_bytes: u32,
}

pub(super) struct Bindings {
    global: Vec<Range>,
    locals: Vec<(String, Vec<Range>)>,
}

struct Range {
    kind: D3D12_DESCRIPTOR_RANGE_TYPE,
    base: u32,
    space: u32,
    count: u32,
    root_descriptor: bool,
}

pub(super) unsafe fn records<'a, T>(base: *const T, count: u32) -> Result<&'a [T]> {
    if count == 0 {
        return Ok(&[]);
    }
    if base.is_null() {
        return Err(E_POINTER.into());
    }
    // SAFETY: The x64 caller supplies the complete array described by its count.
    Ok(unsafe { core::slice::from_raw_parts(base, count as usize) })
}

impl Bindings {
    pub(super) unsafe fn from_single(info: &Single) -> Result<Self> {
        let entry = Association {
            name: info.name,
            local: info.local,
        };
        unsafe { Self::new(info.global, &[entry], info.identifier_bytes) }
    }

    pub(super) unsafe fn from_multiple(info: &Multiple) -> Result<Self> {
        let entries = unsafe { records(info.entries, info.count)? };
        unsafe { Self::new(info.global, entries, info.identifier_bytes) }
    }

    unsafe fn new(
        root: *const D3D12_VERSIONED_ROOT_SIGNATURE_DESC,
        entries: &[Association],
        identifier_bytes: u32,
    ) -> Result<Self> {
        let global = unsafe { Self::ranges(root, false)? };
        let mut locals = Vec::new();
        for entry in entries.iter().filter(|e| !e.local.is_null()) {
            if identifier_bytes != D3D12_SHADER_IDENTIFIER_SIZE_IN_BYTES || entry.name.is_null() {
                return Err(E_NOTIMPL.into());
            }
            // SAFETY: Each mapped local signature has a guest-supplied export name.
            let name = unsafe { entry.name.to_string() }.map_err(|_| E_INVALIDARG)?;
            let ranges = unsafe { Self::ranges(entry.local, true)? };
            locals.push((name, ranges));
        }
        Ok(Self { global, locals })
    }

    unsafe fn ranges(
        root: *const D3D12_VERSIONED_ROOT_SIGNATURE_DESC,
        local: bool,
    ) -> Result<Vec<Range>> {
        let mut ranges = Vec::new();
        if root.is_null() {
            return Ok(ranges);
        }
        // The Series UMD materializes its root descriptors in the 1.0 layout.
        let root = unsafe { &*root };
        if root.Version != D3D_ROOT_SIGNATURE_VERSION_1_0 {
            return Err(E_NOTIMPL.into());
        }
        let desc = unsafe { root.Anonymous.Desc_1_0 };
        if desc
            .Flags
            .contains(D3D12_ROOT_SIGNATURE_FLAG_LOCAL_ROOT_SIGNATURE)
            != local
        {
            return Err(E_INVALIDARG.into());
        }
        for parameter in unsafe { records(desc.pParameters, desc.NumParameters)? } {
            if parameter.ShaderVisibility != D3D12_SHADER_VISIBILITY_ALL {
                continue;
            }
            // SAFETY: ParameterType selects the active union member.
            match parameter.ParameterType {
                D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE if !local => {
                    let table = unsafe { parameter.Anonymous.DescriptorTable };
                    for range in
                        unsafe { records(table.pDescriptorRanges, table.NumDescriptorRanges)? }
                    {
                        ranges.push(Range {
                            kind: range.RangeType,
                            base: range.BaseShaderRegister,
                            space: range.RegisterSpace,
                            count: range.NumDescriptors,
                            root_descriptor: false,
                        });
                    }
                }
                D3D12_ROOT_PARAMETER_TYPE_CBV
                | D3D12_ROOT_PARAMETER_TYPE_SRV
                | D3D12_ROOT_PARAMETER_TYPE_UAV => {
                    let binding = unsafe { parameter.Anonymous.Descriptor };
                    let kind = match parameter.ParameterType {
                        D3D12_ROOT_PARAMETER_TYPE_CBV => D3D12_DESCRIPTOR_RANGE_TYPE_CBV,
                        D3D12_ROOT_PARAMETER_TYPE_SRV => D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
                        _ => D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
                    };
                    ranges.push(Range {
                        kind,
                        base: binding.ShaderRegister,
                        space: binding.RegisterSpace,
                        count: 1,
                        root_descriptor: true,
                    });
                }
                _ if local => return Err(E_NOTIMPL.into()),
                _ => {}
            }
        }
        for sampler in unsafe { records(desc.pStaticSamplers, desc.NumStaticSamplers)? } {
            if sampler.ShaderVisibility == D3D12_SHADER_VISIBILITY_ALL {
                ranges.push(Range {
                    kind: D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER,
                    base: sampler.ShaderRegister,
                    space: sampler.RegisterSpace,
                    count: 1,
                    root_descriptor: false,
                });
            }
        }
        Ok(ranges)
    }

    pub(super) unsafe fn resolves(
        &self,
        function: PCSTR,
        resource: &D3D12_SHADER_INPUT_BIND_DESC,
    ) -> bool {
        let kind = match resource.Type {
            D3D_SIT_CBUFFER => D3D12_DESCRIPTOR_RANGE_TYPE_CBV,
            D3D_SIT_SAMPLER => D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER,
            D3D_SIT_TBUFFER
            | D3D_SIT_TEXTURE
            | D3D_SIT_STRUCTURED
            | D3D_SIT_BYTEADDRESS
            | D3D_SIT_RTACCELERATIONSTRUCTURE => D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            D3D_SIT_UAV_RWTYPED
            | D3D_SIT_UAV_RWSTRUCTURED
            | D3D_SIT_UAV_RWBYTEADDRESS
            | D3D_SIT_UAV_APPEND_STRUCTURED
            | D3D_SIT_UAV_CONSUME_STRUCTURED
            | D3D_SIT_UAV_RWSTRUCTURED_WITH_COUNTER => D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
            _ => return false,
        };
        let root_eligible = matches!(
            resource.Type,
            D3D_SIT_CBUFFER
                | D3D_SIT_STRUCTURED
                | D3D_SIT_BYTEADDRESS
                | D3D_SIT_RTACCELERATIONSTRUCTURE
                | D3D_SIT_UAV_RWSTRUCTURED
                | D3D_SIT_UAV_RWBYTEADDRESS
        );
        let matches = |range: &Range| {
            kind == range.kind
                && resource.Space == range.space
                && resource.BindPoint >= range.base
                && (!range.root_descriptor || root_eligible)
                && (range.count == u32::MAX
                    || (resource.BindCount != 0
                        && u64::from(resource.BindPoint) + u64::from(resource.BindCount)
                            <= u64::from(range.base) + u64::from(range.count)))
        };
        if self.global.iter().any(matches) {
            return true;
        }
        if function.is_null() {
            return false;
        }
        // Reflection can expose an LLVM-decorated HLSL name or its public alias.
        let Ok(name) = (unsafe { function.to_string() }) else {
            return false;
        };
        let decorated = name.trim_start_matches('\u{1}');
        let plain = decorated
            .strip_prefix('?')
            .and_then(|n| n.split_once("@@"))
            .map_or(decorated, |(n, _)| n);
        self.locals.iter().any(|(export, ranges)| {
            (export == &name || export == decorated || export == plain)
                && ranges.iter().any(matches)
        })
    }
}
