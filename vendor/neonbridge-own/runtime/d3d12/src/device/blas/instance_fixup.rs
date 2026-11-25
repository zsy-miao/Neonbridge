//! Patch a GPU-produced array on its recording list, after all preceding GPU
//! writes. A private volatile descriptor lets queue submission publish the
//! current relocation table even when an address first moved after Close.
use super::{guest, lock, rebuild};
use core::{ffi::c_void, mem::ManuallyDrop, slice};
use std::sync::Mutex;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, FreeLibrary};
use windows::Win32::Graphics::{
    Direct3D::ID3DBlob, Direct3D12::*, Dxgi::Common::DXGI_FORMAT_R32_TYPELESS,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::core::{GUID, HRESULT, IUnknown, Interface, Result, s, w};

// The allocator retains the private output through completed GPU work. Tie the
// shader-visible heap and its current upload to that same native lifetime.
const RECORD_HEAP: GUID = GUID::from_u128(0x74784258_6631_4d38_bfd8_98876fd36a17);
const TABLE_BUFFER: GUID = GUID::from_u128(0xeb6e1828_7fec_47d6_9d71_43cd67ba0d04);

#[derive(Clone)]
pub(super) struct Kernel {
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
}

fn shader() -> Result<Vec<u8>> {
    type Compile = unsafe extern "system" fn(
        *const c_void,
        usize,
        *const u8,
        *const c_void,
        *const c_void,
        *const u8,
        *const u8,
        u32,
        u32,
        *mut *mut c_void,
        *mut *mut c_void,
    ) -> HRESULT;
    let library =
        unsafe { LoadLibraryExW(w!("d3dcompiler_47.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)? };
    let result = (|| unsafe {
        let entry = GetProcAddress(library, s!("D3DCompile")).ok_or(E_NOTIMPL)?;
        let compile: Compile = core::mem::transmute(entry);
        let text = include_bytes!("instance_fixup.hlsl");
        let mut raw = core::ptr::null_mut();
        compile(
            text.as_ptr().cast(),
            text.len(),
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null(),
            s!("main").as_ptr(),
            s!("cs_5_0").as_ptr(),
            0,
            0,
            &mut raw,
            core::ptr::null_mut(),
        )
        .ok()?;
        let blob = ID3DBlob::from_raw(raw);
        Ok(
            slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
                .to_vec(),
        )
    })();
    let _ = unsafe { FreeLibrary(library) };
    result
}

impl Kernel {
    fn new(device: &ID3D12Device) -> Result<Self> {
        let range = D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 1,
            BaseShaderRegister: 1,
            ..Default::default()
        };
        let desc = |kind| D3D12_ROOT_PARAMETER {
            ParameterType: kind,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR::default(),
            },
            ..Default::default()
        };
        let args = [
            desc(D3D12_ROOT_PARAMETER_TYPE_SRV),
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                        NumDescriptorRanges: 1,
                        pDescriptorRanges: &range,
                    },
                },
                ..Default::default()
            },
            desc(D3D12_ROOT_PARAMETER_TYPE_UAV),
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Constants: D3D12_ROOT_CONSTANTS {
                        Num32BitValues: 2,
                        ..Default::default()
                    },
                },
                ..Default::default()
            },
        ];
        let rs = D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: args.len() as u32,
            pParameters: args.as_ptr(),
            ..Default::default()
        };
        let mut blob = None;
        unsafe {
            D3D12SerializeRootSignature(&rs, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, None)?;
        }
        let blob = blob.ok_or(E_NOTIMPL)?;
        let root: ID3D12RootSignature = unsafe {
            device.CreateRootSignature(
                0,
                slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()),
            )?
        };
        let shader = shader()?;
        let mut desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: ManuallyDrop::new(Some(root.clone())),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: shader.as_ptr().cast(),
                BytecodeLength: shader.len(),
            },
            ..Default::default()
        };
        let pipeline = unsafe { device.CreateComputePipelineState(&desc) };
        unsafe {
            ManuallyDrop::drop(&mut desc.pRootSignature);
        }
        Ok(Self {
            root,
            pipeline: pipeline?,
        })
    }
}

pub(crate) struct Instances {
    output: ID3D12Resource,
    heap: ID3D12DescriptorHeap,
    latest: Mutex<Option<ID3D12Resource>>,
}

