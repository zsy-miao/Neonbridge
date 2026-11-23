//! Lower resource-free miss shaders to the Series driver's continuation ABI.
//! The driver supplies its runtime IR and owns the layout of payload and stack.
use super::{Bytecode, Compiler};
use core::ffi::{CStr, c_char, c_void};
use core::fmt::Write;
use core::sync::atomic::Ordering;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_POINTER};
use windows::Win32::Graphics::Direct3D::Dxc::*;
use windows::core::{PCWSTR, Result};

#[repr(C)]
pub(super) struct ShaderInfo {
    state: u32,
    stack_bytes: u32,
}

struct Leaf {
    name: String,
    /// Original DXIL parameter declaration and body, including SSA numbering.
    parameter: String,
    body: String,
    fields: String,
    payload: String,
    words: usize,
}

impl Leaf {
    fn read(module: &str, name: &str) -> Result<Self> {
        // These simple identifiers are also legal unquoted LLVM symbols.
        if name.is_empty() || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            return Err(E_NOTIMPL.into());
        }
        let mangled = format!("@\"\\01?{name}@@");
        let plain = format!("@{name}(");
        let definition = module
            .split("\ndefine void ")
            .skip(1)
            .find(|d| d.starts_with(&mangled) || d.starts_with(&plain))
            .ok_or(E_NOTIMPL)?;
        let (symbol, rest) = definition.split_once('(').ok_or(E_INVALIDARG)?;
        let (parameter, rest) = rest.split_once(')').ok_or(E_INVALIDARG)?;
        let payload = parameter.split_once('*').ok_or(E_NOTIMPL)?.0;
        if !payload.starts_with('%') || parameter.contains(',') {
            return Err(E_NOTIMPL.into());
        }
        let declaration = format!("{payload} = type {{ ");
        let fields = module
            .lines()
            .find_map(|s| s.strip_prefix(&declaration)?.strip_suffix(" }"))
            .ok_or(E_NOTIMPL)?;
        let words = fields.split(", ").count();
        if words > 5 || !fields.split(", ").all(|ty| matches!(ty, "i32" | "float")) {
            return Err(E_NOTIMPL.into());
        }
        // The shader property list specifies a miss shader (kind 11) and its
        // payload byte count. Leave other stages and aggregate layouts for
        // their own lowering paths.
        let entry_type = format!("= !{{void ({payload}*)* {symbol}, !");
        let props = module
            .lines()
            .find(|s| s.contains(&entry_type))
            .and_then(|s| s.strip_suffix('}'))
            .and_then(|s| s.rsplit_once(", !"))
            .map(|(_, id)| id)
            .ok_or(E_NOTIMPL)?;
        let expected = format!("!{props} = !{{i32 8, i32 11, i32 6, i32 {},", words * 4);
        if !module.lines().any(|s| s.starts_with(&expected)) {
            return Err(E_NOTIMPL.into());
        }
        let (_, lines) = rest.split_once('{').ok_or(E_INVALIDARG)?;
        let (body, _) = lines.split_once("\n}").ok_or(E_INVALIDARG)?;
        // External calls, globals and instruction metadata can require another
        // module or a suspension point. An isolated scalar payload body cannot.
        if body.contains(['@', '!']) {
            return Err(E_NOTIMPL.into());
        }
        Ok(Self {
            name: name.to_owned(),
            parameter: parameter.into(),
            body: body.into(),
            fields: fields.into(),
            payload: payload.into(),
            words,
        })
    }

    fn append(&self, ir: &mut String, index: usize) {
        let ty = format!("%tuxbox.payload.{index}");
        writeln!(ir, "{ty} = type {{ {} }}", self.fields).unwrap();
        let parameter = self.parameter.replace(&self.payload, &ty);
        let body = self.body.replace(&self.payload, &ty);
        writeln!(
            ir,
            r#"define internal void @tuxbox.body.{index}({parameter}) alwaysinline {{{body}
}}"#
        )
        .unwrap();
        let runtime = "%struct.RuntimeDataStruct* %runtime";
        writeln!(ir, "define cc804 void @{}.ss_0({runtime}, [256 x i32]* %stack) noinline \"state_function\"=\"true\" \"xdxr.shaderkind\"=\"miss\" \"xdxr_indirect_callable\" {{", self.name).unwrap();
        writeln!(
            ir,
            r#"entry:
  %payload = alloca {ty}, align 4
  %base = bitcast {ty}* %payload to i32*"#
        )
        .unwrap();
        for i in 0..self.words {
            writeln!(
                ir,
                r#"  %ptr{i} = getelementptr i32, i32* %base, i32 {i}
  %in{i} = call i32 @payloadDword({runtime}, i32 {i})
  store i32 %in{i}, i32* %ptr{i}"#
            )
            .unwrap();
        }
        writeln!(ir, "  call void @tuxbox.body.{index}({ty}* %payload)").unwrap();
        for i in 0..self.words {
            writeln!(
                ir,
                r#"  %out{i} = load i32, i32* %ptr{i}
  call void @setPayloadDword({runtime}, i32 {i}, i32 %out{i})"#
            )
            .unwrap();
        }
        writeln!(
            ir,
            r#"  %off = call i32 @stackFrameOffset({runtime})
  %next = call i32* @stackIntPtr({runtime}, [256 x i32]* %stack, i32 %off, i32 0)
  %id = load i32, i32* %next
  call void @__xdxr_impl_XDXR_SetStateId({runtime}, i32 %id)
  ret void
}}"#
        )
        .unwrap();
    }
}

