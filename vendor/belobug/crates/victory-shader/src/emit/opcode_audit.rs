//! A single base encoding per table entry. This measures which instructions
//! translate in these compute fixtures, including unused results. It does
//! not establish operand/modifier coverage or correctness on a GPU.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use super::{lower, io, ir, resource, structure};
use crate::ShaderError;
use crate::gfx10::{self, Encoding, test_asm::{S_ENDPGM, bytes, opcodes, sopp}};
use crate::native::{ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource};

pub(super) const fn sop1(op: u32, dst: u32, src: u32) -> u32 {
    0xBE80_0000 | dst << 16 | op << 8 | src
}

pub(super) const fn sop2(op: u32, dst: u32, a: u32, b: u32) -> u32 {
    0x8000_0000 | op << 23 | dst << 16 | b << 8 | a
}

pub(super) const fn sopc(op: u32, a: u32, b: u32) -> u32 {
    0xBF00_0000 | op << 16 | b << 8 | a
}

pub(super) const fn sopk(op: u32, dst: u32, imm: u32) -> u32 {
    0xB000_0000 | op << 23 | dst << 16 | imm
}

pub(super) const fn vop1(op: u32, dst: u32, src: u32) -> u32 {
    0x7E00_0000 | dst << 17 | op << 9 | src
}

pub(super) const fn root_param(root: RootType, user_data: u8, sgprs: u32) -> RootParam {
    RootParam { root, user_data, sgprs, table_offset_dwords: 0, base_register: 0, slot: 0 }
}

pub(super) fn compute(words: &[u32], layout: Vec<RootParam>, lds: bool) -> NativeProgram {
    NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 32,
        layout,
        used: Vec::new(),
        io: StageIo::Cs(ComputeIo { rsrc2: u32::from(lds) << 15, threads: [32, 1, 1] }),
        code: bytes(words),
    }
}

