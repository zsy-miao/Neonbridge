//! Describe decoded EXEC elections without printing retail instruction bytes.
use std::{env, fs, process::ExitCode};
use tuxbox_xshader::{extract, rdna2::{instructions, Enc, Opnd}};

fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let path = args.next().ok_or("expected a shader container path")?;
    if args.next().is_some() { return Err("expected exactly one shader path".into()); }
    let ex = extract(&fs::read(path).map_err(|e| e.to_string())?)?;
    println!("stage\t{:?}", ex.stage);
    println!("entry\t{}", ex.entry.as_deref().unwrap_or(""));
    let ins: Vec<_> = instructions(&ex.isa).collect();
    for (i, op) in ins.iter().enumerate() {
        if op.mnemonic != "s_ff1_i32_b64" || op.src != [Opnd::ExecLo] { continue; }
        let end = (i + 30).min(ins.len());
        let next = ins[i + 1..end].iter().position(|p| p.mnemonic == "s_ff1_i32_b64" && p.src == [Opnd::ExecLo]);
        let block = &ins[i..next.map_or(end, |n| i + 1 + n)];
        let names = ["s_and_saveexec_b64", "s_bcnt1_i32_b64", "buffer_atomic_add", "v_readfirstlane_b32", "v_mbcnt_lo_u32_b32", "v_mbcnt_hi_u32_b32", "v_readlane_b32", "v_permlanex16_b32"];
        print!("election\t{:#x}", op.offset);
        for name in names { print!("\t{}", block.iter().filter(|v| v.mnemonic == name).count()); }
        let branches = block.iter().filter(|v| v.enc == Enc::Sopp && v.mnemonic.starts_with("s_cbranch_")).count();
        println!("\t{branches}");
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() { Ok(()) => ExitCode::SUCCESS, Err(e) => { eprintln!("{e}"); ExitCode::FAILURE } }
}
