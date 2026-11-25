//! `xcl inspect <exe>` – static analysis without running anything (read-only).

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::pe::{Layout, Pe};
use crate::resolver::{self, Source};

#[derive(Serialize)]
struct Entry {
    dll: String,
    function: String,
    delay: bool,
    status: String,
}

#[derive(Serialize)]
struct Out {
    file: String,
    machine: String,
    image_base: String,
    entry_rva: String,
    size_of_image: String,
    subsystem: u16,
    is_dll: bool,
    has_relocations: bool,
    tls_template_bytes: Option<u64>,
    exception_entries: u32,
    stack_reserve: String,
    imports: Vec<Entry>,
}

/// Console-only DLLs a PE imports (kernelx.dll, xgameruntime*, d3d12_xs …).
/// `None` when the file is not a valid x64 PE. A plain Windows program yields `Some([])`.
pub fn console_imports(path: &Path) -> Option<Vec<String>> {
    let data = std::fs::read(path).ok()?;
    let pe = Pe::parse(&data, Layout::File).ok()?;
    let mut all = pe.imports().unwrap_or_default();
    all.extend(pe.delay_imports().unwrap_or_default());
    let reg = crate::hle::registry();
    let mut out: Vec<String> = Vec::new();
    for d in &all {
        let key = crate::util::dll_key(&d.dll);
        let console = key == crate::hle::KERNELX
            || reg
                .hle_for(&key)
                .map(|m| m.name == crate::hle::XGAMERUNTIME || m.name == crate::hle::D3D12X)
                .unwrap_or(false);
        if console && !out.contains(&key) {
            out.push(key);
        }
    }
    Some(out)
}

fn status(s: &Source) -> String {
    match s {
        Source::Hle(m) => format!("HLE ({m})"),
        Source::HleOther(m) => format!("HLE ({m}, other module)"),
        Source::System(d) => format!("Windows ({d})"),
        Source::GameDll(d) => format!("game DLL ({d})"),
        Source::Stub => "STUB".into(),
    }
}

pub fn run(path: &Path, json: bool) -> i32 {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("xcl inspect: cannot read {}: {e}", path.display());
            return 2;
        }
    };
    let pe = match Pe::parse(&data, Layout::File) {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "xcl inspect: {}: {e}\n(If this is a file from an XVC/MSIXVC package – XCL only works on already unpacked files.)",
                path.display()
            );
            return 3;
        }
    };
    if let Some(dir) = path.parent() {
        resolver::set_search_dirs(vec![dir.to_path_buf()]);
    }
    let mut imports = Vec::new();
    let mut all = pe.imports().unwrap_or_default();
    all.extend(pe.delay_imports().unwrap_or_default());
    for d in &all {
        for e in &d.entries {
            let s = resolver::classify(&d.dll, &e.what);
            imports.push(Entry { dll: d.dll.clone(), function: e.what.to_string(), delay: d.delay, status: status(&s) });
        }
    }
    let tls = pe.tls().ok().flatten().map(|t| t.total_len());
    let ex = pe.dir(crate::pe::DIR_EXCEPTION);
    let out = Out {
        file: path.to_string_lossy().into_owned(),
        machine: format!("{:#06x}", pe.machine),
        image_base: format!("{:#x}", pe.image_base),
        entry_rva: format!("{:#x}", pe.entry_rva),
        size_of_image: format!("{:#x}", pe.size_of_image),
        subsystem: pe.subsystem,
        is_dll: pe.is_dll(),
        has_relocations: pe.has_relocs(),
        tls_template_bytes: tls,
        exception_entries: ex.size / 12,
        stack_reserve: format!("{:#x}", pe.stack_reserve),
        imports,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return 0;
    }
    println!("File:          {}", out.file);
    println!("Type:          {} x64, subsystem {}", if out.is_dll { "DLL" } else { "EXE" }, out.subsystem);
    println!("ImageBase:     {}  (size {}, entry +{})", out.image_base, out.size_of_image, out.entry_rva);
    println!("Relocations:   {}", if out.has_relocations { "yes" } else { "NO (the image must be mapped at ImageBase)" });
    println!("Static TLS:    {}", tls.map(|n| format!("{n} B (handled by the TLS carrier)")).unwrap_or_else(|| "none".into()));
    println!("Exceptions:    {} .pdata entries", out.exception_entries);
    println!("Stack:         {}", out.stack_reserve);
    match console_imports(path) {
        Some(c) if !c.is_empty() => println!("Console game:  yes (imports {})", c.join(", ")),
        _ => println!("Console game:  NO – no kernelx / xgameruntime / d3d12_xs imports (a regular Windows program?)"),
    }
    let mut per_dll: BTreeMap<String, Vec<&Entry>> = BTreeMap::new();
    for e in &out.imports {
        per_dll.entry(format!("{}{}", e.dll, if e.delay { " (delay-load)" } else { "" })).or_default().push(e);
    }
    let total_stub = out.imports.iter().filter(|e| e.status == "STUB").count();
    println!("\nImports: {} functions from {} DLLs, {} of them unimplemented (stubs):", out.imports.len(), per_dll.len(), total_stub);
    for (dll, entries) in &per_dll {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for e in entries {
            let k = if e.status.starts_with("HLE") {
                "HLE"
            } else if e.status.starts_with("Windows") {
                "Windows"
            } else if e.status.starts_with("game DLL") {
                "game DLL"
            } else {
                "stubs"
            };
            *counts.entry(k).or_default() += 1;
        }
        let c: Vec<String> = counts.iter().map(|(k, v)| format!("{v} {k}")).collect();
        println!("  {dll}: {}", c.join(", "));
        for e in entries.iter().filter(|e| e.status == "STUB") {
            println!("      - {}", e.function);
        }
    }
    0
}
