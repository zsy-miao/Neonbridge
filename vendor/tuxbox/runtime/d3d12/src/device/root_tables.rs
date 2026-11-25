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
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_ROOT_VIEW_TABLES") != Some("0"))
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

/// Bind half (VIctory-fxceless draw.rs root_srv_table / pipeline/host.rs
/// bounded_root_srvs, GPL-3.0, ported): a tabled parameter is bound to a
/// raw view of the root view's host import, from the bound address to the
/// import's end, written into a private slot past the title's count in the
/// bound shader-visible CBV/SRV/UAV heap ([`TAIL`] slots, a ring). A slot is
/// reused for the same (buffer address, size, kind); the ring wrapping (all
/// views rewritten while older lists may still read them) is logged once.
pub(crate) const TAIL: u32 = 4096;

struct Tail {
    first: u32,
    next: u32,
    stride: u64,
    cpu: usize,
    gpu: u64,
    cached: HashMap<(u64, u64, bool), u32>,
}

static TAILS: Mutex<Option<HashMap<usize, Tail>>> = Mutex::new(None);

/// The tail this native heap's creation adds (0: none).
pub(crate) fn tail_for(desc: &D3D12_DESCRIPTOR_HEAP_DESC) -> u32 {
    (enabled() && desc.Type == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV
        && desc.Flags == D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
        && desc.NumDescriptors.checked_add(TAIL).is_some_and(|n| n <= 1_000_000)) as u32 * TAIL
}

/// Records (or forgets, `tail` 0) the private slots of a new native heap.
pub(crate) fn heap_created(heap: &ID3D12DescriptorHeap, first: u32, tail: u32) {
    if !enabled() {
        return;
    }
    let mut tails = TAILS.lock().unwrap_or_else(|e| e.into_inner());
    let tails = tails.get_or_insert_with(HashMap::new);
    let key = heap.as_raw() as usize;
    if tail == 0 {
        tails.remove(&key);
        return;
    }
    let mut device: Option<ID3D12Device> = None;
    if unsafe { heap.GetDevice(&mut device) }.is_err() { return; }
    let Some(device) = device else { return };
    let stride = u64::from(unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) });
    tails.insert(key, Tail {
        first, next: 0, stride,
        cpu: unsafe { heap.GetCPUDescriptorHandleForHeapStart() }.ptr,
        gpu: unsafe { heap.GetGPUDescriptorHandleForHeapStart() }.ptr,
        cached: HashMap::new(),
    });
}

/// The private slots past the title's in this native heap.
pub(crate) fn tail_len(heap: &ID3D12DescriptorHeap) -> u32 {
    if !enabled() {
        return 0;
    }
    let tails = TAILS.lock().unwrap_or_else(|e| e.into_inner());
    tails.as_ref().and_then(|t| t.get(&(heap.as_raw() as usize))).map_or(0, |_| TAIL)
}

/// `count` contiguous private slots of `heap`'s ring (SRV-as-UAV tables,
/// srv_views::fill_table): (CPU start, GPU start). None: no ring, or more
/// than it holds. Wrapping rewrites slots older lists may still read, as for
/// the root view tables (logged once there).
pub(crate) fn private_slots(heap: &ID3D12DescriptorHeap, count: u32) -> Option<(usize, u64, usize)> {
    let mut tails = TAILS.lock().unwrap_or_else(|e| e.into_inner());
    let tail = tails.as_mut()?.get_mut(&(heap.as_raw() as usize))?;
    if count == 0 || count > TAIL / 4 {
        return None;
    }
    if tail.next + count > TAIL {
        tail.next = 0;
        tail.cached.clear();
    }
    let index = tail.next;
    tail.next = (tail.next + count) % TAIL;
    tail.cached.retain(|_, &mut i| !(index..index + count).contains(&i));
    let at = u64::from(tail.first + index) * tail.stride;
    Some((tail.cpu + at as usize, tail.gpu + at, tail.stride as usize))
}

/// A one-view table in `heap` for a root view at host `address` inside
/// `buffer` (None: a null view): a raw SRV/UAV to the buffer's end. None
/// when the heap has no private slots.
pub(crate) fn view_slot(
    heap: &ID3D12DescriptorHeap, buffer: Option<&ID3D12Resource>, address: u64, uav: bool,
) -> Option<D3D12_GPU_DESCRIPTOR_HANDLE> {
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_TYPELESS;
    let mut tails = TAILS.lock().unwrap_or_else(|e| e.into_inner());
    let tail = tails.as_mut()?.get_mut(&(heap.as_raw() as usize))?;
    // The view: (resource, first byte, bytes), or None for a null view.
    let view = buffer.and_then(|buffer| unsafe {
        let base = buffer.GetGPUVirtualAddress();
        let desc = buffer.GetDesc();
        let offset = address.checked_sub(base).filter(|&o| o < desc.Width)?;
        if !offset.is_multiple_of(16) {
            static ODD: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&ODD, || format!("root view table: address {address:#x} not on 16 bytes is bound as a null view"));
            return None;
        }
        if uav && (desc.Flags & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS).0 == 0 {
            static RO: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&RO, || "root view table: a root UAV over an import without unordered access is bound as a null view".into());
            return None;
        }
        Some((buffer, offset, (desc.Width - offset) & !3))
    }).filter(|v| v.2 != 0);
    let key = view.map_or((0, 0, uav), |(_, _, bytes)| (address, bytes, uav));
    let index = match tail.cached.get(&key) {
        Some(&index) => index,
        None => {
            let index = tail.next;
            tail.next = (tail.next + 1) % TAIL;
            if tail.next == 0 {
                static WRAP: std::sync::Once = std::sync::Once::new();
                crate::diagnostics::log_once(&WRAP, || format!("root view table: the {TAIL}-slot ring of a heap wrapped; older views are rewritten"));
                tail.cached.clear();
            }
            tail.cached.retain(|_, &mut i| i != index);
            tail.cached.insert(key, index);
            let cpu = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: tail.cpu + ((u64::from(tail.first + index)) * tail.stride) as usize };
            let elements = view.map_or(0, |v| u32::try_from(v.2 / 4).unwrap_or(u32::MAX));
            let first = view.map_or(0, |v| v.1 / 4);
            let mut device: Option<ID3D12Device> = None;
            unsafe { heap.GetDevice(&mut device) }.ok()?;
            let device = device?;
            let resource = view.map(|v| v.0);
            unsafe {
                if uav {
                    let desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
                        Format: DXGI_FORMAT_R32_TYPELESS,
                        ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
                        Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Buffer: D3D12_BUFFER_UAV {
                            FirstElement: first, NumElements: elements, StructureByteStride: 0,
                            CounterOffsetInBytes: 0, Flags: D3D12_BUFFER_UAV_FLAG_RAW,
                        } },
                    };
                    device.CreateUnorderedAccessView(resource, None, Some(&desc), cpu);
                } else {
                    let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                        Format: DXGI_FORMAT_R32_TYPELESS,
                        ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
                        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Buffer: D3D12_BUFFER_SRV {
                            FirstElement: first, NumElements: elements, StructureByteStride: 0,
                            Flags: D3D12_BUFFER_SRV_FLAG_RAW,
                        } },
                    };
                    device.CreateShaderResourceView(resource, Some(&desc), cpu);
                }
            }
            index
        }
    };
    Some(D3D12_GPU_DESCRIPTOR_HANDLE { ptr: tail.gpu + u64::from(tail.first + index) * tail.stride })
}