fn fixture(encoding: Encoding, op: u32, name: &str) -> Result<(NativeProgram, usize), &'static str> {
    use Encoding::*;
    // s[0:7] are input words, s8 is a table address and s[10:11] a root
    // descriptor. Use distinct, aligned registers for wide operands.
    let mut layout = vec![root_param(RootType::Constants, 0, 8)];
    let mut words = Vec::new();
    let descriptor = matches!(encoding, Mubuf | Mtbuf)
        || encoding == Smem && name.starts_with("s_buffer_");
    if descriptor {
        layout.push(root_param(RootType::Table, 8, 1));
        words.extend([sop1(3, 9, 128), 0xF408_0404, 0xFA00_0000]); // s_load_dwordx4 s[16:19], s[8:9], null
    }
    if encoding == Global || encoding == Smem && !descriptor {
        layout.push(root_param(RootType::Descriptor, 10, 2));
    }
    if matches!(name, "s_addc_u32" | "s_subb_u32") || name.starts_with("s_cmov")
        || name.starts_with("s_cselect") || name.starts_with("s_cbranch_scc")
    {
        words.push(sopc(6, 0, 128)); // s_cmp_eq_u32 s0, 0
    }
    if name.starts_with("s_cmov") || name.starts_with("s_bitset") || matches!(name, "s_addk_i32" | "s_mulk_i32") {
        words.push(sop1(4, 24, 6)); // old destination, including its high dword
    }
    if name.contains("rel") || name.starts_with("v_interp") || encoding == Ds {
        words.push(sop1(3, 124, 128)); // M0
    }
    if matches!(encoding, Vop1 | Vop2 | Vopc | Vop3 | Vop3p | Ds | Vintrp | Exp | Global | Mubuf | Mtbuf) {
        let last = if encoding == Ds { 15 } else { 9 };
        for r in 4..=last {
            let src = if r == 4 && matches!(encoding, Ds | Global) { 128 } else { (r - 4) % 8 };
            words.push(vop1(1, r, src));
        }
    }
    if name.starts_with("s_cbranch_vcc") || encoding == Vop2
        && (name == "v_cndmask_b32" || name.ends_with("_co_ci_u32"))
    {
        words.push(0x7C00_0000 | 0xC2 << 17 | 256); // v_cmp_eq_u32 vcc_lo, v0, v0
    }
    let pc_load = encoding == Smem && name.starts_with("s_load_dword");
    if pc_load {
        words.push(sop1(0x1F, 10, 0)); // s_getpc_b64 s[10:11]
    }
    let offset = 4 * words.len();
    match encoding {
        Sop2 => words.push(sop2(op, 24, 0, 2)),
        Sop1 => {
            let dst = if matches!(name, "s_setpc_b64" | "s_rfe_b64") { 0 } else { 24 };
            words.push(sop1(op, dst, 0));
        }
        Sopc => words.push(sopc(op, 0, 2)),
        Sopk => {
            let reg = if name.starts_with("s_cmpk_") || name == "s_setreg_b32"
                || name == "s_version" || name.starts_with("s_waitcnt") { 0 } else { 24 };
            let imm = if name.contains("reg_") { 1 | 31 << 11 } else { 0 }; // MODE, full dword
            words.push(sopk(op, reg, imm));
            if name == "s_setreg_imm32_b32" {
                words.push(0);
            }
        }
        Sopp => words.push(sopp(op, 0)), // any relative branch targets the following instruction
        Vop1 => {
            let no_operands = matches!(name, "v_nop" | "v_pipeflush" | "v_clrexcp");
            words.push(vop1(op, if no_operands { 0 } else { 24 }, if no_operands { 0 } else { 260 }));
        }
        Vop2 => {
            words.push(op << 25 | 24 << 17 | 6 << 9 | (256 + 4));
            if name.ends_with("mk_f32") || name.ends_with("ak_f32")
                || name.ends_with("mk_f16") || name.ends_with("ak_f16")
            {
                words.push(0x3F80_0000);
            }
        }
        Vopc => words.push(0x7C00_0000 | op << 17 | 6 << 9 | (256 + 4)),
        Vop3 | Vop3p => {
            let base = if encoding == Vop3 { 0xD400_0000 } else { 0xCC00_0000 };
            let mut w = base | op << 16 | 24;
            // Ask the decoder for the operand count and presence of SDST.
            let inst = gfx10::Instruction { offset, encoding, opcode: op as u16, words: vec![w, 0] };
            let gfx10::Fields::Valu(v) = inst.fields(false) else { unreachable!() };
            if v.carry.is_some() {
                w |= 26 << 8;
            }
            let mut src = [260, 262, 264];
            match name {
                "v_readlane_b32" => src[1] = 128,
                "v_writelane_b32" => src = [0, 128, 0],
                "v_screen_partition_4se_b32" => src[1..].fill(0),
                n if n.starts_with("v_interp_") => src[0] = 0,
                n if n.ends_with("_co_ci_u32") => src[2] = 4,
                _ => {}
            }
            src[v.src.len()..].fill(0);
            words.extend([w, src[0] | src[1] << 9 | src[2] << 18]);
        }
        Vintrp => words.push(0xC800_0000 | 24 << 18 | op << 16),
        Ds => {
            let gds = name.starts_with("ds_gws") || matches!(name, "ds_append" | "ds_consume" | "ds_ordered_count");
            words.extend([0xD800_0000 | op << 18 | u32::from(gds) << 17, 0x180C_0804]);
        }
        Global => words.extend([0xDC00_8000 | op << 18, 24 << 24 | 10 << 16 | 8 << 8 | 4]),
        Mubuf => words.extend([0xE000_0000 | (op & 127) << 18 | (op >> 7) << 25, 128 << 24 | 4 << 16 | 24 << 8]),
        Mtbuf => {
            let format = match name.rsplit('_').next().unwrap() {
                "xy" => 62, "xyz" => 72, "xyzw" => 75, _ => 20, // 32-bit UINT components
            };
            words.extend([0xE800_0000 | (op & 7) << 16 | format << 19,
                128 << 24 | (op >> 3) << 21 | 4 << 16 | 24 << 8]);
        }
        Smem if name.contains("scratch") => return Err("private segment addressing"),
        Smem => {
            let base = if descriptor { 16 } else { 10 };
            let imm = if pc_load { 12 } else { 0 }; // after the load and s_endpgm
            words.extend([0xF400_0000 | op << 18 | 24 << 6 | (base / 2), 0xFA00_0000 | imm]);
        }
        Exp => words.extend([0xF800_080F, 0x0706_0504]), // MRT0.xyzw, done
        Mimg => return Err("image descriptors and coordinates"),
        Flat => return Err("generic flat address space"),
        Scratch => return Err("private segment addressing"),
    }
    words.push(S_ENDPGM);
    if pc_load {
        words.extend([0; 16]);
    }
    let mut native = compute(&words, layout, encoding == Ds);
    if encoding == Global {
        native.used.push(UsedResource {
            kind: ResourceType::Srv, root: RootType::Descriptor, table_offset_dwords: 0,
            lower_bound: 0, stride: 0, dwords: 4, user_data: 10, slot: 0,
        });
    }
    let decoded = gfx10::decode(&native.code).unwrap();
    assert!(decoded.iter().any(|i| i.offset == offset && i.encoding == encoding && i.name() == Some(name)));
    Ok((native, offset))
}

