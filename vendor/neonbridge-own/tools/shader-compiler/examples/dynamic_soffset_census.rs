//! Classify s_load instructions that the emitter refused for a dynamic SOFFSET.
//! usage: dynamic_soffset_census <requests dir> <list of "<name> <hex offset>" lines>
//!
//! Prints one line per shader: stage, the load's dword count and immediate, and
//! the instruction that last wrote the SOFFSET register (searched backwards
//! through the straight-line listing), the SM6.6 dynamic-resource feature bits
//! (25 and 26) and the root signature's flags and parameter count. Aggregate with `sort | uniq -c`.
use std::fmt::Write as _;

fn kind(operand: &tuxbox_dxbc::rdna2::Opnd) -> String {
    let text = format!("{operand:?}");
    text.split('(').next().unwrap_or("?").to_string()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let requests = std::path::PathBuf::from(args.next().expect("requests dir"));
    let list = std::fs::read_to_string(args.next().expect("list file")).expect("list");
    for line in list.lines() {
        let mut fields = line.split_whitespace();
        let (Some(name), Some(at)) = (fields.next(), fields.next()) else { continue };
        let at = usize::from_str_radix(at.trim_start_matches("0x"), 16).expect("offset");
        let Ok(bytes) = std::fs::read(requests.join(name)) else { continue };
        let Ok(shader) = tuxbox_xshader::extract(&bytes) else { continue };
        let insns = tuxbox_dxbc::rdna2::disassemble(&shader.isa);
        let Some(load) = insns.iter().position(|insn| insn.offset == at) else { continue };
        let insn = &insns[load];
        let soffset = insn.raw.get(1).map_or(0, |word| word >> 25);
        let mut definition = String::from("none-found");
        for earlier in insns[..load].iter().rev().take(80) {
            let writes = earlier
                .dst
                .iter()
                .any(|dst| format!("{dst:?}") == format!("Sgpr({soffset})"));
            if writes {
                let mut sources = String::new();
                for source in &earlier.src {
                    let _ = write!(sources, "{},", kind(source));
                }
                definition = format!("{}({})", earlier.mnemonic, sources.trim_end_matches(','));
                break;
            }
        }
        let features = shader.required_features.map_or(0, |bits| bits);
        let root = shader
            .root_sig
            .as_ref()
            .map_or("no-root".to_string(), |root| format!("flags={:#x},params={}", root.flags, root.params.len()));
        println!(
            "{:?} n={} imm={} soff_def={} bindless_bits={:#x} root={}",
            shader.stage,
            1u32 << insn.op,
            insn.aux.imm,
            definition,
            (features >> 25) & 3,
            root
        );
    }
}
