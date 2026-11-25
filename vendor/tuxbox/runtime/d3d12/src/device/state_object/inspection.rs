//! Console ray state inspection and host DXR properties share a COM identity.
use super::{Object, Out, query, wide};
use crate::proxy::{self, Slots};
use core::{ffi::c_void, mem::offset_of, ptr};
use std::sync::OnceLock;
use tuxbox_dxbc::ray::State;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_POINTER, S_OK};
use windows::Win32::Graphics::{Direct3D::ID3DBlob, Direct3D12::*};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::core::{GUID, HRESULT, IUnknown, Interface, PCWSTR, Result, s, w};

#[repr(C)]
pub(super) struct Inspection {
    properties: &'static Slots<8>,
    inspector: &'static Slots<10>,
    native: ID3D12StateObjectProperties,
    exports: ID3DBlob,
    symbols: ID3DBlob,
    entries: Vec<Export>,
}

fn blob(size: usize) -> Result<ID3DBlob> {
    type Create = unsafe extern "system" fn(usize, *mut Option<ID3DBlob>) -> HRESULT;
    static CREATE: OnceLock<Option<Create>> = OnceLock::new();
    let create = CREATE
        .get_or_init(|| unsafe {
            let module =
                LoadLibraryExW(w!("d3dcompiler_47.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
                    .ok()?;
            Some(core::mem::transmute::<_, Create>(GetProcAddress(
                module,
                s!("D3DCreateBlob"),
            )?))
        })
        .ok_or(E_NOTIMPL)?;
    let mut result = None;
    unsafe {
        create(size, &mut result).ok()?;
    }
    result.ok_or_else(|| E_POINTER.into())
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*data.get(offset..)?.first_chunk()?))
}

// A program trailer carries 24-byte symbol records followed by their terminated
// names. Serialized name references are byte offsets relative to that table.
pub(super) struct Export {
    name: String,
    functions: std::sync::Arc<[Function]>,
}

struct Function {
    name: Vec<u8>,
    data: [u8; 16],
}

fn functions(state: &State) -> Result<Export> {
    let count = u32_at(state.symbols, 0x38).ok_or(E_INVALIDARG)? as usize;
    let table = state.symbols.get(0x40..).ok_or(E_INVALIDARG)?;
    let records = table
        .get(..count.checked_mul(24).ok_or(E_INVALIDARG)?)
        .ok_or(E_INVALIDARG)?;
    if count == 0 {
        return Err(E_INVALIDARG.into());
    }
    let mut functions = Vec::with_capacity(count);
    for raw in records.as_chunks::<24>().0 {
        let offset = u64::from_le_bytes(raw[..8].try_into().unwrap()) as usize;
        let tail = table
            .get(offset..)
            .filter(|_| offset >= records.len())
            .ok_or(E_INVALIDARG)?;
        let end = tail.iter().position(|&v| v == 0).ok_or(E_INVALIDARG)?;
        functions.push(Function {
            name: tail[..=end].to_vec(),
            data: raw[8..].try_into().unwrap(),
        });
    }
    Ok(Export {
        name: state.hit_group.as_ref().unwrap_or(&state.name).clone(),
        functions: functions.into(),
    })
}

fn symbol_blob(exports: &[Export]) -> Result<ID3DBlob> {
    let records: Vec<_> = exports
        .iter()
        .flat_map(|entry| entry.functions.iter())
        .collect();
    let strings = records.iter().map(|f| f.name.len()).sum::<usize>();
    let size = 24 + 24 * records.len() + strings;
    if size > (64 << 20) {
        return Err(E_INVALIDARG.into());
    }
    let result = blob(size)?;
    unsafe {
        let base = result.GetBufferPointer().cast::<u8>();
        base.write_bytes(0, 24);
        let table = base.add(24);
        base.cast::<*mut u8>().write(table);
        base.add(16).cast::<u32>().write(records.len() as u32);
        let mut string = table.add(24 * records.len());
        for (index, function) in records.iter().enumerate() {
            let record = table.add(24 * index);
            record.cast::<*mut u8>().write(string);
            ptr::copy_nonoverlapping(function.data.as_ptr(), record.add(8), 16);
            ptr::copy_nonoverlapping(function.name.as_ptr(), string, function.name.len());
            string = string.add(function.name.len());
        }
    }
    Ok(result)
}

