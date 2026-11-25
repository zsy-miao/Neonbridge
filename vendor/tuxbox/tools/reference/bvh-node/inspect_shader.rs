//! Inspect the locally supplied shader's immediate BVH descriptor setup.
//! This deliberately recognises only the verified register setup; unknown
//! instructions or a branch between setup and use cause an error.
use std::{env, fs, process::ExitCode};
use tuxbox_xshader::rdna2::{instructions, Insn, Opnd};

fn descriptor(window: &[Insn]) -> Result<[u32; 4], String> {
    let start = window
        .iter()
        .rposition(|i| {
            i.dst == [Opnd::Sgpr(68)] && i.mnemonic == "s_mov_b32" && i.src == [Opnd::ConstInt(0)]
        })
        .ok_or("descriptor setup not found")?;
    let mut values = [None; 4];
    for i in &window[start..] {
        if i.mnemonic.starts_with("s_branch") || i.mnemonic.starts_with("s_cbranch") {
            return Err("branch inside descriptor setup".into());
        }
        for dst in &i.dst {
            let Opnd::Sgpr(register @ 68..=71) = *dst else {
                continue;
            };
            let value = match (i.mnemonic.as_str(), i.src.as_slice()) {
                ("s_mov_b32", [Opnd::ConstInt(value)]) => *value as u32,
                ("s_mov_b32", [Opnd::Literal(value)]) => *value,
                ("s_brev_b32", [Opnd::ConstInt(value)]) => (*value as u32).reverse_bits(),
                _ => return Err(format!("unsupported descriptor write at {:#x}", i.offset)),
            };
            values[usize::from(register - 68)] = Some(value);
        }
    }
    Ok([
        values[0].ok_or("missing descriptor word 0")?,
        values[1].ok_or("missing descriptor word 1")?,
        values[2].ok_or("missing descriptor word 2")?,
        values[3].ok_or("missing descriptor word 3")?,
    ])
}

fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let input = args.next().ok_or("usage: inspect_shader SHADER.dxbc")?;
    if args.next().is_some() {
        return Err("expected one input file".into());
    }
    let data = fs::read(input).map_err(|e| e.to_string())?;
    let shader = tuxbox_xshader::extract(&data)?;
    let ins: Vec<_> = instructions(&shader.isa).collect();
    let mut count = 0;
    for (index, instruction) in ins.iter().enumerate() {
        if instruction.mnemonic != "image_bvh64_intersect_ray" {
            continue;
        }
        if !instruction.src.contains(&Opnd::Sgpr(68)) {
            return Err(format!(
                "unexpected descriptor register at {:#x}",
                instruction.offset
            ));
        }
        let values = descriptor(&ins[index.saturating_sub(24)..index])?;
        println!(
            "offset={:#x} descriptor={:08x},{:08x},{:08x},{:08x} box_growth={} sorted={}",
            instruction.offset,
            values[0],
            values[1],
            values[2],
            values[3],
            (values[1] >> 23) & 0xff,
            values[1] >> 31
        );
        count += 1;
    }
    if count == 0 {
        return Err("no matching BVH instruction".into());
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
