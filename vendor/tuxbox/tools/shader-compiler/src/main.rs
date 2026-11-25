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
            // Mesh programs also feed per-primitive attributes to the pixel stage.
            for output in producer.output_sig.iter_mut().chain(producer.primitive_sig.iter_mut()).filter(|o| o.sysval == 0) {
                let mut matches = interface.inputs.iter().filter(|i| i.sysval == 0
                    && i.index == output.index && i.name.eq_ignore_ascii_case(&output.name));
                if let Some(input) = matches.next() {
                    // A packed half output has no integer form.
                    if matches.next().is_some() || !matches!(input.comp_type, 1..=3)
                        || output.comp_type == 6 && input.comp_type != 3 {
                        return Err(Error::Unsupported("ambiguous or unsupported portable pixel component type".into()));
                    }
                    output.comp_type = input.comp_type;
                }
            }
            hlsl::InputInterp(interface.interpolation)
        } else {
            let pixel = extract(&bytes)?;
            if pixel.stage != Stage::Pixel {
                return Err("expected a pixel consumer".into());
            }
            // Console interpolants are raw dwords, so a title may write an element
            // as float and read it as uint (or the reverse). Desktop drivers reject
            // the pair (vkd3d: "signature element ... not compatible"). Give each
            // producer output the consumer's component type; exports keep the bits.
            for output in producer.output_sig.iter_mut().chain(producer.primitive_sig.iter_mut()).filter(|o| o.sysval == 0) {
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
            hlsl::translate(&pixel).map_err(Error::Unsupported)?.ps_input_interp
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