fn translate(native: &NativeProgram, offset: usize) -> Result<(), (&'static str, ShaderError)> {
    let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).map_err(|e| ("ir", e))?;
    let res = resource::track(&p, &native.layout, native.stage).map_err(|e| ("resource", e))?;
    let io = io::raise(&p, native).map_err(|e| ("io", e))?;
    let st = structure::structure(&p).map_err(|e| ("structure", e))?;
    lower::AUDIT_OFFSET.set(Some(offset));
    let lowered = lower::lower(&lower::Parts {
        native, p: &p, st: &st, io: &io, res: &res, inputs: Vec::new(), outputs: Vec::new(),
        prims: Vec::new(), mesh_stride: 0,
        target: crate::emit::Target::ANY, console_cf: false, per_lane_only: false, psv: None,
    });
    lower::AUDIT_OFFSET.set(None);
    lowered.map(|_| ()).map_err(|e| ("lower", e))
}

// Run in a child test process so a stack overflow cannot abort the test
// runner. Each BEGIN resets the parent's deadline and identifies the case
// even if the process hangs or aborts before it can report the result.
#[test]
#[ignore = "worker for the opcode audit"]
fn audit_worker() {
    if std::env::var_os("VICTORY_OPCODE_AUDIT_WORKER").is_none() {
        return;
    }
    for (encoding, op, name) in opcodes() {
        println!("BEGIN\t{encoding:?}\t{name}");
        let result = std::panic::catch_unwind(|| match fixture(encoding, u32::from(op), name) {
            Err(reason) => format!("not audited\t{reason}"),
            Ok((native, offset)) => match translate(&native, offset) {
                Ok(()) => "lowered\t".to_owned(),
                Err((stage, err)) => {
                    let reason = match err {
                        ShaderError::Unsupported { what, .. } => what,
                        ShaderError::Isa { reason, .. } | ShaderError::Malformed(reason) => reason,
                        ShaderError::NotDxbc => "not a DXBC container",
                    };
                    format!("refused\t{stage}: {reason}")
                }
            },
        });
        println!("RESULT\t{}", result.unwrap_or_else(|_| "panic\t".to_owned()));
    }
}

struct Row {
    encoding: String,
    name: String,
    outcome: String,
    reason: String,
}

fn audit() -> Vec<Row> {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "emit::opcode_audit::audit_worker", "--nocapture"])
        .env("VICTORY_OPCODE_AUDIT_WORKER", "1")
        .stdout(Stdio::piped())
        .spawn().unwrap();
    let out = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(out).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut rows = Vec::new();
    let mut current = (String::new(), String::new());
    loop {
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(line) if line.starts_with("BEGIN\t") => {
                let (e, n) = line[6..].split_once('\t').unwrap();
                current = (e.to_owned(), n.to_owned());
            }
            Ok(line) if line.starts_with("RESULT\t") => {
                let (outcome, reason) = line[7..].split_once('\t').unwrap();
                rows.push(Row { encoding: current.0.clone(), name: current.1.clone(),
                    outcome: outcome.to_owned(), reason: reason.to_owned() });
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("opcode audit timed out in {} {}", current.0, current.1);
            }
        }
    }
    reader.join().unwrap();
    assert!(child.wait().unwrap().success(), "opcode audit aborted in {} {}", current.0, current.1);
    assert_eq!(rows.len(), opcodes().count(), "one result per opcode table entry");
    let panics: Vec<_> = rows.iter().filter(|r| r.outcome == "panic").map(|r| &r.name).collect();
    assert!(panics.is_empty(), "opcode audit panicked in {panics:?}");
    rows
}

#[test]
#[ignore = "prints all opcode audit results"]
fn opcode_coverage_table() {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    let mut totals = BTreeMap::<_, usize>::new();
    for row in audit() {
        *totals.entry(row.outcome.clone()).or_default() += 1;
        groups.entry((row.encoding, row.outcome, row.reason)).or_default().push(row.name);
    }
    for ((encoding, outcome, reason), names) in groups {
        println!("{encoding} / {outcome} / {reason} ({}): {}", names.len(), names.join(" "));
    }
    println!("{totals:?}");
}

#[test]
fn refused_opcodes() {
    let rows = audit();
    let refused: BTreeSet<_> = rows.iter().filter(|r| r.outcome == "refused").map(|r| r.name.as_str()).collect();
    let expected: BTreeSet<_> = include_str!("opcode_refusals.txt").lines().collect();
    let added: Vec<_> = refused.difference(&expected).collect();
    let removed: Vec<_> = expected.difference(&refused).collect();
    // An opcode that lowers now only makes its line stale, so a change that
    // lowers one need not touch the list; an opcode that stopped lowering
    // fails.
    if !removed.is_empty() {
        println!("opcode_refusals.txt lists opcodes that lower now: {removed:?}");
    }
    assert!(added.is_empty(), "opcodes that no longer lower: {added:?}");
}
