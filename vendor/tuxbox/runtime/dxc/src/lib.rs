//! Scarlett ray compiler factories and desktop DXIL library linking.
mod bindings;
mod continuations;
use bindings::{Bindings, Multiple, Single};
use core::ffi::c_void;
use core::mem::size_of;
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicU32, Ordering};
use windows::Win32::Foundation::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, REGDB_E_CLASSNOTREG, S_OK,
};
use windows::Win32::Graphics::Direct3D::Dxc::{CLSID_DxcAssembler, DxcBuffer, IDxcAssembler};
use windows::Win32::Graphics::Direct3D::Dxc::{
    CLSID_DxcLinker, CLSID_DxcUtils, DXC_CP_ACP, DxcCreateInstance2, IDxcLinker,
    IDxcOperationResult, IDxcUtils,
};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_FUNCTION_DESC, D3D12_SHADER_INPUT_BIND_DESC, ID3D12LibraryReflection,
};
use windows::Win32::System::Com::{CoGetMalloc, IMalloc, INoMarshal};
use windows::core::{GUID, HRESULT, IUnknown, Interface, PCWSTR, Result, w};

const COMPILER_ID: GUID = GUID::from_u128(0x16bb3c85_006d_4b72_9e10_63cd97df57f0);

#[repr(C)]
struct Compiler {
    methods: &'static Methods,
    refs: AtomicU32,
    allocator: IMalloc,
    callback: AtomicPtr<c_void>,
    option_a: AtomicU8,
    option_b: AtomicU8,
    debug: AtomicU32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Bytecode {
    data: *const c_void,
    len: u32,
}

#[repr(C)]
struct Export {
    source: PCWSTR,
    name: PCWSTR,
    flags: u32,
}

#[repr(C)]
struct LibraryGroup {
    code: Bytecode,
    count: u32,
    exports: *const Export,
}

type Output = *mut *mut c_void;
type Rename = unsafe extern "system" fn(
    *const Compiler,
    *const Bytecode,
    u32,
    *const Export,
    u32,
    Output,
) -> HRESULT;

#[repr(C)]
struct Methods {
    query: unsafe extern "system" fn(*const Compiler, *const GUID, Output) -> HRESULT,
    add_ref: unsafe extern "system" fn(*const Compiler) -> u32,
    release: unsafe extern "system" fn(*mut Compiler) -> u32,
    set_a: unsafe extern "system" fn(*const Compiler, u8) -> HRESULT,
    set_b: unsafe extern "system" fn(*const Compiler, u8) -> HRESULT,
    set_debug: unsafe extern "system" fn(*const Compiler, u32) -> HRESULT,
    rename: Rename,
    patch: unsafe extern "system" fn(
        *const Compiler,
        PCWSTR,
        *const Bytecode,
        *const Single,
        Output,
    ) -> HRESULT,
    compile: unsafe extern "system" fn(
        *const Compiler,
        *const Bytecode,
        u32,
        *const PCWSTR,
        *mut continuations::ShaderInfo,
        u32,
        u32,
        Output,
    ) -> HRESULT,
    link: unsafe extern "system" fn(
        *const Compiler,
        PCWSTR,
        *const *const c_void,
        u32,
        *const PCWSTR,
        u32,
        u32,
        u32,
        Output,
    ) -> HRESULT,
    set_callback: unsafe extern "system" fn(*const Compiler, *mut c_void),
    rename_grouped: unsafe extern "system" fn(
        *const Compiler,
        *const LibraryGroup,
        u32,
        *const Export,
        u32,
        Output,
    ) -> HRESULT,
    patch_multi: unsafe extern "system" fn(
        *const Compiler,
        *const Bytecode,
        *const Multiple,
        Output,
    ) -> HRESULT,
}

static METHODS: Methods = Methods {
    query,
    add_ref,
    release,
    set_a,
    set_b,
    set_debug,
    rename,
    patch,
    compile,
    link,
    set_callback,
    rename_grouped,
    patch_multi,
};

unsafe extern "system" fn query(this: *const Compiler, iid: *const GUID, out: Output) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: The caller supplies a live object, readable IID and writable output.
    let iid = unsafe { *iid };
    if iid != COMPILER_ID && iid != IUnknown::IID && iid != INoMarshal::IID {
        return E_NOINTERFACE;
    }
    unsafe {
        add_ref(this);
        out.write(this.cast_mut().cast());
    }
    S_OK
}

