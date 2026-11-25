//! Translate a cooked shader, optionally linking its outputs to a pixel shader.
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use tuxbox_xshader::{extract, ShaderExtract};
#[cfg(not(feature = "legacy-hlsl"))]
use tuxbox_xshader::Stage;

// Used only for deterministic refusals by the HLSL emitter.
const UNSUPPORTED_HLSL: u8 = 65;

enum Error {
    Other(String),
    Unsupported(String),
}

impl From<String> for Error {
    fn from(message: String) -> Self { Self::Other(message) }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self { Self::Other(message.into()) }
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The command line's translation: [`tuxbox_xshader::translate_request`],
/// the one copy the launcher also calls in-process. Returns the HLSL and the
/// DXC profile.
#[cfg(feature = "legacy-hlsl")]
fn translate(
    source: &[u8],
    _shader: &ShaderExtract,
    consumer: Option<&Path>,
    pixel_order: Option<&Path>,
    stencil_target: Option<u32>,
) -> Result<(String, String), Error> {
    let consumer = consumer.map(read).transpose()?;
    let order = pixel_order.map(read).transpose()?;
    match tuxbox_xshader::translate_request(source, consumer.as_deref(), order.as_deref(), stencil_target) {
        Ok((hlsl, profile)) => Ok((String::from_utf8(hlsl).map_err(|e| e.to_string())?, profile)),
        Err((UNSUPPORTED_HLSL, m)) => Err(Error::Unsupported(m)),
        Err((_, m)) => Err(Error::Other(m)),
    }
}

#[cfg(not(feature = "legacy-hlsl"))]
fn translate(
    _source: &[u8],
    shader: &ShaderExtract,
    consumer: Option<&Path>,
    _pixel_order: Option<&Path>,
    stencil_target: Option<u32>,
) -> Result<(String, &'static str), Error> {
    if consumer.is_some() {
        return Err(Error::Unsupported("independent HLSL emitter does not support linked shaders".into()));
    }
    if stencil_target.is_some() {
        return Err(Error::Unsupported("independent HLSL emitter does not support stencil exports".into()));
    }
    let hlsl = tuxbox_xshader::independent_hlsl(shader).ok_or_else(|| {
        format!(
            "unsupported {:?} program for the independent HLSL emitter",
            shader.stage
        )
    }).map_err(Error::Unsupported)?;
    let model = if tuxbox_xshader::requires_inline_ray_query(shader) { "6_5" } else { "6_0" };
    Ok((hlsl, format!("{}_{model}", stage_prefix(shader.stage)?)))
}

#[cfg(not(feature = "legacy-hlsl"))]
fn stage_prefix(stage: Stage) -> Result<&'static str, Error> {
    Ok(match stage {
        Stage::Pixel => "ps",
        Stage::Vertex => "vs",
        Stage::Geometry => "gs",
        Stage::Compute => "cs",
        Stage::Mesh => "ms",
        _ => return Err("unsupported DXC stage".into()),
    })
}

