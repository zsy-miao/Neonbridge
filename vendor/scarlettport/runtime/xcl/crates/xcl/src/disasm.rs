//! `xcl disasm <file> [--offset N] [--length N] [--all] [--hex] [--fssm FILE]` – RDNA2
//! disassembler for captured shader blobs (or any binary with console GPU code).
//! Console containers (XDAT/XBCH/FSSM) are unpacked and their layout is printed.

use std::path::Path;

use crate::gpu::{rdna2, shader, xbox};

fn parse_num(s: &str) -> Option<usize> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => usize::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

fn number_option(args: &[String], name: &str) -> Result<Option<usize>, String> {
    let Some(index) = args.iter().position(|arg| arg == name) else { return Ok(None) };
    let value = args.get(index + 1).ok_or_else(|| format!("{name} requires a byte count"))?;
    parse_num(value).map(Some).ok_or_else(|| format!("invalid value for {name}: {value}"))
}

fn print_lines(code: &[u8], base: usize, stop: bool) {
    for l in rdna2::disassemble(code, base as u64, stop) {
        let hex: Vec<String> = l.bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mark = if l.flags & rdna2::F_BRANCH != 0 { "  ; →" } else { "" };
        println!("  {:08x}: {:<52} {}{mark}", l.pc, l.text, hex.join(" "));
    }
}

fn hexdump(data: &[u8], base: usize) {
    for (i, row) in data.chunks(16).enumerate() {
        let hex: Vec<String> = row.iter().map(|b| format!("{b:02x}")).collect();
        let txt: String = row.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect();
        println!("    {:06x}: {:<47}  {txt}", base + i * 16, hex.join(" "));
    }
}

fn xdat_label(id: u32) -> &'static str {
    match id {
        xbox::XDAT_HASH => "hash",
        xbox::XDAT_XBCH => "XBCH → FSSM (native code)",
        xbox::XDAT_NESTED_HEADER => "nested DXIL header (NOBC)",
        _ => "?",
    }
}

fn print_xbox(sh: &xbox::XboxShader, full_hex: bool, fssm_out: Option<&str>) {
    println!(
        "console container: stage {} (DXIL), SM {}.{}, XDAT @ +{:#x}",
        sh.stage, sh.shader_model.0, sh.shader_model.1, sh.xdat_offset
    );
    for e in &sh.entries {
        println!("  XDAT {:#04x}  +{:#x}  {} B  {}", e.id, e.offset, e.size, xdat_label(e.id));
    }
    if let Some(h) = sh.hash {
        println!("  hash: {}", h.iter().map(|b| format!("{b:02x}")).collect::<String>());
    }
    for n in &sh.notes {
        println!("  NOTE: {n}");
    }
    for (i, f) in sh.fssm.iter().enumerate() {
        println!(
            "FSSM #{i}: XBCH v{} @ +{:#x}, zlib {} B → {} B, FSSM v{:#x}",
            f.xbch_version,
            f.xbch_offset,
            f.compressed,
            f.data.len(),
            f.version
        );
        if let Some(e) = &f.error {
            println!("  WARNING: {e}");
        }
        for s in &f.stages {
            println!("  hardware stage: {} – descriptor @ +{:#x}", s.label(), s.offset);
        }
        match f.code {
            Some((o, l)) => println!(
                "  code: +{o:#x}, {l} B ({}), programs: {}",
                if f.code_from_header { "pointer from header +0xC0" } else { "found by scanning" },
                f.programs.len()
            ),
            None => println!("  code: not found"),
        }
        let limit = if full_hex { usize::MAX } else { 0x140 };
        println!("  header – non-zero qwords (without the 0xDEADBEEF filler){}:", if full_hex { "" } else { ", up to +0x140; --hex: all" });
        for (o, q) in xbox::header_qwords(f, limit) {
            println!("    +{o:#05x}: {q:016x}");
        }
        if full_hex {
            if let Some((o, l)) = f.code {
                println!("  data after the code (+{:#x}):", o + l);
                hexdump(&f.data[o + l..], o + l);
            }
        }
        if let Some(out) = fssm_out {
            let name = if i == 0 { out.to_string() } else { format!("{out}.{i}") };
            match std::fs::write(&name, &f.data) {
                Ok(()) => println!("  saved FSSM to {name}"),
                Err(e) => eprintln!("xcl disasm: {name}: {e}"),
            }
        }
    }
}

pub fn run(path: &Path, args: &[String]) -> i32 {
    let (offset, length) = match (number_option(args, "--offset"), number_option(args, "--length")) {
        (Ok(offset), Ok(length)) => (offset, length),
        (Err(error), _) | (_, Err(error)) => {
            eprintln!("xcl disasm: {error}");
            return 2;
        }
    };
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("xcl disasm: {}: {e}", path.display());
            return 2;
        }
    };
    let all = args.iter().any(|a| a == "--all");
    println!("{} – {} B, decoder: {}", path.display(), data.len(), crate::gpu::RDNA2_ARCH);

    if offset.is_some() || length.is_some() {
        let off = offset.unwrap_or(0);
        if off >= data.len() {
            eprintln!("xcl disasm: offset {off:#x} is past the end of the file");
            return 2;
        }
        let len = length.unwrap_or(data.len() - off).min(data.len() - off);
        print_lines(&data[off..off + len], off, !all);
        return 0;
    }

    let (kind, parts) = shader::classify(&data);
    println!("format: {}", kind.label());
    if !parts.is_empty() {
        println!("container parts:");
        for p in &parts {
            println!("  {}  +{:#x}  {} B", p.fourcc, p.offset, p.size);
        }
    }
    if kind == shader::Kind::XboxNative {
        if let Some(sh) = xbox::parse(&data) {
            let out = args.iter().position(|a| a == "--fssm").and_then(|i| args.get(i + 1)).map(String::as_str);
            print_xbox(&sh, args.iter().any(|a| a == "--hex"), out);
        }
    }
    let listing = shader::rdna2_listing(&data);
    if listing.is_empty() {
        println!("no RDNA2 programs found (ending in s_endpgm); use --offset N to force decoding");
        if matches!(kind, shader::Kind::Dxil | shader::Kind::Dxbc) {
            println!("(this is a PC-format shader – no recompilation needed)");
        }
    } else {
        print!("{listing}");
    }
    0
}