unsafe fn driver_module(compiler: &Compiler) -> Result<String> {
    let driver = compiler.callback.load(Ordering::Acquire);
    if driver.is_null() {
        return Err(E_NOTIMPL.into());
    }
    type Runtime = unsafe extern "system" fn(*mut c_void) -> *const c_char;
    // SAFETY: The object installed through slot 10 lives through the call. Its
    // first virtual method returns a NUL-terminated runtime module.
    let table = unsafe { *driver.cast::<*const Runtime>() };
    let source = unsafe { (*table)(driver) };
    if source.is_null() {
        return Err(E_POINTER.into());
    }
    let runtime = unsafe { CStr::from_ptr(source) }
        .to_str()
        .map_err(|_| E_INVALIDARG)?;
    let state_id_is_u32 = runtime.lines().any(|line| {
        line.strip_prefix("define void @__xdxr_impl_XDXR_SetStateId(%struct.RuntimeDataStruct* ")
            .and_then(|args| args.split_once(','))
            .is_some_and(|(_, id)| id.trim_start().starts_with("i32 "))
    });
    if !state_id_is_u32 || runtime.contains("!dx.entryPoints") {
        return Err(E_NOTIMPL.into());
    }
    Ok(runtime.to_owned())
}

pub(super) unsafe fn compile(
    compiler: &Compiler,
    libraries: &[Bytecode],
    names: &[PCWSTR],
    info: *mut ShaderInfo,
) -> Result<IDxcOperationResult> {
    let mut ir = unsafe { driver_module(compiler)? };
    let utils: IDxcUtils = unsafe { DxcCreateInstance2(&compiler.allocator, &CLSID_DxcUtils)? };
    let frontend: IDxcCompiler =
        unsafe { DxcCreateInstance2(&compiler.allocator, &CLSID_DxcCompiler)? };
    let mut inputs = Vec::with_capacity(libraries.len());
    for code in libraries {
        if code.data.is_null() {
            return Err(E_POINTER.into());
        }
        let blob = unsafe { utils.CreateBlob(code.data, code.len, DXC_CP_ACP)? };
        let listing = unsafe { frontend.Disassemble(&blob)? };
        inputs.push(listing);
    }
    let mut selected = vec![false; inputs.len()];
    let mut leaves = Vec::with_capacity(names.len());
    for name in names {
        if name.is_null() {
            return Err(E_POINTER.into());
        }
        let name = unsafe { name.to_string() }.map_err(|_| E_INVALIDARG)?;
        let mut found = None;
        for (index, blob) in inputs.iter().enumerate() {
            let bytes = unsafe {
                core::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize())
            };
            let listing = core::str::from_utf8(bytes).map_err(|_| E_INVALIDARG)?;
            if let Ok(leaf) = Leaf::read(listing, &name) {
                if found.is_some() {
                    return Err(E_INVALIDARG.into());
                }
                selected[index] = true;
                found = Some(leaf);
            }
        }
        let leaf = found.ok_or(E_NOTIMPL)?;
        leaf.append(&mut ir, leaves.len());
        leaves.push(leaf);
    }
    // Mark each generated state as a separately callable library export with
    // its original ray payload size and its Series shader-kind annotation.
    ir.push_str(
        r#"
!dx.version = !{!0}
!dx.valver = !{!1}
!dx.shaderModel = !{!2}
!dx.entryPoints = !{!3"#,
    );
    for i in 0..leaves.len() {
        write!(ir, ", !{}", 6 + 2 * i).unwrap();
    }
    ir.push_str(
        r#"}
!0 = !{i32 1, i32 3}
!1 = !{i32 1, i32 7}
!2 = !{!"lib", i32 6, i32 3}
!3 = !{null, !"", null, null, !4}
!4 = !{i32 0, i64 1048576}
!5 = !{i32 0}
"#,
    );
    for (i, leaf) in leaves.iter().enumerate() {
        let id = 6 + 2 * i;
        let name = &leaf.name;
        writeln!(ir,r#"!{id} = !{{void (%struct.RuntimeDataStruct*, [256 x i32]*)* @{name}.ss_0, !"{name}.ss_0", null, null, !{}}}
!{} = !{{i32 8, i32 11, i32 6, i32 {}, i32 5, !5}}"#, id+1, id+1, leaf.words * 4).unwrap();
    }
    let size = ir.len().try_into().map_err(|_| E_INVALIDARG)?;
    let source = unsafe { utils.CreateBlob(ir.as_ptr().cast(), size, DXC_CP_UTF8)? };
    let assembler: IDxcAssembler =
        unsafe { DxcCreateInstance2(&compiler.allocator, &CLSID_DxcAssembler)? };
    let mut result = unsafe { assembler.AssembleToContainer(&source)? };
    if unsafe { result.GetStatus()? }.is_err() {
        return Ok(result);
    }
    let mut support: Vec<_> = libraries
        .iter()
        .zip(&selected)
        .filter_map(|(lib, used)| (!used).then_some(*lib))
        .collect();
    if !support.is_empty() {
        // The UMD contributes traversal helpers and a scheduling entry as ordinary
        // library inputs. Preserve their actual implementations in the collection.
        let collection = unsafe { result.GetResult()? };
        support.push(Bytecode {
            data: unsafe { collection.GetBufferPointer() },
            len: unsafe { collection.GetBufferSize() }
                .try_into()
                .map_err(|_| E_INVALIDARG)?,
        });
        result = unsafe { super::rename_libraries(&compiler.allocator, &support, &[])? };
    }
    if !info.is_null() && unsafe { result.GetStatus()? }.is_ok() {
        for i in 0..leaves.len() {
            // The only stack word in these leaf states is their caller's return ID.
            unsafe {
                info.add(i).write(ShaderInfo {
                    state: 0,
                    stack_bytes: 4,
                });
            }
        }
    }
    Ok(result)
}
