//! Warm Frosty's profiled compute shaders through the title's app-local provider.

use std::ffi::{OsStr, c_void};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::mem::ManuallyDrop;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::time::Instant;
use windows::Win32::Foundation::{E_POINTER, HMODULE};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW, SetDllDirectoryW};
use windows::core::{GUID, HRESULT, IUnknown, Interface, Owned, PCWSTR, s};

type CreateDevice =
    unsafe extern "system" fn(*mut c_void, *const u32, *const GUID, *mut *mut c_void) -> HRESULT;

// Frosty's 72-byte device creation record, including its AutoHDR field.
const CREATION: [u32; 18] = [
    0x20004,
    0,
    u32::MAX,
    0,
    0,
    0,
    u32::MAX,
    u32::MAX,
    0,
    1,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
];

fn checked<T>(operation: &str, result: windows::core::Result<T>) -> Result<T, String> {
    result.map_err(|error| format!("{operation} failed: 0x{:08x}", error.code().0 as u32))
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn create_guest_device(module: HMODULE) -> Result<ID3D12Device, String> {
    // SAFETY: the module stays loaded until the device and all its children are
    // released. This named export takes the Scarlett factory ABI and writes one
    // owned COM reference to the cleared output pointer on success.
    unsafe {
        let address = GetProcAddress(module, s!("D3D12XboxCreateDevice"))
            .ok_or("missing Xbox device factory")?;
        let factory: CreateDevice = std::mem::transmute(address);
        let mut device: Option<ID3D12Device> = None;
        let status = factory(
            ptr::null_mut(),
            CREATION.as_ptr(),
            &ID3D12Device::IID,
            (&raw mut device).cast(),
        );
        checked("CreateDevice", status.ok())?;
        checked("CreateDevice", device.ok_or_else(|| E_POINTER.into()))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Layout {
    ReferenceCompute,
    Compute,
    Ray,
}

fn profiled_root(guest: &ID3D12Device, layout: Layout) -> Result<ID3D12RootSignature, String> {
    // Frosty uses one descriptor table per range. On the TuxBox device the
    // game requests root signature 1.0. The historical comparison used 1.1.
    let descriptors = D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE;
    let static_data = descriptors | D3D12_DESCRIPTOR_RANGE_FLAG_DATA_STATIC_WHILE_SET_AT_EXECUTE;
    let volatile_data = descriptors | D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE;
    let ranges = [
        (D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 64, static_data),
        (D3D12_DESCRIPTOR_RANGE_TYPE_CBV, 16, static_data),
        (D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER, 16, descriptors),
        (D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 16, volatile_data),
    ]
    .map(|(kind, count, flags)| D3D12_DESCRIPTOR_RANGE1 {
        RangeType: kind,
        NumDescriptors: count,
        Flags: flags,
        RegisterSpace: u32::from(layout == Layout::Ray),
        OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
        ..Default::default()
    });
    let parameters = ranges.each_ref().map(|range| D3D12_ROOT_PARAMETER1 {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        Anonymous: D3D12_ROOT_PARAMETER1_0 {
            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE1 {
                NumDescriptorRanges: 1,
                pDescriptorRanges: range,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    });
    let descriptor = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
        Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
        Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 {
            Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
                NumParameters: parameters.len() as u32,
                pParameters: parameters.as_ptr(),
                // Xbox's four-table global DXR signature includes this flag.
                Flags: D3D12_ROOT_SIGNATURE_FLAGS(if layout == Layout::Ray {
                    0x0100_0000
                } else {
                    0
                }),
                ..Default::default()
            },
        },
    };
    // SAFETY: both arrays outlive serialization. The blob owns the returned
    // slice for the duration of the guest's shared CreateRootSignature slot.
    unsafe {
        let mut bytes = None;
        checked(
            "SerializeVersionedRootSignature",
            D3D12SerializeVersionedRootSignature(&descriptor, &mut bytes, None),
        )?;
        let mut bytes = checked(
            "SerializeVersionedRootSignature",
            bytes.ok_or_else(|| E_POINTER.into()),
        )?;
        if layout != Layout::ReferenceCompute {
            let mut reader: Option<ID3D12VersionedRootSignatureDeserializer> = None;
            checked(
                "ReadRootSignature",
                D3D12CreateVersionedRootSignatureDeserializer(
                    bytes.GetBufferPointer(),
                    bytes.GetBufferSize(),
                    &ID3D12VersionedRootSignatureDeserializer::IID,
                    (&raw mut reader).cast(),
                ),
            )?;
            let reader = checked("ReadRootSignature", reader.ok_or_else(|| E_POINTER.into()))?;
            let root = checked(
                "RootSignature1.0",
                reader.GetRootSignatureDescAtVersion(D3D_ROOT_SIGNATURE_VERSION_1_0),
            )?;
            let mut converted = None;
            checked(
                "SerializeRootSignature1.0",
                D3D12SerializeVersionedRootSignature(root, &mut converted, None),
            )?;
            bytes = checked(
                "SerializeRootSignature1.0",
                converted.ok_or_else(|| E_POINTER.into()),
            )?;
        }
        let data =
            std::slice::from_raw_parts(bytes.GetBufferPointer().cast(), bytes.GetBufferSize());
        checked("CreateRootSignature", guest.CreateRootSignature(1, data))
    }
}

fn guest_pipeline(
    guest: &ID3D12Device,
    desc: &D3D12_COMPUTE_PIPELINE_STATE_DESC,
) -> windows::core::Result<ID3D12PipelineState> {
    type Create = unsafe extern "system" fn(
        *mut c_void,
        *const D3D12_COMPUTE_PIPELINE_STATE_DESC,
        u32,
        *const c_void,
        *const GUID,
        *mut *mut c_void,
    ) -> HRESULT;
    let mut state = None;
    // SAFETY: the verified TuxBox Xbox device exposes CreateComputePipelineStateX
    // at slot 62. The descriptor and its root live through the synchronous call.
    unsafe {
        let slots = *guest.as_raw().cast::<*const *const c_void>();
        let create: Create = std::mem::transmute(*slots.add(62));
        create(
            guest.as_raw(),
            desc,
            0,
            ptr::null(),
            &ID3D12PipelineState::IID,
            (&raw mut state).cast(),
        )
        .ok()?;
    }
    state.ok_or_else(|| E_POINTER.into())
}

fn ray_collections(
    guest: &ID3D12Device,
    state_dir: &Path,
    report_path: &Path,
) -> Result<(), String> {
    let root = profiled_root(guest, Layout::Ray)?;
    let iid = GUID::from_u128(0x728f1fa2_ea11_4a9f_9b52_9b42a484d5c9);
    let mut device8: Option<IUnknown> = None;
    // SAFETY: query writes an owned guest device reference on success.
    checked("Xbox Device8", unsafe {
        guest.query(&iid, (&raw mut device8).cast()).ok()
    })?;
    let device8 = device8.ok_or("missing Xbox Device8")?;
    type Deserialize = unsafe extern "system" fn(
        *mut c_void,
        *const u8,
        u32,
        *mut c_void,
        u32,
        *const GUID,
        *mut *mut c_void,
    ) -> HRESULT;
    // SAFETY: slot 115 on this Xbox Device8 IID is DeserializeStateObjectX.
    let deserialize: Deserialize = unsafe {
        let slots = *device8.as_raw().cast::<*const *const c_void>();
        std::mem::transmute(*slots.add(115))
    };
    let mut report = File::create(report_path).map_err(|error| error.to_string())?;
    let mut paths: Vec<PathBuf> = fs::read_dir(state_dir)
        .map_err(|e| e.to_string())?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<io::Result<_>>()
        .map_err(|e| e.to_string())?;
    paths.sort();
    for path in paths
        .iter()
        .filter(|path| path.extension() == Some(OsStr::new("sptr")))
    {
        let state = fs::read(path).map_err(|error| error.to_string())?;
        let size = u32::try_from(state.len()).map_err(|_| "ray state too large")?;
        let mut collection: Option<ID3D12StateObject> = None;
        let start = Instant::now();
        // SAFETY: the input and global root remain live until the synchronous
        // host creation returns. The output receives an owned reference.
        checked("DeserializeStateObjectX", unsafe {
            deserialize(
                device8.as_raw(),
                state.as_ptr(),
                size,
                root.as_raw(),
                0,
                &ID3D12StateObject::IID,
                (&raw mut collection).cast(),
            )
            .ok()
        })?;
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let _collection = collection.ok_or("missing ray collection")?;
        let key = path
            .file_stem()
            .ok_or("missing ray-state key")?
            .to_string_lossy();
        writeln!(report, "{key} {elapsed_ms}")
            .and_then(|()| report.flush())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn prepare(
    title_dir: &Path,
    report_path: &Path,
    shaders: &[PathBuf],
    standalone: bool,
) -> Result<(), String> {
    let directory_error = "cannot select the title's binary directory";
    let title_dir = std::path::absolute(title_dir).map_err(|_| directory_error)?;
    std::env::set_current_dir(&title_dir).map_err(|_| directory_error)?;
    // SAFETY: each Win32 path buffer is NUL-terminated and lives through the
    // call. The owned module is declared before the COM objects that call it.
    let module = unsafe {
        SetDllDirectoryW(PCWSTR(wide(&title_dir).as_ptr())).map_err(|_| directory_error)?;
        let provider = wide(&title_dir.join("d3d12_xs.dll"));
        Owned::new(
            LoadLibraryW(PCWSTR(provider.as_ptr())).map_err(|_| "missing Xbox device factory")?,
        )
    };
    let guest = create_guest_device(*module)?;
    if let [flag, state_dir] = shaders
        && standalone
        && flag == Path::new("--ray-collections")
    {
        return ray_collections(&guest, state_dir, report_path);
    }
    let root = profiled_root(
        &guest,
        if standalone {
            Layout::Compute
        } else {
            Layout::ReferenceCompute
        },
    )?;

    // The reference renderer returns native root objects, whereas TuxBox
    // returns guest objects which must go through the Xbox pipeline slot.
    let mut host: Option<ID3D12Device> = None;
    if !standalone {
        // SAFETY: root is live and GetDevice writes to an empty COM reference slot.
        checked("GetDevice", unsafe { root.GetDevice(&mut host) })?;
        if host.is_none() {
            return Err("missing reference device".into());
        }
    }

    let mut report = File::create(report_path).map_err(|error| error.to_string())?;
    for shader_path in shaders {
        let shader = fs::read(shader_path).map_err(|_| "missing prepared compute shader")?;
        if shader.is_empty() {
            return Err("missing prepared compute shader".into());
        }
        let descriptor = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: ManuallyDrop::new(Some(root.clone())),
            NodeMask: 1,
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: shader.as_ptr().cast(),
                BytecodeLength: shader.len(),
            },
            ..Default::default()
        };
        let start = Instant::now();
        // SAFETY: the descriptor borrows initialized shader bytes for the call.
        // Release its explicit COM reference on success and failure.
        let result = match &host {
            Some(host) => unsafe {
                host.CreateComputePipelineState::<ID3D12PipelineState>(&descriptor)
            },
            None => guest_pipeline(&guest, &descriptor),
        };
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        drop(ManuallyDrop::into_inner(descriptor.pRootSignature));
        let _pipeline = checked("CreateComputePipelineState", result)?;
        let key = shader_path
            .file_stem()
            .unwrap_or(OsStr::new(""))
            .to_string_lossy();
        writeln!(report, "{key} {elapsed_ms}")
            .and_then(|()| report.flush())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Unicode console entry point invoked by the MSVC CRT. Rust's Windows
/// args_os reads the native command line without a narrow-codepage conversion.
#[unsafe(no_mangle)]
pub extern "C" fn wmain(_argc: i32, _argv: *const *const u16) -> i32 {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [title_dir, report, shaders @ ..] = args.as_slice() else {
        return 2;
    };
    let standalone = shaders
        .first()
        .is_some_and(|arg| arg == Path::new("--tuxbox"));
    let shaders = if standalone { &shaders[1..] } else { shaders };
    if shaders.is_empty() {
        return 2;
    }
    match prepare(title_dir, report, shaders, standalone) {
        Ok(()) => 0,
        Err(error) => {
            if let Ok(mut output) = OpenOptions::new().create(true).append(true).open(report) {
                let _ = writeln!(output, "{error}");
            }
            let _ = writeln!(io::stderr().lock(), "Pipeline preparation failed: {error}");
            1
        }
    }
}
