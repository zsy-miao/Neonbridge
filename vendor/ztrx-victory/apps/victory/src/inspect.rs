use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use victory_pe::implib::ImportLibrary;
use victory_pe::{ImageInfo, layout};

pub fn run(file: &Path, list_imports: bool, list_exports: bool) -> Result<()> {
    let data = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let info = ImageInfo::parse(&data).with_context(|| format!("parsing {}", file.display()))?;

    println!("file           {}", file.display());
    println!("blake3         {}", blake3::hash(&data).to_hex());
    if let Some(cv) = &info.codeview {
        println!("pdb            {}  (key {})", cv.pdb_path, cv.symbol_key());
    }
    println!(
        "kind           {}",
        if info.is_dll() { "dll" } else { "exe" }
    );
    println!(
        "subsystem      {} ({})",
        info.subsystem,
        info.subsystem_name()
    );
    println!(
        "image base     {:#x}  size {:#x}",
        info.image_base, info.size_of_image
    );
    println!("entry rva      {:#x}", info.entry_rva);
    println!(
        "stack          reserve {:#x} commit {:#x}",
        info.stack_reserve, info.stack_commit
    );
    println!(
        "dll flags      {:#06x}{}",
        info.dll_characteristics,
        dll_flags(info.dll_characteristics)
    );
    println!(
        "relocations    {}   exception table {}   load config {}",
        yes(info.has_relocations),
        yes(info.has_exception_table),
        yes(info.has_load_config)
    );
    if let Some(tls) = &info.tls {
        println!("tls            {} callback(s)", tls.callback_rvas.len());
    }

    println!("\nsections");
    for s in &info.sections {
        println!(
            "  {:<8} rva {:#010x} vsize {:#010x} raw {:#010x} flags {:#010x}",
            s.name.trim_end_matches('\0'),
            s.rva,
            s.virtual_size,
            s.file_size,
            s.characteristics
        );
    }

    println!(
        "\nimports        {} functions from {} DLLs",
        info.import_count(),
        info.imports.len()
    );
    for m in &info.imports {
        println!("  {:<48} {:>5}", m.dll, m.entries.len());
        if list_imports {
            for e in &m.entries {
                println!("      {}", e.symbol);
            }
        }
    }
    if !info.delay_imports.is_empty() {
        println!("\ndelay-load imports");
        for m in &info.delay_imports {
            println!("  {:<48} {:>5}", m.dll, m.entries.len());
            if list_imports {
                for e in &m.entries {
                    println!("      {}", e.symbol);
                }
            }
        }
    }

    println!("\nexports        {}", info.exports.len());
    if list_exports {
        for e in &info.exports {
            let name = e.name.as_deref().unwrap_or("(by ordinal)");
            match (&e.rva, &e.forward) {
                (_, Some(fwd)) => println!("  {:>5}  {name} -> {fwd}", e.ordinal),
                (Some(rva), _) => println!("  {:>5}  {name}  {rva:#x}", e.ordinal),
                _ => println!("  {:>5}  {name}", e.ordinal),
            }
        }
    }
    Ok(())
}

pub fn lib_exports(lib: &Path, list: bool) -> Result<()> {
    let data = std::fs::read(lib).with_context(|| format!("reading {}", lib.display()))?;
    let parsed =
        ImportLibrary::parse(&data).with_context(|| format!("parsing {}", lib.display()))?;
    let mut by_dll: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for i in &parsed.imports {
        by_dll
            .entry(i.dll.to_ascii_lowercase())
            .or_default()
            .push(i.export.to_string());
    }
    println!(
        "{} exports from {} DLLs ({} other members)",
        parsed.imports.len(),
        by_dll.len(),
        parsed.other_members
    );
    for (dll, names) in &by_dll {
        println!("{dll:<60} {:>5}", names.len());
        if list {
            for n in names {
                println!("    {n}");
            }
        }
    }
    Ok(())
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn dll_flags(v: u16) -> String {
    use layout::dll_flags::*;
    let names = [
        (HIGH_ENTROPY_VA, "high-entropy-va"),
        (DYNAMIC_BASE, "dynamic-base"),
        (FORCE_INTEGRITY, "force-integrity"),
        (NX_COMPAT, "nx"),
        (APPCONTAINER, "appcontainer"),
        (GUARD_CF, "cfg"),
    ];
    let set: Vec<_> = names
        .iter()
        .filter(|(f, _)| v & f != 0)
        .map(|(_, n)| *n)
        .collect();
    if set.is_empty() {
        String::new()
    } else {
        format!("  ({})", set.join(", "))
    }
}