fn export_blob(props: &ID3D12StateObjectProperties, entries: &[Export]) -> Result<ID3DBlob> {
    let names: Vec<_> = entries.iter().map(|e| wide(&e.name)).collect();
    let bytes = 16 + 64 * entries.len() + names.iter().map(|w| 2 * w.len()).sum::<usize>();
    if bytes > (64 << 20) {
        return Err(E_INVALIDARG.into());
    }
    let output = blob(bytes)?;
    unsafe {
        let base = output.GetBufferPointer().cast::<u8>();
        base.write_bytes(0, bytes);
        let table = base.add(16);
        base.cast::<*mut u8>().write(table);
        base.add(8).cast::<u32>().write(entries.len() as u32);
        let mut text = table.add(64 * entries.len()).cast::<u16>();
        for (index, (entry, name)) in entries.iter().zip(names).enumerate() {
            let id = props.GetShaderIdentifier(PCWSTR(name.as_ptr()));
            if id.is_null() {
                return Err(E_NOTIMPL.into());
            }
            let row = table.add(64 * index);
            row.cast::<*mut u16>().write(text);
            ptr::copy_nonoverlapping(name.as_ptr(), text, name.len());
            text = text.add(name.len());
            ptr::copy_nonoverlapping(id.cast::<u8>(), row.add(8), 32);
            let mut vgpr = 0u16;
            let mut sgpr = 0u16;
            let mut scratch = 0u16;
            for function in entry.functions.iter().filter(|f| {
                let primary = f.name.split(|&c| c == b'.').next().unwrap_or_default();
                primary != b"XDXR_TraceRay" && primary != b"main\0"
            }) {
                let regs = u32::from_le_bytes(function.data[12..16].try_into().unwrap());
                vgpr = vgpr.max((regs & 511) as u16);
                sgpr = sgpr.max(((regs >> 11) & 255) as u16);
                scratch = scratch.max(u16::from_le_bytes(function.data[6..8].try_into().unwrap()));
            }
            row.add(44).cast::<u32>().write(u32::from(scratch));
            row.add(56).cast::<u16>().write(vgpr);
            row.add(58).cast::<u16>().write(sgpr);
        }
    }
    Ok(output)
}

impl Inspection {
    pub(super) fn from_state(native: &ID3D12StateObject, state: &State) -> Result<Self> {
        Self::from_exports(native, vec![functions(state)?])
    }

    pub(super) fn from_exports(native: &ID3D12StateObject, entries: Vec<Export>) -> Result<Self> {
        let props = native.cast()?;
        let exports = export_blob(&props, &entries)?;
        let symbols = symbol_blob(&entries)?;
        Ok(Self {
            properties: &PROPERTIES,
            inspector: &INSPECTOR,
            native: props,
            exports,
            symbols,
            entries,
        })
    }

    pub(super) unsafe fn import(
        &self,
        count: u32,
        selected: *const D3D12_EXPORT_DESC,
        out: &mut Vec<Export>,
    ) -> Result<()> {
        if count == 0 {
            out.extend(self.entries.iter().map(|e| Export {
                name: e.name.clone(),
                functions: e.functions.clone(),
            }));
            return Ok(());
        }
        if selected.is_null() || count > 65_536 {
            return Err(E_INVALIDARG.into());
        }
        for selection in unsafe { core::slice::from_raw_parts(selected, count as usize) } {
            if selection.Name.is_null() {
                return Err(E_INVALIDARG.into());
            }
            let src = if selection.ExportToRename.is_null() {
                selection.Name
            } else {
                selection.ExportToRename
            };
            let src = unsafe { src.to_string() }.map_err(|_| E_INVALIDARG)?;
            if let Some(entry) = self.entries.iter().find(|e| e.name == src) {
                let name = unsafe { selection.Name.to_string() }.map_err(|_| E_INVALIDARG)?;
                out.push(Export {
                    name,
                    functions: entry.functions.clone(),
                });
            }
        }
        Ok(())
    }
}
unsafe fn object(this: *mut c_void) -> *mut Object {
    unsafe { this.byte_sub(offset_of!(Object, state)).cast() }
}

