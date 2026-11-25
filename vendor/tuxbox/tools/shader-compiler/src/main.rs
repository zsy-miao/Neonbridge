//! Translate a cooked shader, optionally linking its outputs to a pixel shader.
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use tuxbox_xshader::{extract, ShaderExtract, Stage};

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

#[cfg(feature = "legacy-hlsl")]
fn translate(
    shader: &ShaderExtract,
    consumer: Option<&Path>,
    pixel_order: Option<&Path>,
    stencil_target: Option<u32>,
) -> Result<(String, &'static str), Error> {
    use tuxbox_xshader::{hlsl, meta};

    let translated = if let Some(path) = consumer {
        let bytes = read(path)?;
        if !matches!(shader.stage, Stage::Vertex | Stage::Geometry | Stage::Mesh) {
            return Err("expected a vertex, geometry or mesh producer and a pixel consumer".into());
        }
        let actual = pixel_order.map(read).transpose()?;
        // A portable original consumer has no native ISA to translate. Read
        // its interpolation directly; retain native consumer handling otherwise.
        let portable = tuxbox_xshader::psv::portable_pixel_interface(&bytes);
        let is_portable = portable.is_some();
        let mut producer = shader.clone();
        let interp = if let Some(interface) = portable {
            if actual.as_deref().is_some_and(|code| code != bytes.as_slice()) {
                return Err(Error::Unsupported("portable pixel consumer differs from its bound bytecode".into()));
            }
            // Portable consumers need the same component-type repair as
            // native consumers. Only established 32-bit types are supported.
            // TuxBox 6a2ca92 (Valdi320, PR #117).
            for output in producer.output_sig.iter_mut().filter(|o| o.sysval == 0) {
                let mut matches = interface.inputs.iter().filter(|i| i.sysval == 0
                    && i.index == output.index && i.name.eq_ignore_ascii_case(&output.name));
                if let Some(input) = matches.next() {
                    if matches.next().is_some() || !matches!(input.comp_type, 1..=3) {
                        return Err(Error::Unsupported("ambiguous or unsupported portable pixel component type".into()));
                    }
                    output.comp_type = input.comp_type;
                }
            }
            let read = (shader.stage == Stage::Mesh)
                .then(|| interface.inputs.iter().map(|i| format!("{}{}", i.name, i.index)).collect());
            hlsl::InputInterp(interface.interpolation, read)
        } else {
            let pixel = extract(&bytes)?;
            if pixel.stage != Stage::Pixel {
                return Err("expected a pixel consumer".into());
            }
            // Console interpolants are raw dwords, so a title may write an element
            // as float and read it as uint (or the reverse). Desktop drivers reject
            // the pair (vkd3d: "signature element ... not compatible"). Give each
            // producer output the consumer's component type; exports keep the bits.
            for output in producer.output_sig.iter_mut().filter(|o| o.sysval == 0) {
                let input = pixel.input_sig.iter().find(|i| {
                    i.sysval == 0 && i.index == output.index && i.name.eq_ignore_ascii_case(&output.name)
                });
                // 1 = uint, 2 = sint; 0, 3 (float) and 6 (float16) all emit as float.
                let integer = |t: u32| matches!(t, 1 | 2);
                match input.map(|i| i.comp_type) {
                    Some(t) if integer(t) => output.comp_type = t,
                    Some(_) if integer(output.comp_type) => output.comp_type = 3,
                    _ => {}
                }
            }
            let mut interp = hlsl::translate(&pixel).map_err(Error::Unsupported)?.ps_input_interp;
            // A linked mesh producer drops the vertex outputs this pixel shader does
            // not declare (five-star-recomp 1236bfa, GPL-3.0, ThatRecompDev).
            if shader.stage == Stage::Mesh {
                interp.1 = Some(pixel.input_sig.iter().map(|i| format!("{}{}", i.name, i.index)).collect());
            }
            interp
        };
        let order = actual.as_deref().map(meta::pixel_input_order)
            .unwrap_or_else(|| if is_portable { meta::pixel_input_order(&bytes) } else { Vec::new() });
        hlsl::translate_ordered(&producer, Some(&interp), &order)
    } else if let Some(target) = stencil_target {
        hlsl::translate_stencil_target(shader, target)
    } else {
        hlsl::translate(shader)
    }.map_err(Error::Unsupported)?;
    let model = if translated.required_wave_size.is_some()
        || translated.typed_int64_atomics
        || matches!(translated.profile, "ps_6_6" | "cs_6_6" | "ms_6_6")
    {
        "6_6"
    } else if shader.stage == Stage::Mesh || translated.profile == "cs_6_5" {
        "6_5"
    } else if translated.packed_dot_product {
        "6_4"
    } else if translated.needs_barycentrics {
        "6_1"
    } else {
        "6_0"
    };
    Ok((translated.hlsl, model))
}

#[cfg(not(feature = "legacy-hlsl"))]
fn translate(
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
    Ok((hlsl, if tuxbox_xshader::requires_inline_ray_query(shader) { "6_5" } else { "6_0" }))
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
    let mut target_writes_output = None;
    let mut consumer = None;
    let mut pixel_order = None;
    let mut stencil_target = None;
    while let Some(argument) = args.next() {
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
            Some("--target-writes-out") => &mut target_writes_output,
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
    let (hlsl, model) = translate(
        &shader,
        consumer.as_deref(),
        pixel_order.as_deref(),
        stencil_target,
    )?;
    if let Some(path) = profile_output {
        let stage = match shader.stage {
            Stage::Pixel => "ps",
            Stage::Vertex => "vs",
            Stage::Geometry => "gs",
            Stage::Compute => "cs",
            Stage::Mesh => "ms",
            _ => return Err("unsupported DXC stage".into()),
        };
        fs::write(&path, format!("{stage}_{model}"))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    // The channels each colour target's exports enable, as `target=mask` (hex,
    // bit 0 red) for the exported targets: the HLSL writes 0.0 into a disabled
    // channel, where the hardware leaves the target's channel alone, so the
    // runtime ANDs the mask into RenderTargetWriteMask (Xbox2Windows
    // 03dee932 / 27f32934, MIT). Read from the ISA so the HLSL is unchanged.
    if let Some(path) = target_writes_output {
        let mut writes = std::collections::BTreeMap::<i64, u8>::new();
        if shader.stage == Stage::Pixel {
            for ins in tuxbox_xshader::rdna2::instructions(&shader.isa) {
                if ins.enc != tuxbox_xshader::rdna2::Enc::Exp || !(0..8).contains(&ins.aux.imm) {
                    continue;
                }
                let (en, compr) = (ins.aux.mask, ins.aux.imm2 & 1 != 0);
                let on = |i: u8| if compr { (en >> (i / 2 * 2)) & 1 != 0 || (en >> i) & 1 != 0 } else { (en >> i) & 1 != 0 };
                *writes.entry(ins.aux.imm).or_default() |= (0..4u8).filter(|&i| on(i)).fold(0, |m, i| m | 1 << i);
            }
        }
        let list: Vec<String> = writes.iter().map(|(t, m)| format!("{t}={m:x}")).collect();
        fs::write(&path, list.join(" ")).map_err(|e| format!("{}: {e}", path.display()))?;
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
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| translate(&shader, None, None, None)));
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
