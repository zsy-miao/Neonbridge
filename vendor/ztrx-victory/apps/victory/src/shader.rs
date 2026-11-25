//! Survey shader containers: bitcode, console additions and native code.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use victory_game::sgd5;
use victory_shader::gfx10::{self, Instruction};
use victory_shader::{Container, DxilProgram, FourCC, HwStage, ShaderError, native_programs};

/// Files above this size are skipped (read whole into memory for now).
pub(crate) const MAX_FILE: u64 = 2 << 30;

/// A shader container in a file: as is, or packed in an SGD5 record.
pub(crate) enum Source<'a> {
    Raw { offset: usize, data: &'a [u8] },
    Packed(sgd5::Record<'a>),
}

impl Source<'_> {
    /// The container bytes; `None` if a packed record is corrupt.
    pub(crate) fn bytes(&self) -> Option<Cow<'_, [u8]>> {
        match self {
            Source::Raw { data, .. } => Some(Cow::Borrowed(data)),
            Source::Packed(r) => r.unpack().map(Cow::Owned),
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Source::Raw { offset, .. } => format!("@{offset:#x}"),
            Source::Packed(r) => format!("@{:#x} {}", r.offset, r.name.unwrap_or("?")),
        }
    }
}

/// The packed records of SGD5 data, and every container stored as is (the
/// executable keeps some records uncompressed), found by its magic.
pub(crate) fn sources(data: &[u8]) -> Vec<Source<'_>> {
    let mut out: Vec<Source<'_>> = Vec::new();
    if sgd5::is_sgd5(data) {
        out.extend(sgd5::records(data).map(Source::Packed));
    }
    let raw = memchr::memmem::find_iter(data, b"DXBC")
        .filter(|&offset| Container::parse(&data[offset..]).is_ok())
        .map(|offset| Source::Raw {
            offset,
            data: &data[offset..],
        });
    out.extend(raw);
    out
}

#[derive(Default)]
struct Totals {
    containers: usize,
    with_dxil_part: usize,
    with_bitcode: usize,
    standard_layout: usize,
    xdat: usize,
    xbop: usize,
    kinds: BTreeMap<String, usize>,
    with_native: usize,
    native_stages: BTreeMap<HwStage, usize>,
    native_errors: BTreeMap<String, usize>,
    decoded: usize,
    instructions: usize,
    unnamed: BTreeMap<String, usize>,
    decode_errors: BTreeMap<&'static str, usize>,
    files_with_shaders: usize,
}