fn run() -> Result<(), Error> {
    let mut args = env::args_os().skip(1);
    let source = args.next().ok_or("expected a cooked shader path")?;
    if source == "--disasm" {
        // Translator development: list the program's instructions.
        let path = args.next().ok_or("--disasm requires a cooked shader path")?;
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.to_string_lossy()))?;
        let ex = tuxbox_xshader::extract(&bytes)?;
        println!("stage {:?} features {:?} root {:#?}", ex.stage, ex.required_features, ex.root_sig);
        let f = &ex.fssm;
        let w = |o: usize| f.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        println!("fssm {} bytes, magic {:?}, size field {:?} (records need len-16), rows at {:?}/{:?} count {:?}",
            f.len(), f.get(..8), w(8), w(0xf0), w(0xf4), w(0xfc));
        if std::env::var_os("TB_DISASM_HEADER").is_some() {
            let words: Vec<String> = (0..0x40).filter_map(|i| w(i * 4).map(|v| format!("{:#x}:{v:#x}", i * 4))).collect();
            println!("fssm header {}", words.join(" "));
        }
        if let Some(rows) = tuxbox_dxbc::compute_table::records(&ex.fssm) {
            for r in rows { println!("fssm resource {:#x} pos {} reg {} {:#x} loc {:#x}", r[0], r[1], r[2], r[3], r[4]); }
        }
        if let Some(rows) = tuxbox_dxbc::compute_table::user_data_records(&ex.fssm) {
            for r in rows { println!("fssm user {:#x} {} {} {:#x} {:#x}", r[0], r[1], r[2], r[3], r[4]); }
        }
        for r in &ex.resources { println!("psv type {} space {} regs {}..={}", r.res_type, r.space, r.lower, r.upper); }
        for i in tuxbox_xshader::rdna2::disassemble(&ex.isa) {
            println!("{:#06x} {:<28} {:?} <- {:?} imm={} {:08x?}", i.offset, i.mnemonic, i.dst, i.src, i.aux.imm, i.raw);
        }
        return Ok(());
    }
    if source == "--refusals" {
        let dir = PathBuf::from(args.next().ok_or("--refusals requires a request directory")?);
        let mut json = false;
        for extra in args {
            match extra.to_str() {
                Some("--json") => json = true,
                _ => return Err(format!("unknown option {extra:?}").into()),
            }
        }
        return refusal_census(&dir, json).map_err(Error::Other);
    }
    if source == "--list-emitters" {
        if args.next().is_some() {
            return Err("--list-emitters takes no arguments".into());
        }
        println!("independent");
        #[cfg(feature = "legacy-hlsl")]
        println!("legacy");
        return Ok(());
    }
    let mut output = None;
    let mut profile_output = None;
    let mut consumer = None;
    let mut pixel_order = None;
    let mut stencil_target = None;
    let mut quad_mask = false;
    while let Some(argument) = args.next() {
        if argument == "--quad-mask" {
            quad_mask = true;
            continue;
        }
        if argument == "--stencil-target" {
            let index: u32 = args
                .next()
                .and_then(|v| v.to_str()?.parse().ok())
                .ok_or("--stencil-target requires a render target index")?;
            if stencil_target.replace(index).is_some() {
                return Err("duplicate --stencil-target".into());
            }
            continue;
        }
        let slot = match argument.to_str() {
            Some("--hlsl-out") => &mut output,
            Some("--profile-out") => &mut profile_output,
            Some("--link-pixel") => &mut consumer,
            Some("--pixel-order") => &mut pixel_order,
            _ => return Err(format!("unknown option {argument:?}").into()),
        };
        if slot
            .replace(PathBuf::from(args.next().ok_or("missing path")?))
            .is_some()
        {
            return Err(format!("duplicate option {argument:?}").into());
        }
    }
    let output = output.ok_or("--hlsl-out is required")?;
    if consumer.is_some() && stencil_target.is_some() {
        return Err("a stencil export requires an unlinked pixel shader".into());
    }
    if pixel_order.is_some() && consumer.is_none() {
        return Err("--pixel-order requires --link-pixel".into());
    }
    let source_bytes = read(source.as_ref())?;
    if source_bytes.starts_with(&2_u32.to_le_bytes()) {
        if consumer.is_some() || stencil_target.is_some() {
            return Err("raster linking options cannot be applied to a ray library".into());
        }
        let (name, library) = tuxbox_xshader::ray::library(&source_bytes)?;
        fs::write(&output, library).map_err(|e| format!("{}: {e}", output.display()))?;
        if let Some(path) = profile_output {
            fs::write(&path, "lib_6_3").map_err(|e| format!("{}: {e}", path.display()))?;
        }
        println!("entry={name}");
        return Ok(());
    }
    let shader = extract(&source_bytes)?;
    let run = || translate(
        &source_bytes,
        &shader,
        consumer.as_deref(),
        pixel_order.as_deref(),
        stencil_target,
    );
    // A quad-mask variant (`hlsl::with_quad_mask`) of an unlinked pixel shader.
    let (hlsl, profile) = if quad_mask { tuxbox_xshader::hlsl::with_quad_mask(run)? } else { run()? };
    if let Some(path) = profile_output {
        fs::write(&path, profile).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let mut instructions = 0;
    let mut consumed = 0;
    let mut ended = false;
    for ins in tuxbox_xshader::rdna2::instructions(&shader.isa) {
        instructions += 1;
        consumed += ins.dwords * 4;
        ended |= ins.mnemonic == "s_endpgm";
    }
    println!("entry={:?}", shader.entry);
    println!(
        "decode: {instructions} instrs, consumed {consumed}/{} bytes, terminated_on_s_endpgm={ended}",
        shader.isa.len()
    );
    fs::write(&output, hlsl).map_err(|e| format!("{}: {e}", output.display()))?;
    // Heap access reflection for the runtime (hlsl/heap_reflect.rs).
    if let Some(heap) = tuxbox_xshader::hlsl::heap_reflect::take() {
        let path = format!("{}.heap", output.display());
        fs::write(&path, heap).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(())
}

/// Reasons with offsets, immediates and register numbers replaced by `#`, so
/// that one refusal site counts once however many programs reach it. Cause
/// numbers written as plain words (a format ID, a dimension) stay visible.
fn normalize_refusal(reason: &str) -> String {
    let first = reason.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let b = first.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let boundary = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        if boundary && b[i] == b'0' && b.get(i + 1) == Some(&b'x') {
            let mut j = i + 2;
            while j < b.len() && b[j].is_ascii_hexdigit() { j += 1; }
            out.push('#');
            i = j;
            continue;
        }
        if boundary && matches!(b[i], b's' | b'v' | b't' | b'u' | b'b') && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() { j += 1; }
            if j == b.len() || !(b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                out.push(b[i] as char);
                out.push('#');
                i = j;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn json_string(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Read-only census of translator refusal sites over a request directory (the
/// idea of Xbox2Windows 5510deca/b4d1cf01/425cd53b, avt0m4t and contributors,
/// MIT). Every file is translated in memory, without dxc, and nothing is
/// written. xshader stops at the first refusal, so each program counts its
/// first site only: fixing one can expose the next. A program that takes over
/// TB_REFUSALS_TIMEOUT seconds (default 60) counts as `timeout`.
fn refusal_census(dir: &Path, json: bool) -> Result<(), String> {
    use std::collections::BTreeMap;
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    let limit = std::time::Duration::from_secs(
        env::var("TB_REFUSALS_TIMEOUT").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(60),
    );
    // (outcome, stage, reason) -> programs
    let mut rows: BTreeMap<(&'static str, String, String), u32> = BTreeMap::new();
    let (mut ok, mut total) = (0u32, 0u32);
    // Translated programs with a scalar op mixing a lane mask and a number (xcl 438d420).
    let mut mixed = 0u32;
    for path in &files {
        let Ok(bytes) = fs::read(path) else { continue };
        total += 1;
        if bytes.starts_with(&2_u32.to_le_bytes()) {
            ok += 1; // ray library: not translated by the HLSL emitter
            continue;
        }
        let shader = match extract(&bytes) {
            Ok(s) => s,
            Err(e) => {
                *rows.entry(("failed", "decode".into(), normalize_refusal(&e))).or_default() += 1;
                continue;
            }
        };
        let stage = format!("{:?}", shader.stage).to_lowercase();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| translate(&bytes, &shader, None, None, None)));
            let _ = tx.send(match result {
                Ok(Ok(t)) => t.0.contains(tuxbox_xshader::hlsl::MIXED_MASK_MARK).then_some(("mixed", String::new())),
                Ok(Err(Error::Unsupported(e))) => Some(("unsupported", e)),
                Ok(Err(Error::Other(e))) => Some(("failed", e)),
                Err(_) => Some(("crash", "translator panic".to_string())),
            });
        });
        match rx.recv_timeout(limit) {
            Ok(None) => ok += 1,
            Ok(Some(("mixed", _))) => { ok += 1; mixed += 1; }
            Ok(Some((kind, reason))) => *rows.entry((kind, stage, normalize_refusal(&reason))).or_default() += 1,
            Err(_) => *rows.entry(("timeout", stage, format!("over {} s", limit.as_secs()))).or_default() += 1,
        }
    }
    let mut sorted: Vec<_> = rows.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    if json {
        let items: Vec<String> = sorted.iter().map(|((kind, stage, reason), n)| format!(
            "{{\"outcome\":{},\"stage\":{},\"reason\":{},\"programs\":{n}}}",
            json_string(kind), json_string(stage), json_string(reason))).collect();
        println!("{{\"programs\":{total},\"translated\":{ok},\"mixed_mask_programs\":{mixed},\"refusals\":[{}]}}", items.join(","));
    } else {
        println!("{total} programs, {ok} translated, {} refused (first site each)", total - ok);
        println!("{mixed} translated programs mix a lane mask with a number in a scalar op (TB_MIXED_MASK)");
        println!("{:>8}  {:<11} {:<9} reason", "programs", "outcome", "stage");
        for ((kind, stage, reason), n) in &sorted {
            println!("{n:>8}  {kind:<11} {stage:<9} {reason}");
        }
    }
    // A timed-out translation may still be running on its thread.
    std::process::exit(0);
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(Error::Other(error)) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
        Err(Error::Unsupported(error)) => {
            eprintln!("{error}");
            std::process::ExitCode::from(UNSUPPORTED_HLSL)
        }
    }
}
