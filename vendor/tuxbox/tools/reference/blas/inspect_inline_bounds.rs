//! Check only the four candidate-distance comparisons of a supplied shader.
//! This does not prove that other predicates admit any particular candidate.
//! Build the shader-compiler library as in reference/bvh-node/native/README.md,
//! then link this file against that library and pass the extracted DXBC file.
use std::{env, fs, process::ExitCode};
use tuxbox_xshader::rdna2::{instructions, Opnd};

fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let path = args
        .next()
        .ok_or("usage: inspect_inline_bounds SHADER.dxbc")?;
    if args.next().is_some() {
        return Err("expected one shader file".into());
    }
    let shader = tuxbox_xshader::extract(&fs::read(path).map_err(|e| e.to_string())?)?;
    if shader.entry.as_deref() != Some("LumenSceneDirectLightingHardwareRayTracingCS") {
        return Err("unexpected shader entry point".into());
    }
    let ins: Vec<_> = instructions(&shader.isa).collect();
    let checks: [(usize, &str, &[Opnd]); 4] = [
        (
            0xa34,
            "v_cmp_nlt_f32",
            &[Opnd::Vgpr(46), Opnd::Vgpr(15), Opnd::Sgpr(0)],
        ),
        (0xa50, "v_cmp_nge_f32", &[Opnd::Vgpr(46), Opnd::Vgpr(23)]),
        (
            0x11cc,
            "v_cmp_ngt_f32",
            &[Opnd::ConstInt(0), Opnd::Vgpr(38), Opnd::Sgpr(0)],
        ),
        (0x11e8, "v_cmp_nge_f32", &[Opnd::Vgpr(38), Opnd::Vgpr(15)]),
    ];
    for (offset, name, operands) in checks {
        let instruction = ins
            .iter()
            .find(|i| i.offset == offset)
            .ok_or_else(|| format!("missing instruction at {offset:#x}"))?;
        if instruction.mnemonic != name || instruction.src != operands {
            return Err(format!("unexpected instruction at {offset:#x}"));
        }
        println!("{offset:#06x} {name} {:?}", instruction.src);
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