impl Instances {
    /// The owner identifies the guest device that created the input resource.
    pub(crate) unsafe fn record(
        owner: &IUnknown,
        host: &ID3D12GraphicsCommandList4,
        address: u64,
        count: u32,
    ) -> Result<Self> {
        let device = unsafe { guest(owner) };
        let kernel = {
            let mut value = lock(&device.state.blas.fixup);
            if value.is_none() {
                *value = Some(Kernel::new(&device.native)?);
            }
            value.as_ref().unwrap().clone()
        };
        let output = rebuild::buffer(
            &device.native,
            u64::from(count) * 64,
            D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        )?;
        let heap: ID3D12DescriptorHeap = unsafe {
            device
                .native
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: 1,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })?
        };
        unsafe {
            output.SetPrivateDataInterface(&RECORD_HEAP, &heap)?;
        }
        let instances = Self {
            output,
            heap,
            latest: Mutex::new(None),
        };
        // Initialize the table before setting it, even on drivers which inspect
        // the descriptor as soon as the root table is recorded.
        unsafe {
            instances.prepare(owner)?;
        }
        let groups = count.div_ceil(64);
        let x = groups.min(D3D12_CS_DISPATCH_MAX_THREAD_GROUPS_PER_DIMENSION);
        let bounds = [count, x];
        unsafe {
            // A replay starts and ends with the patched array available to both
            // the DXR builder and later shaders that need its instance records.
            instances.barrier(host, false);
            host.SetDescriptorHeaps(&[Some(instances.heap.clone())]);
            host.SetComputeRootSignature(&kernel.root);
            host.SetPipelineState(&kernel.pipeline);
            host.SetComputeRootShaderResourceView(0, address);
            host.SetComputeRootDescriptorTable(
                1,
                instances.heap.GetGPUDescriptorHandleForHeapStart(),
            );
            host.SetComputeRootUnorderedAccessView(2, instances.output.GetGPUVirtualAddress());
            host.SetComputeRoot32BitConstants(3, 2, bounds.as_ptr().cast(), 0);
            host.Dispatch(x, groups.div_ceil(x), 1);
            instances.barrier(host, true);
        }
        Ok(instances)
    }

    pub(crate) fn output(&self) -> &ID3D12Resource {
        &self.output
    }

    unsafe fn barrier(&self, host: &ID3D12GraphicsCommandList4, read: bool) {
        let (write, srv) = (
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
        );
        let mut barrier = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    pResource: ManuallyDrop::new(Some(self.output.clone())),
                    Subresource: 0,
                    StateBefore: if read { write } else { srv },
                    StateAfter: if read { srv } else { write },
                }),
            },
        };
        unsafe {
            host.ResourceBarrier(slice::from_ref(&barrier));
            ManuallyDrop::drop(&mut (*barrier.Anonymous.Transition).pResource);
        }
    }

    // Each recorded fixup owns its heap, so other lists can execute with older
    // tables concurrently. D3D12 requires a direct list's previous execution to
    // complete before that same list is submitted again; only then may its heap
    // be updated for replay. The output retained by the allocator also retains
    // the heap and table if the command list is reset while execution is pending.
    pub(crate) unsafe fn prepare(&self, owner: &IUnknown) -> Result<()> {
        let mut latest = lock(&self.latest);
        let device = unsafe { guest(owner) };
        let table = {
            let mut maps = lock(&device.state.blas.mappings);
            if maps.table.is_none() {
                maps.table = Some(Table::new(&device.native, &maps.snapshot())?);
            }
            maps.table.as_ref().unwrap().clone()
        };
        if latest.as_ref() == Some(&table.resource) {
            return Ok(());
        }
        unsafe {
            self.heap
                .SetPrivateDataInterface(&TABLE_BUFFER, &table.resource)?;
            device.native.CreateShaderResourceView(
                &table.resource,
                Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: DXGI_FORMAT_R32_TYPELESS,
                    ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Buffer: D3D12_BUFFER_SRV {
                            NumElements: table.words,
                            Flags: D3D12_BUFFER_SRV_FLAG_RAW,
                            ..Default::default()
                        },
                    },
                }),
                self.heap.GetCPUDescriptorHandleForHeapStart(),
            );
        }
        *latest = Some(table.resource);
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct Table {
    resource: ID3D12Resource,
    words: u32,
}

impl Table {
    fn new(device: &ID3D12Device, mappings: &[(u64, u64)]) -> Result<Self> {
        let size = mappings
            .len()
            .checked_add(1)
            .and_then(|n| n.checked_mul(16))
            .and_then(|v| u32::try_from(v).ok())
            .ok_or(E_INVALIDARG)?;
        let table = rebuild::upload_buffer(device, u64::from(size))?;
        let mut data = core::ptr::null_mut();
        unsafe {
            table.Map(0, Some(&D3D12_RANGE::default()), Some(&mut data))?;
            let words = data.cast::<u64>();
            words.write(mappings.len() as u64);
            words.add(1).write(0);
            for (i, &(source, target)) in mappings.iter().enumerate() {
                words.add(2 + i * 2).write(source);
                words.add(3 + i * 2).write(target);
            }
            table.Unmap(0, None);
        }
        Ok(Self {
            resource: table,
            words: size / 4,
        })
    }
}