pub fn run(paths: Vec<PathBuf>, verbose: bool, disasm: bool) -> Result<()> {
    let paths = crate::default_paths(paths)?;
    let mut t = Totals::default();
    for root in &paths {
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if entry.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE {
                eprintln!("skipping large file {}", path.display());
                continue;
            }
            let Ok(data) = std::fs::read(path) else {
                continue;
            };
            let mut found_here = 0;
            for source in sources(&data) {
                let Some(bytes) = source.bytes() else {
                    eprintln!("corrupt record {}{}", path.display(), source.label());
                    continue;
                };
                let Ok(container) = Container::parse(&bytes) else {
                    continue;
                };
                found_here += 1;
                t.containers += 1;
                let dxil = container.part(FourCC::DXIL);
                t.with_dxil_part += usize::from(dxil.is_some());
                let program = dxil.and_then(|p| DxilProgram::parse(p.data).ok());
                let mut listing = Vec::new();
                // Independent of the bitcode: titles may strip it.
                let native = match dxil.map(|p| native_programs(p.data)) {
                    Some(Ok(programs)) if !programs.is_empty() => {
                        t.with_native += 1;
                        let mut list = Vec::new();
                        for n in &programs {
                            *t.native_stages.entry(n.stage).or_default() += 1;
                            list.push(format!("{:?} {} B", n.stage, n.code.len()));
                            match gfx10::decode(&n.code) {
                                Ok(insts) => {
                                    t.decoded += 1;
                                    t.instructions += insts.len();
                                    for i in insts.iter().filter(|i| i.name().is_none()) {
                                        let key = format!("{:?} {:#x}", i.encoding, i.opcode);
                                        *t.unnamed.entry(key).or_default() += 1;
                                    }
                                    if disasm {
                                        listing
                                            .extend(n.layout.iter().map(|p| format!("    {p:?}")));
                                        listing.extend(n.used.iter().map(|u| format!("    {u:?}")));
                                        listing.extend(
                                            insts.iter().map(|i| format_instruction(i, n.wave64)),
                                        );
                                    }
                                }
                                Err(e) => {
                                    let reason = match e {
                                        ShaderError::Isa { reason, .. } => reason,
                                        _ => "other",
                                    };
                                    *t.decode_errors.entry(reason).or_default() += 1;
                                    list.push(format!("({e})"));
                                }
                            }
                        }
                        format!(", native {}", list.join(" "))
                    }
                    Some(Err(e)) => {
                        *t.native_errors.entry(e.to_string()).or_default() += 1;
                        format!(", native: {e}")
                    }
                    _ => String::new(),
                };
                let line = match &program {
                    Some(p) => {
                        t.with_bitcode += 1;
                        let x = p.xbox_extras();
                        t.standard_layout += usize::from(p.is_standard_layout());
                        t.xdat += usize::from(x.xdat);
                        t.xbop += usize::from(x.xbop);
                        *t.kinds.entry(format!("{:?}", p.kind)).or_default() += 1;
                        format!(
                            "{:?} sm{}.{} bitcode {} B, xdat {}, xbop {}",
                            p.kind,
                            p.shader_model.0,
                            p.shader_model.1,
                            p.bitcode.len(),
                            x.xdat,
                            x.xbop
                        )
                    }
                    None => "no parsable DXIL program".to_owned(),
                } + &native;
                if verbose {
                    println!("{}{}: {line}", path.display(), source.label());
                    for l in &listing {
                        println!("{l}");
                    }
                }
            }
            t.files_with_shaders += usize::from(found_here > 0);
        }
    }
    println!(
        "containers found       {} (in {} files)",
        t.containers, t.files_with_shaders
    );
    println!("  with DXIL part       {}", t.with_dxil_part);
    println!("  with LLVM bitcode    {}", t.with_bitcode);
    println!("  PC-standard layout   {}", t.standard_layout);
    println!("  XDAT prefix          {}", t.xdat);
    println!("  xbop trailer         {}", t.xbop);
    for (k, n) in &t.kinds {
        println!("  {k:<20} {n}");
    }
    println!("  with native code     {}", t.with_native);
    for (stage, n) in &t.native_stages {
        println!("    {:<18} {n}", format!("{stage:?}"));
    }
    for (e, n) in &t.native_errors {
        println!("  native unreadable    {n}: {e}");
    }
    if t.with_native > 0 {
        println!(
            "  decoded programs     {} ({} instructions)",
            t.decoded, t.instructions
        );
        for (e, n) in &t.decode_errors {
            println!("  decode failed        {n}: {e}");
        }
        let mut unnamed: Vec<_> = t.unnamed.iter().collect();
        unnamed.sort_by_key(|&(_, n)| std::cmp::Reverse(*n));
        for (op, n) in unnamed.iter().take(10) {
            println!("  opcode not in table  {op} x{n}");
        }
    }
    if t.containers == 0 {
        println!("(none found: shaders may sit in compressed or encrypted archives)");
    }
    Ok(())
}

fn format_instruction(i: &Instruction, wave64: bool) -> String {
    let words: Vec<_> = i.words.iter().map(|w| format!("{w:08X}")).collect();
    let text = match i.name() {
        Some(_) => i.text(wave64),
        None => format!("<{:?} {:#x}>", i.encoding, i.opcode),
    };
    format!("    {:06x}: {:<44} {text}", i.offset, words.join(" "))
}