unsafe extern "system" fn add_ref(this: *const Compiler) -> u32 {
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn release(this: *mut Compiler) -> u32 {
    let refs = unsafe { (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1 };
    if refs == 0 {
        // SAFETY: The last reference owns both the object allocation and its allocator.
        // Move the object out before freeing its backing store and dropping its IMalloc.
        let object = unsafe { this.read() };
        unsafe { object.allocator.Free(Some(this.cast())) };
    }
    refs
}

#[unsafe(export_name = "CreateXdxrCompiler")]
unsafe extern "system" fn create(class: *const GUID, iid: *const GUID, out: Output) -> HRESULT {
    // SAFETY: Both factories share the same three arguments.
    unsafe { create_with_allocator(ptr::null_mut(), class, iid, out) }
}

#[unsafe(export_name = "CreateXdxrCompiler2")]
unsafe extern "system" fn create_with_allocator(
    allocator: *mut c_void,
    class: *const GUID,
    iid: *const GUID,
    out: Output,
) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { out.write(ptr::null_mut()) };
    if class.is_null() || iid.is_null() {
        return E_POINTER;
    }
    if unsafe { *class != COMPILER_ID } {
        return REGDB_E_CLASSNOTREG;
    }
    // SAFETY: A supplied IMalloc is valid for the call. Retain it for the object's lifetime.
    let allocator = if let Some(borrowed) = unsafe { IMalloc::from_raw_borrowed(&allocator) } {
        borrowed.clone()
    } else {
        match unsafe { CoGetMalloc(1) } {
            Ok(a) => a,
            Err(e) => return e.code(),
        }
    };
    let object = unsafe { allocator.Alloc(size_of::<Compiler>()) }.cast::<Compiler>();
    if object.is_null() {
        return E_OUTOFMEMORY;
    }
    unsafe {
        object.write(Compiler {
            methods: &METHODS,
            refs: AtomicU32::new(1),
            allocator,
            callback: AtomicPtr::new(ptr::null_mut()),
            option_a: AtomicU8::new(0),
            option_b: AtomicU8::new(0),
            debug: AtomicU32::new(0),
        });
        let hr = query(object, iid, out);
        release(object);
        hr
    }
}

unsafe extern "system" fn set_a(this: *const Compiler, value: u8) -> HRESULT {
    unsafe { (*this).option_a.store(value, Ordering::Relaxed) };
    S_OK
}
unsafe extern "system" fn set_b(this: *const Compiler, value: u8) -> HRESULT {
    unsafe { (*this).option_b.store(value, Ordering::Relaxed) };
    S_OK
}
unsafe extern "system" fn set_debug(this: *const Compiler, value: u32) -> HRESULT {
    unsafe { (*this).debug.store(value, Ordering::Relaxed) };
    S_OK
}
unsafe extern "system" fn set_callback(this: *const Compiler, callback: *mut c_void) {
    unsafe { (*this).callback.store(callback, Ordering::Release) };
}

unsafe fn rename_libraries(
    allocator: &IMalloc,
    libraries: &[Bytecode],
    exports: &[Export],
) -> Result<IDxcOperationResult> {
    // SAFETY: The desktop linker and blob utility are retained for the duration of the synchronous link.
    let linker: IDxcLinker = unsafe { DxcCreateInstance2(allocator, &CLSID_DxcLinker)? };
    let utils: IDxcUtils = unsafe { DxcCreateInstance2(allocator, &CLSID_DxcUtils)? };
    let names: Vec<Vec<u16>> = (0..libraries.len())
        .map(|i| format!("lib{i}\0").encode_utf16().collect())
        .collect();
    let pointers: Vec<_> = names.iter().map(|s| PCWSTR(s.as_ptr())).collect();
    for (library, &name) in libraries.iter().zip(&pointers) {
        if library.data.is_null() {
            return Err(E_POINTER.into());
        }
        let blob = unsafe { utils.CreateBlob(library.data, library.len, DXC_CP_ACP)? };
        unsafe { linker.RegisterLibrary(name, &blob)? };
    }
    let mut list = Vec::new();
    for (index, export) in exports.iter().enumerate() {
        if export.name.is_null() {
            return Err(E_POINTER.into());
        }
        if index != 0 {
            list.push(b';' as u16);
        }
        // SAFETY: Each export name is a caller-provided NUL-terminated wide string.
        list.extend_from_slice(unsafe { export.name.as_wide() });
        if !export.source.is_null() {
            list.push(b'=' as u16);
            list.extend_from_slice(unsafe { export.source.as_wide() });
        }
    }
    list.push(0);
    let mut args = vec![w!("-Vd")];
    if !exports.is_empty() {
        args.extend([w!("-exports"), PCWSTR(list.as_ptr())]);
    }
    unsafe { linker.Link(PCWSTR::null(), w!("lib_6_3"), &pointers, Some(&args)) }
}

unsafe extern "system" fn rename(
    this: *const Compiler,
    libs: *const Bytecode,
    count: u32,
    exports: *const Export,
    n: u32,
    out: Output,
) -> HRESULT {
    if libs.is_null() || exports.is_null() || out.is_null() {
        return E_POINTER;
    }
    if n == 0 || count == 0 {
        return E_INVALIDARG;
    }
    unsafe {
        out.write(ptr::null_mut());
        let libraries = core::slice::from_raw_parts(libs, count as usize);
        let exports = core::slice::from_raw_parts(exports, n as usize);
        match rename_libraries(&(*this).allocator, libraries, exports) {
            Ok(result) => {
                out.write(result.into_raw());
                S_OK
            }
            Err(error) => error.code(),
        }
    }
}

unsafe fn link_groups(
    allocator: &IMalloc,
    groups: &[LibraryGroup],
    exports: &[Export],
) -> Result<IDxcOperationResult> {
    let mut intermediate = Vec::new();
    let mut libraries = Vec::with_capacity(groups.len());
    for group in groups {
        let local = unsafe { bindings::records(group.exports, group.count)? };
        if local.is_empty() {
            libraries.push(group.code);
            continue;
        }
        // Apply this group's names in isolation. Another library may rename the
        // same input symbol to a different public shader.
        let result =
            unsafe { rename_libraries(allocator, core::slice::from_ref(&group.code), local)? };
        if unsafe { result.GetStatus()? }.is_err() {
            return Ok(result);
        }
        let blob = unsafe { result.GetResult()? };
        libraries.push(Bytecode {
            data: unsafe { blob.GetBufferPointer() },
            len: unsafe { blob.GetBufferSize() }
                .try_into()
                .map_err(|_| E_INVALIDARG)?,
        });
        intermediate.push(blob);
    }
    // The blobs owning the intermediate bytecode remain alive through this link.
    unsafe { rename_libraries(allocator, &libraries, exports) }
}

unsafe extern "system" fn rename_grouped(
    this: *const Compiler,
    libs: *const LibraryGroup,
    count: u32,
    exports: *const Export,
    n: u32,
    out: Output,
) -> HRESULT {
    if libs.is_null() || out.is_null() {
        return E_POINTER;
    }
    if count == 0 {
        return E_INVALIDARG;
    }
    unsafe { out.write(ptr::null_mut()) };
    let global = match unsafe { bindings::records(exports, n) } {
        Ok(s) => s,
        Err(e) => return e.code(),
    };
    // SAFETY: The grouped ABI uses 32-byte input records, each with its own export array.
    let groups = unsafe { core::slice::from_raw_parts(libs, count as usize) };
    match unsafe { link_groups(&(*this).allocator, groups, global) } {
        Ok(result) => {
            unsafe { out.write(result.into_raw()) };
            S_OK
        }
        Err(error) => error.code(),
    }
}

// On desktop the state object binds global resources directly. Reassemble the
// portable library after checking that every reflected resource resolves.
unsafe fn patch_library(
    allocator: &IMalloc,
    code: &Bytecode,
    bindings: &Bindings,
) -> Result<IDxcOperationResult> {
    if code.data.is_null() {
        return Err(E_POINTER.into());
    }
    let utils: IDxcUtils = unsafe { DxcCreateInstance2(allocator, &CLSID_DxcUtils)? };
    let buffer = DxcBuffer {
        Ptr: code.data,
        Size: code.len as usize,
        Encoding: 0,
    };
    let mut reflection = ptr::null_mut();
    unsafe { utils.CreateReflection(&buffer, &ID3D12LibraryReflection::IID, &mut reflection)? };
    let reflection = unsafe { ID3D12LibraryReflection::from_raw(reflection) };
    let count = unsafe { reflection.GetDesc()? }.FunctionCount;
    if count == 0 {
        return Err(E_NOTIMPL.into());
    }
    for index in 0..count {
        let function =
            unsafe { reflection.GetFunctionByIndex(index as i32) }.ok_or(E_INVALIDARG)?;
        let mut desc = D3D12_FUNCTION_DESC::default();
        unsafe { function.GetDesc(&mut desc)? };
        // Later shader models can access a descriptor heap without a named binding.
        if (desc.Version & 0xff) > 0x63 {
            return Err(E_NOTIMPL.into());
        }
        for index in 0..desc.BoundResources {
            let mut resource = D3D12_SHADER_INPUT_BIND_DESC::default();
            unsafe { function.GetResourceBindingDesc(index, &mut resource)? };
            if !unsafe { bindings.resolves(desc.Name, &resource) } {
                return Err(E_NOTIMPL.into());
            }
        }
    }
    let assembler: IDxcAssembler = unsafe { DxcCreateInstance2(allocator, &CLSID_DxcAssembler)? };
    let mut part = ptr::null_mut();
    let mut len = 0;
    unsafe {
        utils.GetDxilContainerPart(&buffer, u32::from_le_bytes(*b"DXIL"), &mut part, &mut len)?
    };
    if part.is_null() || len < 24 {
        return Err(E_INVALIDARG.into());
    }
    // The offset is relative to the bitcode header, eight bytes into the part.
    let bytes = unsafe { core::slice::from_raw_parts(part.cast::<u8>(), len as usize) };
    let offset = 8 + u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
    let size = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
    let bitcode = bytes
        .get(offset..offset + size as usize)
        .ok_or(E_INVALIDARG)?;
    let blob = unsafe { utils.CreateBlob(bitcode.as_ptr().cast(), size, DXC_CP_ACP)? };
    unsafe { assembler.AssembleToContainer(&blob) }
}

unsafe fn patch_result(
    this: *const Compiler,
    code: *const Bytecode,
    bindings: Result<Bindings>,
    out: Output,
) -> HRESULT {
    unsafe { out.write(ptr::null_mut()) };
    match bindings.and_then(|b| unsafe { patch_library(&(*this).allocator, &*code, &b) }) {
        Ok(result) => {
            unsafe { out.write(result.into_raw()) };
            S_OK
        }
        Err(e) => e.code(),
    }
}

unsafe extern "system" fn patch(
    this: *const Compiler,
    _: PCWSTR,
    code: *const Bytecode,
    info: *const Single,
    out: Output,
) -> HRESULT {
    if code.is_null() || info.is_null() || out.is_null() {
        return E_POINTER;
    }
    unsafe { patch_result(this, code, Bindings::from_single(&*info), out) }
}

unsafe extern "system" fn patch_multi(
    this: *const Compiler,
    code: *const Bytecode,
    info: *const Multiple,
    out: Output,
) -> HRESULT {
    if code.is_null() || info.is_null() || out.is_null() {
        return E_POINTER;
    }
    unsafe { patch_result(this, code, Bindings::from_multiple(&*info), out) }
}
unsafe extern "system" fn compile(
    this: *const Compiler,
    libs: *const Bytecode,
    count: u32,
    names: *const PCWSTR,
    info: *mut continuations::ShaderInfo,
    n: u32,
    _: u32,
    out: Output,
) -> HRESULT {
    if libs.is_null() || names.is_null() || out.is_null() {
        return E_POINTER;
    }
    if count == 0 || n == 0 {
        return E_INVALIDARG;
    }
    unsafe { out.write(ptr::null_mut()) };
    let libraries = unsafe { core::slice::from_raw_parts(libs, count as usize) };
    let names = unsafe { core::slice::from_raw_parts(names, n as usize) };
    match unsafe { continuations::compile(&*this, libraries, names, info) } {
        Ok(result) => {
            unsafe { out.write(result.into_raw()) };
            S_OK
        }
        Err(error) => error.code(),
    }
}
unsafe extern "system" fn link(
    _: *const Compiler,
    _: PCWSTR,
    libs: *const *const c_void,
    count: u32,
    names: *const PCWSTR,
    n: u32,
    _: u32,
    _: u32,
    out: Output,
) -> HRESULT {
    if libs.is_null() || names.is_null() || out.is_null() {
        return E_POINTER;
    }
    if count == 0 || n == 0 {
        return E_INVALIDARG;
    }
    unsafe { out.write(ptr::null_mut()) };
    E_NOTIMPL
}