unsafe extern "system" fn prop_query(this: *mut c_void, iid: *const GUID, out: Out) -> HRESULT {
    unsafe { query(object(this), iid, out) }
}
unsafe extern "system" fn add(this: *mut c_void) -> u32 {
    unsafe { proxy::add_ref(object(this)) }
}
unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    unsafe { proxy::release(object(this)) }
}
unsafe extern "system" fn identifier(this: &Inspection, name: PCWSTR) -> *mut c_void {
    unsafe { this.native.GetShaderIdentifier(name) }
}
unsafe extern "system" fn shader_stack(this: &Inspection, name: PCWSTR) -> u64 {
    unsafe { this.native.GetShaderStackSize(name) }
}
unsafe extern "system" fn pipeline_stack(this: &Inspection) -> u64 {
    unsafe { this.native.GetPipelineStackSize() }
}
unsafe extern "system" fn set_stack(this: &Inspection, size: u64) {
    unsafe {
        this.native.SetPipelineStackSize(size);
    }
}

unsafe extern "system" fn info(this: &Inspection, selector: usize, out: *mut c_void) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    if selector == 1 {
        let exports = &this.exports;
        unsafe {
            out.cast::<[usize; 2]>()
                .write_unaligned([exports.clone().into_raw() as usize, 0]);
        }
        return S_OK;
    }
    // Native DXR provides no Xbox scratch-memory or wave scheduler statistics.
    E_NOTIMPL
}

pub(super) unsafe extern "system" fn get_inspector(this: *mut Object, out: Out) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe {
        out.write(ptr::addr_of_mut!((*this).state.inspector).cast());
    }
    S_OK
}

unsafe extern "system" fn shader_info(
    this: *mut c_void,
    kind: u32,
    index: u32,
    out: Out,
) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe {
        out.write(ptr::null_mut());
    }
    if kind != 1 || index != 0 {
        return E_NOTIMPL;
    }
    let state = unsafe {
        &*this
            .byte_sub(offset_of!(Inspection, inspector))
            .cast::<Inspection>()
    };
    unsafe {
        out.write(state.symbols.clone().into_raw());
    }
    S_OK
}

static PROPERTIES: Slots<8> = [
    crate::slot!(prop_query),
    crate::slot!(add),
    crate::slot!(release),
    crate::slot!(identifier),
    crate::slot!(shader_stack),
    crate::slot!(pipeline_stack),
    crate::slot!(set_stack),
    crate::slot!(info),
];
unsafe fn inspector_owner(this: *mut c_void) -> *mut Object {
    unsafe { object(this.byte_sub(offset_of!(Inspection, inspector))) }
}

unsafe extern "system" fn inspector_query(
    this: *mut c_void,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe {
        out.write(ptr::null_mut());
    }
    if iid.is_null() {
        return E_POINTER;
    }
    if ![IUnknown::IID, ID3D12PipelineState::IID].contains(&unsafe { iid.read_unaligned() }) {
        return windows::Win32::Foundation::E_NOINTERFACE;
    }
    unsafe {
        proxy::add_ref(inspector_owner(this));
        out.write(this);
    }
    S_OK
}
unsafe extern "system" fn inspector_add(this: *mut c_void) -> u32 {
    unsafe { proxy::add_ref(inspector_owner(this)) }
}
unsafe extern "system" fn inspector_release(this: *mut c_void) -> u32 {
    unsafe { proxy::release(inspector_owner(this)) }
}

static INSPECTOR: Slots<10> = {
    let mut slots = proxy::unsupported_slots();
    slots[0] = crate::slot!(inspector_query);
    slots[1] = crate::slot!(inspector_add);
    slots[2] = crate::slot!(inspector_release);
    // The inspector is borrowed (matching state slot 8). Do not dispatch
    // unsupported methods through primary-object handlers with a different this.
    slots[9] = crate::slot!(shader_info);
    slots
};
