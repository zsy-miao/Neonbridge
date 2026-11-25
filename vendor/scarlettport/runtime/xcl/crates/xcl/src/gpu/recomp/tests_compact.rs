//! A/B tests of the compact form (`compact::translate_compact`) against the normal one
//! (`translate`): the same program, translated both ways, compiled with DXC and run on the GPU
//! with the same inputs, must leave byte-identical results (SGPRs, SCC, VGPRs of every lane,
//! exports, the upper half's SGPRs of the split wave64 form, and memory).
//!
//! Every program runs in each form this adapter has: wave64 split into two 32-lane halves
//! (`runner::split_wave64`, e.g. NVIDIA), wave64 in 64-lane waves (where the GPU has them) and
//! as a wave32 program; control flow also with `structured: false` (the normal form's guarded
//! block chain). `XCL_SAMPLE_BIG_CS=<code file>` adds a retail program.

use std::collections::HashMap;

use super::runner::{self, KernelResult, KernelState};
use super::{compact, translate, CsInterface, Mode, Options, Translation};
use super::{KERNEL_EXPORTS, KERNEL_LANES, KERNEL_RES_DWORDS, KERNEL_SCC_SLOT, KERNEL_SGPRS, KERNEL_SPLIT_SGPRS, KERNEL_VGPRS};
use crate::gpu::rdna2::{self, encode};

const V: u64 = 256; // VALU source code of v0
const EXEC: u64 = 126;
const VCC: u64 = 106;

/// Initial state of an A/B run: registers, and flat memory from address 0.
#[derive(Clone)]
struct Inputs {
    state: KernelState,
    mem: Vec<u32>,
}

impl Inputs {
    /// All 64 lanes on, v0 = lane index, `mem` as given.
    fn lanes(mem: Vec<u32>) -> Self {
        let mut state = KernelState::default();
        for (l, r) in state.vgpr.iter_mut().enumerate() {
            r[0] = l as u32;
        }
        Inputs { state, mem }
    }

    fn vgpr(mut self, reg: usize, f: impl Fn(usize) -> u32) -> Self {
        for (l, r) in self.state.vgpr.iter_mut().enumerate() {
            r[reg] = f(l);
        }
        self
    }

    fn sgprs(mut self, at: usize, v: &[u32]) -> Self {
        self.state.sgpr[at..at + v.len()].copy_from_slice(v);
        self
    }
}

/// A GPU that runs wave64 programs (split or native) and DXC; `skip_test` otherwise.
fn gpu_ready() -> bool {
    match (runner::adapter(), runner::wave64_programs_supported(), super::dxc::location()) {
        (Ok(_), Ok(true), Ok(_)) => true,
        (a, w, d) => {
            runner::skip_test(if a.is_err() || w.is_err() { "no GPU" } else if d.is_err() { "no DXC" } else { "no wave64" });
            false
        }
    }
}

/// The forms a program runs in on this adapter: (label, options), mode Kernel.
fn forms(control_flow: bool) -> Vec<(String, Options)> {
    let mut base = Vec::new();
    if runner::split_wave64() {
        base.push(("wave64 split".to_string(), Options { wave64: true, split_wave64: true, ..Default::default() }));
    }
    if runner::wave64_supported() == Ok(true) {
        base.push(("wave64 native".to_string(), Options { wave64: true, split_wave64: false, ..Default::default() }));
    }
    base.push(("wave32".to_string(), Options { wave64: false, ..Default::default() }));
    let mut out = Vec::new();
    for (name, o) in base {
        if control_flow {
            out.push((format!("{name}, unstructured"), Options { structured: false, ..o.clone() }));
        }
        out.push((name, o));
    }
    out
}

/// Translation (must be complete) → DXIL.
fn compiled(t: &Translation, backend: &str) -> Vec<u8> {
    assert!(t.complete(), "{backend}: unsupported {:?}", t.unsupported_why);
    super::dxc::compile(&t.hlsl, "main", "cs_6_6", &[]).unwrap_or_else(|e| panic!("{backend}: DXC: {e}"))
}

fn both_translations(code: &[u8], opt: &Options) -> (Translation, Translation) {
    let normal = translate(code, opt).unwrap_or_else(|e| panic!("translate: {e}"));
    let compact = compact::translate_compact(code, opt).unwrap_or_else(|e| panic!("translate_compact: {e}"));
    (normal, compact)
}

/// Result dwords of a kernel run, then its memory, as bytes.
fn bytes(r: &KernelResult) -> Vec<u8> {
    r.raw.iter().chain(r.mem.iter()).flat_map(|w| w.to_le_bytes()).collect()
}

/// Runs `code` translated by `translate` and by `compact::translate_compact` (options `opt`
/// as given, mode Kernel) on the same inputs: the result buffer (SGPRs, SCC, VGPRs, exports,
/// the split form's upper-half SGPRs) followed by the memory afterwards, as bytes, of each.
fn both_backends(code: &[u8], opt: &Options, inputs: &Inputs) -> (Vec<u8>, Vec<u8>) {
    let (normal, compact) = both_translations(code, opt);
    let run = |t: &Translation, backend: &str| {
        let dxil = compiled(t, backend);
        let r = runner::run_with_statics(&dxil, &inputs.state, &inputs.mem, &[], &[], &t.static_samplers).unwrap_or_else(|e| panic!("{backend}: run: {e}"));
        bytes(&r)
    };
    (run(&normal, "translate"), run(&compact, "translate_compact"))
}

/// Name of result dword `i` (`bytes` layout).
fn slot(i: usize) -> String {
    match i {
        KERNEL_SCC_SLOT => "scc".into(),
        _ if i < KERNEL_SGPRS => format!("s{i}"),
        _ if i < KERNEL_EXPORTS => format!("lane {} v{}", (i - KERNEL_SGPRS) / KERNEL_VGPRS, (i - KERNEL_SGPRS) % KERNEL_VGPRS),
        _ if i < KERNEL_SPLIT_SGPRS => {
            let k = i - KERNEL_EXPORTS;
            format!("export {} lane {} .{}", k / (KERNEL_LANES * 4), k / 4 % KERNEL_LANES, k % 4)
        }
        _ if i < KERNEL_RES_DWORDS => format!("upper half s{}", i - KERNEL_SPLIT_SGPRS),
        _ => format!("mem[{:#x}]", 4 * (i - KERNEL_RES_DWORDS)),
    }
}

/// Dwords where `a` and `b` differ, named (first 24).
fn differences(a: &[u8], b: &[u8], name: fn(usize) -> String) -> Vec<String> {
    let w = |x: &[u8], i: usize| x.get(4 * i..4 * i + 4).map(|c| u32::from_le_bytes(c.try_into().unwrap()));
    let n = a.len().max(b.len()).div_ceil(4);
    let all: Vec<String> = (0..n).filter(|&i| w(a, i) != w(b, i)).map(|i| format!("{}: {:x?} / {:x?}", name(i), w(a, i), w(b, i))).collect();
    let mut v: Vec<String> = all.iter().take(24).cloned().collect();
    if all.len() > 24 {
        v.push(format!("… {} dwords in all", all.len()));
    }
    v
}

/// `both_backends` in every form of `forms`: the results must be identical. Returns each form
/// with its result (`bytes` layout).
fn assert_same(what: &str, code: &[u8], inputs: &Inputs, control_flow: bool) -> Vec<(String, Options, Vec<u8>)> {
    let mut out = Vec::new();
    for (form, opt) in forms(control_flow) {
        let (a, b) = both_backends(code, &opt, inputs);
        let d = differences(&a, &b, slot);
        assert!(d.is_empty(), "{what} ({form}): translate / translate_compact differ:\n{}", d.join("\n"));
        println!("{what} ({form}): {} result bytes identical", a.len());
        out.push((form, opt, a));
    }
    out
}

/// Dword `i` of a result (`bytes` layout: `slot` names it).
fn dword(r: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(r[4 * i..4 * i + 4].try_into().unwrap())
}

/// Result dword of VGPR `v` of `lane`.
fn vgpr_slot(lane: usize, v: usize) -> usize {
    KERNEL_SGPRS + lane * KERNEL_VGPRS + v
}

/// The dispatch loop of a compact translation, read back from its HLSL: each case's text and
/// the address of its first block, and each block's case in address order (from `XCL_BT`).
struct Dispatch {
    cases: Vec<(u64, String)>,
    class: Vec<usize>,
}

impl Dispatch {
    fn of(t: &Translation) -> Self {
        let h = &t.hlsl;
        let after = |key: &str, end: &str| -> String {
            let s = &h[h.find(key).unwrap_or_else(|| panic!("no {key:?} in the compact form:\n{h}")) + key.len()..];
            s[..s.find(end).unwrap()].to_string()
        };
        let n: usize = after("        if (xcl_nb >= ", "u) break;").parse().unwrap();
        // switch ((XCL_BT[xcl_b / {per}u] >> (xcl_b % {per}u * {bits}u)) & {mask}u)
        let per: usize = after("switch ((XCL_BT[xcl_b / ", "u]").parse().unwrap();
        let bits: u32 = after(&format!("(xcl_b % {per}u * "), "u))").parse().unwrap();
        let table = after("static const uint XCL_BT[", "};");
        let words: Vec<u32> = table[table.find('{').unwrap() + 1..]
            .split(',')
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .map(|w| u32::from_str_radix(w.trim_start_matches("0x").trim_end_matches('u'), 16).unwrap())
            .collect();
        let class: Vec<usize> = (0..n).map(|b| ((words[b / per] >> (b % per) as u32 * bits) & ((1u32 << bits) - 1)) as usize).collect();
        let mut cases: Vec<(u64, String)> = Vec::new();
        let mut open = false;
        for l in h[h.find("switch ((XCL_BT[").unwrap()..].lines().skip(1) {
            if let Some(rest) = l.strip_prefix("        case ") {
                let (k, pc) = rest.split_once("u: { // ").unwrap_or_else(|| panic!("case line {l:?}"));
                assert_eq!(k.parse::<usize>().unwrap(), cases.len(), "cases in order");
                cases.push((u64::from_str_radix(pc, 16).unwrap(), String::new()));
                open = true;
            } else if l == "        } break;" {
                open = false;
            } else if l.starts_with("        default:") {
                break;
            } else if open {
                let text = &mut cases.last_mut().unwrap().1;
                text.push_str(l);
                text.push('\n');
            }
        }
        assert_eq!(cases.len(), t.blocks, "Translation::blocks is the number of cases");
        assert!(class.iter().all(|&c| c < cases.len()), "table {class:?}, {} cases", cases.len());
        for (c, (pc, _)) in cases.iter().enumerate() {
            assert!(class.contains(&c), "case {c} (+{pc:#x}) has a block");
        }
        Dispatch { cases, class }
    }

    fn blocks(&self) -> usize {
        self.class.len()
    }

    /// How many blocks run case `c`.
    fn uses(&self, c: usize) -> usize {
        self.class.iter().filter(|&&k| k == c).count()
    }

    /// The cases whose text contains `s`.
    fn with(&self, s: &str) -> Vec<usize> {
        (0..self.cases.len()).filter(|&c| self.cases[c].1.contains(s)).collect()
    }

    /// The one case whose text contains `s`.
    fn only(&self, s: &str) -> usize {
        let v = self.with(s);
        assert_eq!(v.len(), 1, "cases with {s:?}: {v:?}");
        v[0]
    }

    /// The cases with the line `l` (indentation of a case's text included).
    fn with_line(&self, l: &str) -> Vec<usize> {
        (0..self.cases.len()).filter(|&c| self.cases[c].1.lines().any(|x| x == l)).collect()
    }
}

/// `check` on the compact form of `code` in every form of `forms`.
fn assert_shape(what: &str, code: &[u8], control_flow: bool, check: fn(&Dispatch, &Options)) {
    for (form, opt) in forms(control_flow) {
        let t = compact::translate_compact(code, &opt).unwrap_or_else(|e| panic!("{what} ({form}): {e}"));
        let d = Dispatch::of(&t);
        check(&d, &opt);
        println!("{what} ({form}): compact form has {} cases for {} blocks", d.cases.len(), d.blocks());
    }
}

/// Assembler with labels: branches (SOPP SIMM16) and far jumps (s_getpc_b64 / add or sub of
/// a literal / s_setpc_b64) are patched to their label at the end, which also checks every
/// branch decodes to its label.
#[derive(Default)]
struct Asm {
    code: Vec<u8>,
    labels: HashMap<&'static str, usize>,
    /// (offset of the SOPP branch, label)
    branches: Vec<(usize, &'static str)>,
    /// (offset of s_getpc_b64, label, subtract)
    far: Vec<(usize, &'static str, bool)>,
}

impl Asm {
    fn i(&mut self, name: &str, enc: &str, fields: &[(&str, u64)], lit: Option<u32>) -> &mut Self {
        self.code.extend(encode(name, enc, fields, lit).unwrap_or_else(|e| panic!("{name}: {e}")));
        self
    }

    fn label(&mut self, l: &'static str) -> &mut Self {
        assert!(self.labels.insert(l, self.code.len()).is_none(), "label {l} twice");
        self
    }

    /// `name` (an s_branch / s_cbranch_*) to `l`.
    fn br(&mut self, name: &str, l: &'static str) -> &mut Self {
        self.branches.push((self.code.len(), l));
        self.i(name, "ENC_SOPP", &[("SIMM16", 0)], None)
    }

    /// The compiler's far jump through s[4:5] to `l` (backward when `l` comes first).
    fn far_jump(&mut self, l: &'static str, backward: bool) -> &mut Self {
        self.far.push((self.code.len(), l, backward));
        let (lo, hi) = if backward { ("S_SUB_U32", "S_SUBB_U32") } else { ("S_ADD_U32", "S_ADDC_U32") };
        self.i("S_GETPC_B64", "ENC_SOP1", &[("SDST", 4)], None)
            .i(lo, "ENC_SOP2", &[("SDST", 4), ("SSRC0", 4), ("SSRC1", 255)], Some(0))
            .i(hi, "ENC_SOP2", &[("SDST", 5), ("SSRC0", 5), ("SSRC1", 128)], None)
            .i("S_SETPC_B64", "ENC_SOP1", &[("SSRC0", 4)], None)
    }

    /// The program, with s_endpgm at the end.
    fn end(&mut self) -> Vec<u8> {
        self.i("S_ENDPGM", "ENC_SOPP", &[], None);
        let mut code = std::mem::take(&mut self.code);
        for &(at, l) in &self.branches {
            let rel = (self.labels[l] as i64 - (at as i64 + 4)) / 4;
            code[at..at + 2].copy_from_slice(&(rel as i16).to_le_bytes());
        }
        for &(at, l, backward) in &self.far {
            let pc = at as i64 + 4; // what s_getpc_b64 returns
            let lit = if backward { pc - self.labels[l] as i64 } else { self.labels[l] as i64 - pc };
            code[at + 8..at + 12].copy_from_slice(&(lit as u32).to_le_bytes());
        }
        let mut insts = Vec::new();
        let mut off = 0;
        while off + 4 <= code.len() {
            let d = rdna2::decode(&code[off..], off as u64).unwrap();
            off += d.len;
            insts.push(d);
        }
        rdna2::resolve_far_jumps(&mut insts);
        for &(at, l) in &self.branches {
            let d = insts.iter().find(|d| d.pc == at as u64).unwrap();
            assert_eq!(rdna2::branch_target(d), Some(self.labels[l] as u64), "branch at {at:#x} to {l}");
        }
        for &(at, l, _) in &self.far {
            let d = insts.iter().find(|d| d.pc == at as u64 + 16).unwrap();
            assert_eq!(d.far_target, Some(self.labels[l] as u64), "far jump at {at:#x} to {l}");
        }
        code
    }
}

/// Code as flat memory from address 0 (where `runner::run_program` puts it), then `extra`.
fn code_mem(code: &[u8], extra: usize) -> Vec<u32> {
    let mut m: Vec<u32> = code.chunks(4).map(|c| {
        let mut b = [0u8; 4];
        b[..c.len()].copy_from_slice(c);
        u32::from_le_bytes(b)
    }).collect();
    m.resize(m.len() + extra, 0);
    m
}

/// A value per lane that differs in every byte (a multiplicative hash of the lane).
fn hash(l: usize, salt: u32) -> u32 {
    (l as u32 ^ salt).wrapping_mul(0x9E37_79B9).rotate_left(13) ^ salt.wrapping_mul(0x85EB_CA6B)
}

// ---------------------------------------------------------------------------------------------
// programs

/// s_cbranch_scc1 to the next instruction: a program without branches gets `translate`'s form
/// from `translate_compact`; with this one it gets the compact form.
fn branch_first(a: &mut Asm) -> &mut Asm {
    a.br("S_CBRANCH_SCC1", "first").label("first")
}

/// The compact form is used (`Dispatch::of` reads its dispatch loop): two blocks at least.
fn dispatched(d: &Dispatch, _: &Options) {
    assert!(d.blocks() >= 2, "{} blocks", d.blocks());
}

/// Nested exec-masked regions: s_and_saveexec_b64 / s_cbranch_execz / s_or_b64 restore, three
/// deep, with an else half (s_xor_b64 exec), a region no lane enters, and one only lanes 48..63
/// enter (in the split form: one half has no lane in it).
fn execz_nest() -> Vec<u8> {
    let mut a = Asm::default();
    a.i("V_MOV_B32", "ENC_VOP1", &[("VDST", 1), ("SRC0", 128)], None)
        // if (lane < 40)
        .i("V_CMP_GT_U32", "ENC_VOPC", &[("SRC0", 128 + 40), ("VSRC1", 0)], None)
        .i("S_AND_SAVEEXEC_B64", "ENC_SOP1", &[("SDST", 2), ("SSRC0", VCC)], None)
        .br("S_CBRANCH_EXECZ", "else1")
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 1), ("SRC0", 129), ("VSRC1", 1)], None)
        //   if (lane > 10)
        .i("V_CMP_LT_U32", "ENC_VOPC", &[("SRC0", 138), ("VSRC1", 0)], None)
        .i("S_AND_SAVEEXEC_B64", "ENC_SOP1", &[("SDST", 4), ("SSRC0", VCC)], None)
        .br("S_CBRANCH_EXECZ", "out2")
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 1), ("SRC0", 130), ("VSRC1", 1)], None)
        .i("S_ADD_U32", "ENC_SOP2", &[("SDST", 20), ("SSRC0", 20), ("SSRC1", 129)], None)
        //     if (lane == 1000): nobody
        .i("V_CMP_EQ_U32", "ENC_VOPC", &[("SRC0", 255), ("VSRC1", 0)], Some(1000))
        .i("S_AND_SAVEEXEC_B64", "ENC_SOP1", &[("SDST", 6), ("SSRC0", VCC)], None)
        .br("S_CBRANCH_EXECZ", "out3")
        .i("V_MOV_B32", "ENC_VOP1", &[("VDST", 1), ("SRC0", 255)], Some(0x55))
        .i("S_MOV_B32", "ENC_SOP1", &[("SDST", 21), ("SSRC0", 129)], None)
        .label("out3")
        .i("S_OR_B64", "ENC_SOP2", &[("SDST", EXEC), ("SSRC0", EXEC), ("SSRC1", 6)], None)
        .label("out2")
        .i("S_OR_B64", "ENC_SOP2", &[("SDST", EXEC), ("SSRC0", EXEC), ("SSRC1", 4)], None)
        // else (lanes 40..63)
        .label("else1")
        .i("S_XOR_B64", "ENC_SOP2", &[("SDST", EXEC), ("SSRC0", EXEC), ("SSRC1", 2)], None)
        .br("S_CBRANCH_EXECZ", "out1")
        .i("V_MUL_LO_U32", "ENC_VOP3", &[("VDST", 1), ("SRC0", V), ("SRC1", 255)], Some(0x0101_0101))
        //   if (lane >= 48)
        .i("V_CMP_LE_U32", "ENC_VOPC", &[("SRC0", 128 + 48), ("VSRC1", 0)], None)
        .i("S_AND_SAVEEXEC_B64", "ENC_SOP1", &[("SDST", 8), ("SSRC0", VCC)], None)
        .br("S_CBRANCH_EXECZ", "out4")
        .i("V_XOR_B32", "ENC_VOP2", &[("VDST", 1), ("SRC0", 255), ("VSRC1", 1)], Some(0xA5A5_0000))
        .i("S_ADD_U32", "ENC_SOP2", &[("SDST", 22), ("SSRC0", 22), ("SSRC1", 135)], None)
        .label("out4")
        .i("S_OR_B64", "ENC_SOP2", &[("SDST", EXEC), ("SSRC0", EXEC), ("SSRC1", 8)], None)
        .label("out1")
        .i("S_OR_B64", "ENC_SOP2", &[("SDST", EXEC), ("SSRC0", EXEC), ("SSRC1", 2)], None)
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 2), ("SRC0", 20), ("VSRC1", 1)], None)
        .end()
}

/// A divergent loop with a backward branch, as the console compiler writes it: each lane runs
/// `v5 = lane & 7` iterations; the loop leaves through s_cbranch_execz and restores exec.
/// Then a uniform scalar loop (s_cbranch_scc1 back).
fn backward_loops() -> Vec<u8> {
    let mut a = Asm::default();
    a.i("V_MOV_B32", "ENC_VOP1", &[("VDST", 4), ("SRC0", 128)], None)
        .i("V_AND_B32", "ENC_VOP2", &[("VDST", 5), ("SRC0", 135), ("VSRC1", 0)], None)
        .i("S_MOV_B64", "ENC_SOP1", &[("SDST", 10), ("SSRC0", EXEC)], None)
        .label("vloop")
        .i("V_CMP_NE_U32", "ENC_VOPC", &[("SRC0", 128), ("VSRC1", 5)], None)
        .i("S_AND_B64", "ENC_SOP2", &[("SDST", EXEC), ("SSRC0", EXEC), ("SSRC1", VCC)], None)
        .br("S_CBRANCH_EXECZ", "vdone")
        .i("V_MUL_LO_U32", "ENC_VOP3", &[("VDST", 4), ("SRC0", V + 4), ("SRC1", 131)], None)
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 4), ("SRC0", V + 5), ("VSRC1", 4)], None)
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 5), ("SRC0", 193), ("VSRC1", 5)], None)
        .i("S_ADD_U32", "ENC_SOP2", &[("SDST", 12), ("SSRC0", 12), ("SSRC1", 129)], None)
        .br("S_BRANCH", "vloop")
        .label("vdone")
        .i("S_MOV_B64", "ENC_SOP1", &[("SDST", EXEC), ("SSRC0", 10)], None)
        .i("S_MOV_B32", "ENC_SOP1", &[("SDST", 0), ("SSRC0", 128)], None)
        .i("S_MOV_B32", "ENC_SOP1", &[("SDST", 1), ("SSRC0", 138)], None)
        .label("sloop")
        .i("S_ADD_U32", "ENC_SOP2", &[("SDST", 0), ("SSRC0", 0), ("SSRC1", 1)], None)
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 6), ("SRC0", 1), ("VSRC1", 6)], None)
        .i("S_SUB_U32", "ENC_SOP2", &[("SDST", 1), ("SSRC0", 1), ("SSRC1", 129)], None)
        .i("S_CMP_LG_U32", "ENC_SOPC", &[("SSRC0", 1), ("SSRC1", 128)], None)
        .br("S_CBRANCH_SCC1", "sloop")
        .end()
}

/// The same instruction sequence (`X`: VALU + scalar work ending in `s_cbranch_scc1` over the
/// next instruction) at five addresses: its successors differ at each (the branch is relative),
/// one copy is in a loop, one is reached only by an s_branch, and one ends the program.
fn repeated_blocks() -> Vec<u8> {
    fn x(a: &mut Asm) -> &mut Asm {
        a.i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 2), ("SRC0", V), ("VSRC1", 2)], None)
            .i("V_MUL_LO_U32", "ENC_VOP3", &[("VDST", 3), ("SRC0", V + 2), ("SRC1", V + 3)], None)
            .i("S_ADD_U32", "ENC_SOP2", &[("SDST", 10), ("SSRC0", 10), ("SSRC1", 131)], None)
            .i("S_CMP_GT_U32", "ENC_SOPC", &[("SSRC0", 10), ("SSRC1", 135)], None)
            .i("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 1)], None) // over the next instruction
    }
    let mut a = Asm::default();
    a.i("S_MOV_B32", "ENC_SOP1", &[("SDST", 10), ("SSRC0", 128)], None)
        .i("S_MOV_B32", "ENC_SOP1", &[("SDST", 11), ("SSRC0", 133)], None)
        .label("loop");
    x(&mut a).i("V_OR_B32", "ENC_VOP2", &[("VDST", 4), ("SRC0", 129), ("VSRC1", 4)], None);
    x(&mut a).i("V_OR_B32", "ENC_VOP2", &[("VDST", 4), ("SRC0", 130), ("VSRC1", 4)], None);
    x(&mut a).i("V_OR_B32", "ENC_VOP2", &[("VDST", 4), ("SRC0", 132), ("VSRC1", 4)], None);
    a.i("S_SUB_U32", "ENC_SOP2", &[("SDST", 11), ("SSRC0", 11), ("SSRC1", 129)], None)
        .i("S_CMP_LG_U32", "ENC_SOPC", &[("SSRC0", 11), ("SSRC1", 128)], None)
        .br("S_CBRANCH_SCC1", "loop")
        .i("S_MOV_B32", "ENC_SOP1", &[("SDST", 10), ("SSRC0", 128)], None);
    x(&mut a).br("S_BRANCH", "last").i("V_OR_B32", "ENC_VOP2", &[("VDST", 4), ("SRC0", 136), ("VSRC1", 4)], None).label("last");
    x(&mut a).i("V_MOV_B32", "ENC_VOP1", &[("VDST", 5), ("SRC0", 10)], None);
    a.end()
}

/// `repeated_blocks` in the compact form: fewer cases than blocks, and one case runs several.
fn repeated_shape(d: &Dispatch, _: &Options) {
    assert!(d.cases.len() < d.blocks(), "{} cases, {} blocks", d.cases.len(), d.blocks());
    assert!((0..d.cases.len()).any(|c| d.uses(c) >= 3), "table {:?}", d.class);
}

/// Far jumps (s_getpc_b64, add / sub of a literal, s_setpc_b64): a backward one closes a loop,
/// a forward one jumps over code that must not run.
fn far_jumps() -> Vec<u8> {
    let mut a = Asm::default();
    a.i("S_MOV_B32", "ENC_SOP1", &[("SDST", 20), ("SSRC0", 133)], None)
        .label("top")
        .i("S_ADD_U32", "ENC_SOP2", &[("SDST", 21), ("SSRC0", 21), ("SSRC1", 20)], None)
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 1), ("SRC0", 20), ("VSRC1", 1)], None)
        .i("S_SUB_U32", "ENC_SOP2", &[("SDST", 20), ("SSRC0", 20), ("SSRC1", 129)], None)
        .i("S_CMP_EQ_U32", "ENC_SOPC", &[("SSRC0", 20), ("SSRC1", 128)], None)
        .br("S_CBRANCH_SCC1", "out")
        .far_jump("top", true)
        .label("out")
        .far_jump("skip", false)
        .i("V_MOV_B32", "ENC_VOP1", &[("VDST", 2), ("SRC0", 135)], None) // jumped over
        .i("S_MOV_B32", "ENC_SOP1", &[("SDST", 22), ("SSRC0", 135)], None)
        .label("skip")
        .i("V_MOV_B32", "ENC_VOP1", &[("VDST", 3), ("SRC0", 137)], None)
        .end()
}

/// `far_jumps` in the compact form: it has no s_branch, so its two unconditional jumps are the
/// s_setpc_b64s, each in a case with its s_getpc_b64 (`s4 = <pc>`): back one block, forward two.
fn far_jumps_shape(d: &Dispatch, _: &Options) {
    for (l, what) in [("            xcl_nb = xcl_b - 1u;", "backward"), ("            xcl_nb = xcl_b + 2u;", "forward")] {
        let c = d.with_line(l);
        assert_eq!(c.len(), 1, "{what} far jump: cases {c:?}");
        assert!(d.cases[c[0]].1.contains("s4 = 0x"), "{what} far jump:\n{}", d.cases[c[0]].1);
    }
}

/// `far_jumps` ran its loop five times (s21 = 5 + 4 + 3 + 2 + 1) and skipped the two moves
/// (lanes of the wave: 64 for wave64, 32 for wave32).
fn far_jumps_ran(r: &[u8], opt: &Options) {
    assert_eq!((dword(r, 21), dword(r, 22)), (15, 0x33), "s21, s22");
    for l in 0..if opt.wave64 { 64 } else { 32 } {
        assert_eq!((dword(r, vgpr_slot(l, 2)), dword(r, vgpr_slot(l, 3))), (0x22, 9), "lane {l} v2, v3");
    }
}

/// v_mul_lo_u32 with VGPR, SGPR, inline-constant and literal operands (low 32 bits of the
/// product: wraps), next to v_mul_hi_u32 / v_mul_u32_u24. After `branch_first`.
fn mul_lo() -> Vec<u8> {
    let mut a = Asm::default();
    branch_first(&mut a)
        .i("V_MUL_LO_U32", "ENC_VOP3", &[("VDST", 10), ("SRC0", V + 1), ("SRC1", V + 2)], None)
        .i("V_MUL_LO_U32", "ENC_VOP3", &[("VDST", 11), ("SRC0", 8), ("SRC1", V + 1)], None)
        .i("V_MUL_LO_U32", "ENC_VOP3", &[("VDST", 12), ("SRC0", V + 2), ("SRC1", 193)], None)
        .i("V_MUL_LO_U32", "ENC_VOP3", &[("VDST", 13), ("SRC0", V + 1), ("SRC1", 255)], Some(0xDEAD_BEEF))
        .i("V_MUL_LO_U32", "ENC_VOP3", &[("VDST", 14), ("SRC0", V + 10), ("SRC1", V + 10)], None)
        .i("V_MUL_HI_U32", "ENC_VOP3", &[("VDST", 15), ("SRC0", V + 1), ("SRC1", V + 2)], None)
        .i("V_MUL_U32_U24", "ENC_VOP2", &[("VDST", 16), ("SRC0", V + 1), ("VSRC1", 2)], None)
        .end()
}

/// buffer_load_format_x of twelve formats (V# k at s[12 + 4k], format field in word 3), lanes
/// indexed with a stride of 4: lanes past num_records read 0. Also an unaligned offset. After
/// `branch_first`.
const LOAD_X_FORMATS: [u32; 12] = [1, 2, 4, 5, 7, 13, 20, 22, 29, 36, 50, 56];

fn load_format_x() -> Vec<u8> {
    let mut a = Asm::default();
    branch_first(&mut a);
    for k in 0..LOAD_X_FORMATS.len() as u64 {
        a.i("BUFFER_LOAD_FORMAT_X", "ENC_MUBUF", &[("VDATA", 20 + k), ("VADDR", 0), ("SRSRC", 3 + k), ("SOFFSET", 128), ("IDXEN", 1)], None);
    }
    a.i("BUFFER_LOAD_FORMAT_X", "ENC_MUBUF", &[("VDATA", 40), ("VADDR", 0), ("SRSRC", 3 + 7), ("SOFFSET", 128), ("IDXEN", 1), ("OFFSET", 2)], None)
        .i("BUFFER_LOAD_FORMAT_X", "ENC_MUBUF", &[("VDATA", 41), ("VADDR", 1), ("SRSRC", 3), ("SOFFSET", 128), ("OFFEN", 1)], None)
        .end()
}

/// Inputs of `load_format_x` / `load_x_repeated`, the data at `base` (0: flat memory; a window
/// address for `arena_both`, which copies `mem` there).
fn load_format_x_inputs(base: u64) -> Inputs {
    let ident = 4 | 5 << 3 | 6 << 6 | 7 << 9;
    let mem: Vec<u32> = (0..1024).map(|i| hash(i, 0x5EED)).collect();
    let mut st = Inputs::lanes(mem).vgpr(1, |l| 3 * l as u32);
    for (k, &fmt) in LOAD_X_FORMATS.iter().enumerate() {
        // base + 0x100 + 0x100·k, stride 4, 48 records
        let at = base + 0x100 + 0x100 * k as u64;
        st = st.sgprs(12 + 4 * k, &[at as u32, (at >> 32) as u32 & 0xFFFF | 4 << 16, 48, ident | fmt << 12]);
    }
    st
}

/// buffer_load_format_x at several addresses: the same block (one V#, a load, an add, a branch
/// over the next instruction) four times under different exec masks, and once with another
/// V# (16_16_FLOAT), each entered after an s_cbranch_execz.
fn load_x_repeated() -> Vec<u8> {
    fn l(a: &mut Asm, srsrc: u64) -> &mut Asm {
        a.i("BUFFER_LOAD_FORMAT_X", "ENC_MUBUF", &[("VDATA", 20), ("VADDR", 0), ("SRSRC", srsrc), ("SOFFSET", 128), ("IDXEN", 1)], None)
            .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 21), ("SRC0", V + 20), ("VSRC1", 21)], None)
            .i("S_ADD_U32", "ENC_SOP2", &[("SDST", 70), ("SSRC0", 70), ("SSRC1", 129)], None)
            .i("S_CMP_GT_U32", "ENC_SOPC", &[("SSRC0", 70), ("SSRC1", 130)], None)
            .i("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 1)], None)
    }
    let mut a = Asm::default();
    a.i("S_MOV_B64", "ENC_SOP1", &[("SDST", 72), ("SSRC0", EXEC)], None);
    let masks: [(u32, u32, &'static str); 5] = [(u32::MAX, u32::MAX, "c0"), (0, 0xFFFF_0000, "c1"), (0x0000_FFFF, 0, "c2"), (0xF0F0_F0F0, 0x0F0F_0F0F, "c3"), (0, 0, "c4")];
    for (k, &(lo, hi, end)) in masks.iter().enumerate() {
        a.i("S_MOV_B32", "ENC_SOP1", &[("SDST", EXEC), ("SSRC0", 255)], Some(lo))
            .i("S_MOV_B32", "ENC_SOP1", &[("SDST", EXEC + 1), ("SSRC0", 255)], Some(hi))
            .br("S_CBRANCH_EXECZ", end);
        // the fourth copy loads through another V# (srsrc 3 + 8: 16_16_FLOAT)
        l(&mut a, if k == 3 { 3 + 8 } else { 3 + 7 }).i("V_OR_B32", "ENC_VOP2", &[("VDST", 22), ("SRC0", 128 + 1 + k as u64), ("VSRC1", 22)], None).label(end);
    }
    a.i("S_MOV_B64", "ENC_SOP1", &[("SDST", EXEC), ("SSRC0", 72)], None);
    a.end()
}

/// `load_x_repeated` in the compact form: each load is a block of its own (one case for the
/// four through s[40:43], one for s[44:47]), and the block after it, the same in all five
/// copies, is one case.
fn load_x_shape(d: &Dispatch, _: &Options) {
    assert!(d.cases.len() < d.blocks(), "{} cases, {} blocks", d.cases.len(), d.blocks());
    let loads = d.with("FMT_INFO[");
    assert_eq!(loads.len(), 2, "cases with a buffer_load_format_x: {loads:?}");
    for &c in &loads {
        let text = &d.cases[c].1;
        assert_eq!(text.matches("FMT_INFO[").count(), 1, "one load in case {c}:\n{text}");
        assert!(!text.contains("v21 = ") && !text.contains("xcl_nb"), "case {c} is the load alone:\n{text}");
    }
    let by = |s: &str| loads.iter().copied().find(|&c| d.cases[c].1.contains(s)).unwrap_or_else(|| panic!("no load through {s}"));
    assert_eq!((d.uses(by("(s43 >> 12)")), d.uses(by("(s47 >> 12)"))), (4, 1), "blocks per load case");
    assert_eq!(d.uses(d.only("v21 = ")), 5, "the block after the load");
}

/// Cross-lane work (a v_cmp ballot into an SGPR pair and its s_bcnt1, v_readfirstlane_b32,
/// v_readlane_b32 of lanes 5 and 40) in one block repeated under four exec masks: lanes 32..63
/// only (the split form's first active lane is in the upper half), the low half only, every
/// lane, and alternating lanes. Each copy is entered after an s_cbranch_execz and ends in a
/// branch over the next instruction.
fn exchange_repeated() -> Vec<u8> {
    fn e(a: &mut Asm) -> &mut Asm {
        a.i("V_CMP_GT_U32", "ENC_VOP3", &[("VDST", 20), ("SRC0", V + 1), ("SRC1", V)], None)
            .i("S_BCNT1_I32_B64", "ENC_SOP1", &[("SDST", 22), ("SSRC0", 20)], None)
            .i("V_READFIRSTLANE_B32", "ENC_VOP1", &[("VDST", 23), ("SRC0", V + 2)], None)
            .i("V_READLANE_B32", "ENC_VOP3", &[("VDST", 24), ("SRC0", V + 3), ("SRC1", 133)], None)
            .i("V_READLANE_B32", "ENC_VOP3", &[("VDST", 25), ("SRC0", V + 3), ("SRC1", 255)], Some(40))
            .i("S_ADD_U32", "ENC_SOP2", &[("SDST", 26), ("SSRC0", 26), ("SSRC1", 22)], None)
            .i("S_XOR_B32", "ENC_SOP2", &[("SDST", 26), ("SSRC0", 26), ("SSRC1", 23)], None)
            .i("S_ADD_U32", "ENC_SOP2", &[("SDST", 26), ("SSRC0", 26), ("SSRC1", 24)], None)
            .i("S_ADD_U32", "ENC_SOP2", &[("SDST", 26), ("SSRC0", 26), ("SSRC1", 25)], None)
            .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 2), ("SRC0", 26), ("VSRC1", 2)], None)
            .i("V_XOR_B32", "ENC_VOP2", &[("VDST", 1), ("SRC0", 23), ("VSRC1", 1)], None)
            .i("S_BITCMP1_B32", "ENC_SOPC", &[("SSRC0", 26), ("SSRC1", 128)], None)
            .i("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 1)], None)
    }
    let mut a = Asm::default();
    a.i("S_MOV_B64", "ENC_SOP1", &[("SDST", 30), ("SSRC0", EXEC)], None);
    let masks: [(u32, u32, &'static str); 4] = [(0, 0xFFFF_FFFF, "x0"), (0xFFFF_FFFF, 0, "x1"), (u32::MAX, u32::MAX, "x2"), (0x5555_5555, 0xAAAA_AAAA, "x3")];
    for (k, &(lo, hi, end)) in masks.iter().enumerate() {
        a.i("S_MOV_B32", "ENC_SOP1", &[("SDST", EXEC), ("SSRC0", 255)], Some(lo))
            .i("S_MOV_B32", "ENC_SOP1", &[("SDST", EXEC + 1), ("SSRC0", 255)], Some(hi))
            .br("S_CBRANCH_EXECZ", end);
        e(&mut a).i("V_OR_B32", "ENC_VOP2", &[("VDST", 4), ("SRC0", 128 + 1 + k as u64), ("VSRC1", 4)], None).label(end);
    }
    a.i("S_MOV_B64", "ENC_SOP1", &[("SDST", EXEC), ("SSRC0", 30)], None);
    a.end()
}

/// `exchange_repeated` in the compact form: the exchange block (the only one writing s25) is
/// one case for its four copies; in the split wave64 form its ballot and lane reads go through
/// the halves' exchange (`xcl_x_ballot`, `xcl_x_rla_uniform`).
fn exchange_shape(d: &Dispatch, opt: &Options) {
    assert!(d.cases.len() < d.blocks(), "{} cases, {} blocks", d.cases.len(), d.blocks());
    let c = d.only("s25 = ");
    assert_eq!(d.uses(c), 4, "blocks of the exchange case");
    let text = &d.cases[c].1;
    if opt.wave64 && opt.split_wave64 {
        assert_eq!((text.matches("= xcl_x_ballot(and(").count(), text.matches("= xcl_x_rla_uniform(").count()), (1, 3), "{text}");
    }
}

/// s_endpgm before the last block: s30 picks the path. 1: the first s_endpgm; 0: `tail`, which
/// branches back to `mid` (its own s_endpgm); 2: `tail` and on to the end. Lanes below 20 also
/// take an execz-guarded block in `mid`.
fn endpgm_early() -> Vec<u8> {
    let mut a = Asm::default();
    a.i("V_MOV_B32", "ENC_VOP1", &[("VDST", 1), ("SRC0", 128)], None)
        .i("S_CMP_EQ_U32", "ENC_SOPC", &[("SSRC0", 30), ("SSRC1", 129)], None)
        .br("S_CBRANCH_SCC0", "tail")
        .i("V_MOV_B32", "ENC_VOP1", &[("VDST", 1), ("SRC0", 129)], None)
        .i("S_MOV_B32", "ENC_SOP1", &[("SDST", 31), ("SSRC0", 129)], None)
        .i("S_ENDPGM", "ENC_SOPP", &[], None)
        .label("mid")
        .i("V_CMP_GT_U32", "ENC_VOPC", &[("SRC0", 128 + 20), ("VSRC1", 0)], None)
        .i("S_AND_SAVEEXEC_B64", "ENC_SOP1", &[("SDST", 2), ("SSRC0", VCC)], None)
        .br("S_CBRANCH_EXECZ", "mid_out")
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 1), ("SRC0", 135), ("VSRC1", 1)], None)
        .label("mid_out")
        .i("S_OR_B64", "ENC_SOP2", &[("SDST", EXEC), ("SSRC0", EXEC), ("SSRC1", 2)], None)
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 1), ("SRC0", 131), ("VSRC1", 1)], None)
        .i("S_MOV_B32", "ENC_SOP1", &[("SDST", 31), ("SSRC0", 131)], None)
        .i("S_ENDPGM", "ENC_SOPP", &[], None)
        .label("tail")
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 1), ("SRC0", V), ("VSRC1", 1)], None)
        .i("S_ADD_U32", "ENC_SOP2", &[("SDST", 32), ("SSRC0", 32), ("SSRC1", 133)], None)
        .i("S_CMP_EQ_U32", "ENC_SOPC", &[("SSRC0", 30), ("SSRC1", 128)], None)
        .br("S_CBRANCH_SCC1", "mid")
        .i("S_MOV_B32", "ENC_SOP1", &[("SDST", 31), ("SSRC0", 132)], None)
        .end()
}

/// `endpgm_early` in the compact form: two blocks before the last one end the program.
fn endpgm_shape(d: &Dispatch, _: &Options) {
    let ends = d.with_line("            xcl_nb = 0xFFFFFFFFu;");
    let n = d.blocks();
    assert!(ends.contains(&d.class[n - 1]), "the last block ends the program");
    assert_eq!(d.class[..n - 1].iter().filter(|c| ends.contains(c)).count(), 2, "early ends: table {:?}, cases {ends:?}", d.class);
}

/// Memory windows with branches: a scalar loop (two iterations, s0) around four copies of one
/// block body, copy k through window k (s[20:21] = s[40 + 2k : 41 + 2k], the V# s[24:27] =
/// s[48 + 4k : 51 + 4k]) under its own exec mask, entered after an s_cbranch_execz. The body:
/// global_load_dword from the lane's dword at +v42, global_atomic_add of it at +v41 (returns
/// the old value), buffer_load_format_x through the V#, their sum stored back at +v42 (read by
/// the next iteration), s22 counted; it ends in a branch over the copy's own v_or.
fn window_loop() -> Vec<u8> {
    let glbl = "ENC_FLAT_GLBL";
    fn body<'a>(a: &'a mut Asm, glbl: &str) -> &'a mut Asm {
        a.i("GLOBAL_LOAD_DWORD", glbl, &[("VDST", 30), ("ADDR", 42), ("SADDR", 20), ("OFFSET", 0)], None)
            .i("GLOBAL_ATOMIC_ADD", glbl, &[("VDST", 31), ("ADDR", 41), ("DATA", 30), ("SADDR", 20), ("OFFSET", 0), ("GLC", 1)], None)
            .i("BUFFER_LOAD_FORMAT_X", "ENC_MUBUF", &[("VDATA", 32), ("VADDR", 0), ("SRSRC", 6), ("SOFFSET", 128), ("IDXEN", 1)], None)
            .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 33), ("SRC0", V + 30), ("VSRC1", 31)], None)
            .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 33), ("SRC0", V + 32), ("VSRC1", 33)], None)
            .i("GLOBAL_STORE_DWORD", glbl, &[("ADDR", 42), ("DATA", 33), ("SADDR", 20), ("OFFSET", 0)], None)
            .i("S_ADD_U32", "ENC_SOP2", &[("SDST", 22), ("SSRC0", 22), ("SSRC1", 129)], None)
            .i("S_BITCMP1_B32", "ENC_SOPC", &[("SSRC0", 22), ("SSRC1", 128)], None)
            .i("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 1)], None)
    }
    let mut a = Asm::default();
    a.i("S_MOV_B64", "ENC_SOP1", &[("SDST", 2), ("SSRC0", EXEC)], None)
        .i("S_MOV_B32", "ENC_SOP1", &[("SDST", 0), ("SSRC0", 130)], None)
        .label("loop");
    let masks: [(u32, u32, &'static str); 4] = [(u32::MAX, u32::MAX, "w0"), (0, u32::MAX, "w1"), (0x5555_5555, 0xAAAA_AAAA, "w2"), (u32::MAX, 0, "w3")];
    for (k, &(lo, hi, end)) in masks.iter().enumerate() {
        let k = k as u64;
        a.i("S_MOV_B64", "ENC_SOP1", &[("SDST", 20), ("SSRC0", 40 + 2 * k)], None)
            .i("S_MOV_B64", "ENC_SOP1", &[("SDST", 24), ("SSRC0", 48 + 4 * k)], None)
            .i("S_MOV_B64", "ENC_SOP1", &[("SDST", 26), ("SSRC0", 50 + 4 * k)], None)
            .i("S_MOV_B32", "ENC_SOP1", &[("SDST", EXEC), ("SSRC0", 255)], Some(lo))
            .i("S_MOV_B32", "ENC_SOP1", &[("SDST", EXEC + 1), ("SSRC0", 255)], Some(hi))
            .br("S_CBRANCH_EXECZ", end);
        body(&mut a, glbl).i("V_OR_B32", "ENC_VOP2", &[("VDST", 34), ("SRC0", 129 + k), ("VSRC1", 34)], None).label(end);
    }
    a.i("S_MOV_B64", "ENC_SOP1", &[("SDST", EXEC), ("SSRC0", 2)], None)
        .i("S_SUB_U32", "ENC_SOP2", &[("SDST", 0), ("SSRC0", 0), ("SSRC1", 129)], None)
        .i("S_CMP_LG_U32", "ENC_SOPC", &[("SSRC0", 0), ("SSRC1", 128)], None)
        .br("S_CBRANCH_SCC1", "loop")
        .end()
}

/// Inputs of `window_loop` in a four-window `arena`: window k's base at s[40 + 2k] (+0x2000),
/// its V# at s[48 + 4k] (+0x4000, stride 4, 64 records, 32_FLOAT); per-lane offsets v42
/// (+4·lane, loaded and stored) and v41 (+0x800 + 4·lane, the atomic: a dword per lane, so its
/// return value does not depend on the order of the lanes or of the split form's halves).
fn window_loop_inputs(arena: &crate::gpu::uma::Uma) -> Inputs {
    let ident = 4 | 5 << 3 | 6 << 6 | 7 << 9;
    let mut st = Inputs::lanes(Vec::new()).vgpr(41, |l| 0x800 + 4 * l as u32).vgpr(42, |l| 4 * l as u32);
    for k in 0..4 {
        let w = arena.window(k).0 as u64;
        let (base, vsharp) = (w + 0x2000, w + 0x4000);
        st = st
            .sgprs(40 + 2 * k, &[base as u32, (base >> 32) as u32])
            .sgprs(48 + 4 * k, &[vsharp as u32, (vsharp >> 32) as u32 & 0xFFFF | 4 << 16, 64, ident | 22 << 12]);
    }
    st
}

/// `window_loop` in the compact form: not `translate`'s HLSL; fewer cases than blocks; the
/// buffer_load_format_x a case of its own run by the four copies, and the body's last part
/// (the store and the branch) one case for the four.
fn window_loop_shape(d: &Dispatch, _: &Options) {
    assert!(d.cases.len() < d.blocks(), "{} cases, {} blocks", d.cases.len(), d.blocks());
    let load = d.only("FMT_INFO[");
    assert!(!d.cases[load].1.contains("xcl_nb"), "the load alone:
{}", d.cases[load].1);
    assert_eq!(d.uses(load), 4, "blocks of the buffer_load_format_x case");
    assert_eq!(d.uses(d.only("v33 = ")), 4, "blocks of the body's last part");
}

/// Memory-window loads and stores (the `gpu_memory_windows_above_4_gib` program, windows 0
/// and 1); `four`: also a load from window 3 (v[14:15]) stored into window 2 (v[16:17]).
fn window_program(four: bool) -> Vec<u8> {
    let glbl = "ENC_FLAT_GLBL";
    let mut a = Asm::default();
    a.i("S_LOAD_DWORDX2", "ENC_SMEM", &[("SDATA", 20), ("SBASE", 1), ("OFFSET", 0)], None)
        .i("BUFFER_LOAD_DWORD", "ENC_MUBUF", &[("VDATA", 1), ("VADDR", 0), ("SRSRC", 2), ("SOFFSET", 128), ("IDXEN", 1)], None)
        .i("S_BUFFER_LOAD_DWORDX4", "ENC_SMEM", &[("SDATA", 24), ("SBASE", 4), ("OFFSET", 4)], None)
        .i("BUFFER_STORE_DWORD", "ENC_MUBUF", &[("VDATA", 2), ("VADDR", 0), ("SRSRC", 3), ("SOFFSET", 128), ("IDXEN", 1)], None)
        .i("GLOBAL_LOAD_DWORD", glbl, &[("VDST", 6), ("ADDR", 4), ("SADDR", 0x7D), ("OFFSET", 0)], None)
        .i("GLOBAL_STORE_DWORD", glbl, &[("ADDR", 8), ("DATA", 2), ("SADDR", 0x7D), ("OFFSET", 0)], None)
        .i("GLOBAL_ATOMIC_ADD", glbl, &[("VDST", 10), ("ADDR", 3), ("DATA", 11), ("SADDR", 16), ("OFFSET", 0), ("GLC", 1)], None);
    if four {
        a.i("GLOBAL_LOAD_DWORD", glbl, &[("VDST", 12), ("ADDR", 14), ("SADDR", 0x7D), ("OFFSET", 0)], None)
            .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 12), ("SRC0", V + 6), ("VSRC1", 12)], None)
            .i("GLOBAL_STORE_DWORD", glbl, &[("ADDR", 16), ("DATA", 12), ("SADDR", 0x7D), ("OFFSET", 0)], None);
    }
    a.end()
}

// ---------------------------------------------------------------------------------------------
// tests

/// Without a GPU: both forms of the small programs translate completely and DXC compiles them
/// (wave64 split and native, wave32; structured and not).
#[test]
fn compact_and_normal_forms_compile() {
    if super::dxc::location().is_err() {
        runner::skip_test("no DXC");
        return;
    }
    let programs = [
        ("execz nest", execz_nest()),
        ("repeated blocks", repeated_blocks()),
        ("far jumps", far_jumps()),
        ("exchange repeated", exchange_repeated()),
        ("buffer_load_format_x repeated", load_x_repeated()),
        ("s_endpgm early", endpgm_early()),
        ("window loop", window_loop()),
    ];
    for (what, code) in &programs {
        for (wave64, split) in [(true, true), (true, false), (false, false)] {
            for structured in [true, false] {
                let opt = Options { wave64, split_wave64: split, structured, ..Default::default() };
                let (normal, compact) = both_translations(code, &opt);
                compiled(&normal, &format!("{what} translate wave64={wave64} split={split} structured={structured}"));
                compiled(&compact, &format!("{what} translate_compact wave64={wave64} split={split} structured={structured}"));
                assert_eq!(normal.instructions, compact.instructions, "{what}: instruction count");
            }
        }
    }
}

#[test]
fn gpu_compact_execz_nest() {
    if !gpu_ready() {
        return;
    }
    let inputs = Inputs::lanes(Vec::new()).vgpr(1, |l| hash(l, 1));
    assert_shape("execz nest", &execz_nest(), true, dispatched);
    assert_same("execz nest", &execz_nest(), &inputs, true);
}

#[test]
fn gpu_compact_backward_loops() {
    if !gpu_ready() {
        return;
    }
    let inputs = Inputs::lanes(Vec::new()).vgpr(6, |l| hash(l, 6));
    assert_shape("backward loops", &backward_loops(), true, dispatched);
    assert_same("backward loops", &backward_loops(), &inputs, true);
}

#[test]
fn gpu_compact_repeated_blocks() {
    if !gpu_ready() {
        return;
    }
    let inputs = Inputs::lanes(Vec::new()).vgpr(2, |l| hash(l, 2)).vgpr(3, |l| hash(l, 3) | 1);
    assert_shape("repeated blocks", &repeated_blocks(), true, repeated_shape);
    assert_same("repeated blocks", &repeated_blocks(), &inputs, true);
}

#[test]
fn gpu_compact_far_jumps() {
    if !gpu_ready() {
        return;
    }
    let inputs = Inputs::lanes(Vec::new()).vgpr(1, |l| hash(l, 1)).vgpr(2, |_| 0x22).sgprs(22, &[0x33]);
    assert_shape("far jumps", &far_jumps(), true, far_jumps_shape);
    for (form, opt, r) in assert_same("far jumps", &far_jumps(), &inputs, true) {
        println!("far jumps ({form}): checking s21, s22, v2, v3");
        far_jumps_ran(&r, &opt);
    }
}

#[test]
fn gpu_compact_v_mul_lo_u32() {
    if !gpu_ready() {
        return;
    }
    let inputs = Inputs::lanes(Vec::new()).vgpr(1, |l| hash(l, 1)).vgpr(2, |l| hash(l, 2)).sgprs(8, &[0x8000_0001]);
    assert_shape("v_mul_lo_u32", &mul_lo(), false, dispatched);
    assert_same("v_mul_lo_u32", &mul_lo(), &inputs, false);
}

#[test]
fn gpu_compact_buffer_load_format_x() {
    if !gpu_ready() {
        return;
    }
    assert_shape("buffer_load_format_x", &load_format_x(), false, dispatched);
    assert_same("buffer_load_format_x", &load_format_x(), &load_format_x_inputs(0), false);
}

/// The plain (unsplit) wave64 form needs 64-lane waves: it is in `forms` only where the GPU has
/// them, and this test says when it is not.
#[test]
fn gpu_compact_native_wave64_form() {
    if !gpu_ready() {
        return;
    }
    if runner::wave64_supported() != Ok(true) {
        runner::skip_test("no wave64");
        return;
    }
    let opt = Options { wave64: true, split_wave64: false, ..Default::default() };
    for (what, code, inputs) in [
        ("execz nest", execz_nest(), Inputs::lanes(Vec::new())),
        ("backward loops", backward_loops(), Inputs::lanes(Vec::new())),
        ("buffer_load_format_x", load_format_x(), load_format_x_inputs(0)),
    ] {
        let (a, b) = both_backends(&code, &opt, &inputs);
        let d = differences(&a, &b, slot);
        assert!(d.is_empty(), "{what} (wave64 native): differ:\n{}", d.join("\n"));
    }
}

/// Compute mode (the game's path: user SGPRs, group id, thread id, `runner::dispatch`), four
/// groups of 64 threads: the divergent loop, then a global store per thread. Memory compared.
#[test]
fn gpu_compact_compute_dispatch() {
    if !gpu_ready() {
        return;
    }
    // s0, s1 user SGPRs; s2 group id x; v0 thread id
    let mut a = Asm::default();
    a.i("V_MOV_B32", "ENC_VOP1", &[("VDST", 4), ("SRC0", 2)], None)
        .i("V_AND_B32", "ENC_VOP2", &[("VDST", 5), ("SRC0", 135), ("VSRC1", 0)], None)
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 5), ("SRC0", 2), ("VSRC1", 5)], None)
        .i("S_MOV_B64", "ENC_SOP1", &[("SDST", 10), ("SSRC0", EXEC)], None)
        .label("loop")
        .i("V_CMP_NE_U32", "ENC_VOPC", &[("SRC0", 128), ("VSRC1", 5)], None)
        .i("S_AND_B64", "ENC_SOP2", &[("SDST", EXEC), ("SSRC0", EXEC), ("SSRC1", VCC)], None)
        .br("S_CBRANCH_EXECZ", "done")
        .i("V_MUL_LO_U32", "ENC_VOP3", &[("VDST", 4), ("SRC0", V + 4), ("SRC1", 255)], Some(0x0100_0193))
        .i("V_XOR_B32", "ENC_VOP2", &[("VDST", 4), ("SRC0", V + 5), ("VSRC1", 4)], None)
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 5), ("SRC0", 193), ("VSRC1", 5)], None)
        .br("S_BRANCH", "loop")
        .label("done")
        .i("S_MOV_B64", "ENC_SOP1", &[("SDST", EXEC), ("SSRC0", 10)], None)
        .i("S_LSHL_B32", "ENC_SOP2", &[("SDST", 3), ("SSRC0", 2), ("SSRC1", 136)], None)
        .i("V_LSHLREV_B32", "ENC_VOP2", &[("VDST", 6), ("SRC0", 130), ("VSRC1", 0)], None)
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 6), ("SRC0", 3), ("VSRC1", 6)], None)
        .i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 6), ("SRC0", 0), ("VSRC1", 6)], None)
        .i("V_MOV_B32", "ENC_VOP1", &[("VDST", 7), ("SRC0", 128)], None)
        .i("GLOBAL_STORE_DWORD", "ENC_FLAT_GLBL", &[("ADDR", 6), ("DATA", 4), ("SADDR", 0x7D)], None);
    let code = a.end();
    let mut io = vec![0u32; 128];
    io[0] = 0x40; // store base
    for wave64 in [true, false] {
        let cs = CsInterface { threads: [64, 1, 1], user_sgprs: 2, tgid: [true, false, false], tg_size: false, tid_components: 1 };
        let opt = Options { mode: Mode::Compute(cs), wave64, split_wave64: wave64 && runner::split_wave64(), ..Default::default() };
        if wave64 && !opt.split_wave64 && runner::wave64_supported() != Ok(true) {
            continue;
        }
        let (normal, compact) = both_translations(&code, &opt);
        dispatched(&Dispatch::of(&compact), &opt);
        let mem = vec![0xFFFF_FFFFu32; 0x40 / 4 + 4 * 64];
        let a = runner::dispatch(&compiled(&normal, "translate"), [4, 1, 1], &io, &mem).unwrap();
        let b = runner::dispatch(&compiled(&compact, "translate_compact"), [4, 1, 1], &io, &mem).unwrap();
        let to = |m: &[u32]| m.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>();
        let d = differences(&to(&a), &to(&b), |i| format!("mem[{:#x}]", 4 * i));
        assert!(d.is_empty(), "compute (wave64={wave64}): differ:\n{}", d.join("\n"));
        assert!(a[0x40 / 4..].iter().all(|&w| w != 0xFFFF_FFFF), "compute (wave64={wave64}): every thread stored");
    }
}

/// Bytes of each UMA window `arena_both` sets up and compares.
const SPAN: usize = 0x10000;

/// `both_backends` through `runner::run_in_arena` (the `mem_windows` form): before each run the
/// first `SPAN` bytes of every window of `arena` are filled from a hash and `inputs.mem` is
/// copied to the start of window 0; result buffer, then every window's first `SPAN` bytes.
fn arena_both(code: &[u8], opt: &Options, inputs: &Inputs, arena: &crate::gpu::uma::Uma) -> (Vec<u8>, Vec<u8>) {
    let window = |k: usize| unsafe { std::slice::from_raw_parts_mut(arena.window(k).0 as *mut u32, SPAN / 4) };
    let (normal, compact) = both_translations(code, opt);
    let run = |t: &Translation, backend: &str| {
        let dxil = compiled(t, backend);
        for k in 0..arena.windows() {
            for (i, w) in window(k).iter_mut().enumerate() {
                *w = hash(i, 0xA0 + k as u32);
            }
        }
        window(0)[..inputs.mem.len()].copy_from_slice(&inputs.mem);
        let r = runner::run_in_arena(&dxil, &inputs.state, arena).unwrap_or_else(|e| panic!("{backend}: {e}"));
        let mut v: Vec<u8> = r.raw.iter().flat_map(|w| w.to_le_bytes()).collect();
        for k in 0..arena.windows() {
            v.extend(window(k).iter().flat_map(|x| x.to_le_bytes()));
        }
        v
    };
    (run(&normal, "translate"), run(&compact, "translate_compact"))
}

/// Name of dword `i` of an `arena_both` result.
fn arena_slot(i: usize) -> String {
    if i < KERNEL_RES_DWORDS {
        return slot(i);
    }
    let k = i - KERNEL_RES_DWORDS;
    format!("window {} +{:#x}", k / (SPAN / 4), 4 * (k % (SPAN / 4)))
}

fn assert_same_in_arena(what: &str, code: &[u8], opt: &Options, inputs: &Inputs, arena: &crate::gpu::uma::Uma, form: &str) {
    let (a, b) = arena_both(code, opt, inputs, arena);
    let d = differences(&a, &b, arena_slot);
    assert!(d.is_empty(), "{what} ({form}, {} windows): translate / translate_compact differ:\n{}", opt.mem_windows, d.join("\n"));
    println!("{what} ({form}, {} windows): {} result bytes identical", opt.mem_windows, a.len());
}

/// Memory windows (`mem_windows` 2 and 4, `runner::run_in_arena`): loads, stores and an atomic
/// in window 1 next to window 0; with 4, a load from window 3 stored into window 2. The program
/// has no branch, so `translate_compact` gives `translate`'s form: `gpu_compact_memory_windows_loop`
/// runs the compact one.
#[test]
fn gpu_compact_memory_windows() {
    if !gpu_ready() {
        return;
    }
    for windows in [2usize, 4] {
        let arena = crate::gpu::uma::Uma::with_windows(windows, 1 << 20).unwrap();
        let w = |k: usize| arena.window(k).0 as u64;
        let hi = |a: u64| (a >> 32) as u32;
        let (w0, w1) = (w(0), w(1));
        let mut st = Inputs::lanes(Vec::new())
            .sgprs(2, &[(w1 + 0x1000) as u32, hi(w1)])
            .sgprs(8, &[(w1 + 0x2000) as u32, hi(w1) & 0xFFFF | 4 << 16, 64, 0])
            .sgprs(12, &[(w0 + 0x3000) as u32, hi(w0) & 0xFFFF | 4 << 16, 64, 0])
            .sgprs(16, &[(w1 + 0x6000) as u32, hi(w1)])
            .vgpr(2, |l| 7 * l as u32)
            // the atomic's address: a dword per lane. On one dword, the split form's two halves'
            // atomics land in either order, in both backends, and the values they return differ
            .vgpr(3, |l| 4 * l as u32)
            .vgpr(4, |l| (w1 + 0x4000 + 4 * l as u64) as u32)
            .vgpr(5, |_| hi(w1))
            .vgpr(8, |l| (w1 + 0x5000 + 4 * l as u64) as u32)
            .vgpr(9, |_| hi(w1))
            .vgpr(11, |l| l as u32 + 1);
        if windows == 4 {
            let (w2, w3) = (w(2), w(3));
            st = st.vgpr(14, |l| (w3 + 0x7000 + 8 * l as u64) as u32).vgpr(15, |_| hi(w3)).vgpr(16, |l| (w2 + 0x8000 + 4 * l as u64) as u32).vgpr(17, |_| hi(w2));
        }
        let code = window_program(windows == 4);
        for (form, o) in forms(false) {
            assert_same_in_arena("memory windows", &code, &Options { mem_windows: windows as u32, ..o }, &st, &arena, &form);
        }
    }
}

/// `window_loop` (branches, repeated blocks, loads, stores and atomics in four windows) in every
/// form of `forms` with `mem_windows: 4`: the compact form is used (its HLSL is not
/// `translate`'s), and both leave the same registers and windows. Every copy runs in both
/// iterations in the wave64 forms (s22 = 8).
#[test]
fn gpu_compact_memory_windows_loop() {
    if !gpu_ready() {
        return;
    }
    let arena = crate::gpu::uma::Uma::with_windows(4, 1 << 20).unwrap();
    let inputs = window_loop_inputs(&arena);
    let code = window_loop();
    for (form, o) in forms(true) {
        let opt = Options { mem_windows: 4, ..o };
        let (normal, compact) = both_translations(&code, &opt);
        assert_ne!(normal.hlsl, compact.hlsl, "{form}: the compact form is used");
        let d = Dispatch::of(&compact);
        window_loop_shape(&d, &opt);
        println!("window loop ({form}, 4 windows): compact form has {} cases for {} blocks", d.cases.len(), d.blocks());
        let (a, b) = arena_both(&code, &opt, &inputs, &arena);
        let diff = differences(&a, &b, arena_slot);
        assert!(diff.is_empty(), "window loop ({form}, 4 windows): translate / translate_compact differ:
{}", diff.join("
"));
        if opt.wave64 {
            assert_eq!(dword(&a, 22), 8, "window loop ({form}): s22");
            // every window's stored (+0x2000) and atomic (+0x2800) dwords changed (`arena_both`
            // fills window k with hash(i, 0xA0 + k))
            for k in 0..4 {
                for at in [0x2000, 0x2800] {
                    let changed = (at / 4..at / 4 + 64).filter(|&i| dword(&a, KERNEL_RES_DWORDS + k * SPAN / 4 + i) != hash(i, 0xA0 + k as u32)).count();
                    assert!(changed > 0, "window loop ({form}): window {k} +{at:#x} written");
                }
            }
        }
        println!("window loop ({form}, 4 windows): {} result bytes identical", a.len());
    }
}

/// The form titles run in: the wave64 forms of `forms` (split on a 32-lane GPU) with
/// `mem_windows: 4`, run in a four-window arena, for every program here (buffer_load_format_x
/// reads V#s in window 0).
#[test]
fn gpu_compact_programs_with_four_windows() {
    if !gpu_ready() {
        return;
    }
    let arena = crate::gpu::uma::Uma::with_windows(4, 1 << 20).unwrap();
    let w0 = arena.window(0).0 as u64;
    let lanes = || Inputs::lanes(Vec::new()).vgpr(1, |l| hash(l, 1)).vgpr(2, |l| hash(l, 2)).vgpr(3, |l| hash(l, 3) | 1);
    type Shape = Option<fn(&Dispatch, &Options)>;
    let programs: Vec<(&str, Vec<u8>, Inputs, Shape)> = vec![
        ("execz nest", execz_nest(), lanes(), Some(dispatched)),
        ("backward loops", backward_loops(), lanes(), Some(dispatched)),
        ("repeated blocks", repeated_blocks(), lanes(), Some(repeated_shape)),
        ("far jumps", far_jumps(), lanes(), Some(far_jumps_shape)),
        ("v_mul_lo_u32", mul_lo(), lanes().sgprs(8, &[0x8000_0001]), Some(dispatched)),
        ("exchange repeated", exchange_repeated(), lanes(), Some(exchange_shape)),
        ("s_endpgm early (s30 = 0)", endpgm_early(), lanes(), Some(endpgm_shape)),
        ("s_endpgm early (s30 = 2)", endpgm_early(), lanes().sgprs(30, &[2]), Some(endpgm_shape)),
        ("buffer_load_format_x", load_format_x(), load_format_x_inputs(w0), Some(dispatched)),
        ("buffer_load_format_x repeated", load_x_repeated(), load_format_x_inputs(w0), Some(load_x_shape)),
    ];
    let wave64: Vec<(String, Options)> = forms(true).into_iter().filter(|(_, o)| o.wave64).collect();
    assert!(!wave64.is_empty(), "gpu_ready: a wave64 form runs here");
    assert_eq!(wave64.iter().any(|(_, o)| o.split_wave64), runner::split_wave64(), "the split form runs where the adapter needs it");
    for (what, code, inputs, shape) in &programs {
        for (form, o) in &wave64 {
            let opt = Options { mem_windows: 4, ..o.clone() };
            if let Some(check) = shape {
                check(&Dispatch::of(&compact::translate_compact(code, &opt).unwrap()), &opt);
            }
            assert_same_in_arena(what, code, &opt, inputs, &arena, form);
        }
    }
}

/// Cross-lane exchanges in a repeated block (`exchange_repeated`).
#[test]
fn gpu_compact_exchange_repeated() {
    if !gpu_ready() {
        return;
    }
    let inputs = Inputs::lanes(Vec::new()).vgpr(1, |l| hash(l, 1) % 64).vgpr(2, |l| hash(l, 2)).vgpr(3, |l| hash(l, 3));
    assert_shape("exchange repeated", &exchange_repeated(), true, exchange_shape);
    assert_same("exchange repeated", &exchange_repeated(), &inputs, true);
}

/// buffer_load_format_x in a repeated block (`load_x_repeated`).
#[test]
fn gpu_compact_buffer_load_format_x_repeated() {
    if !gpu_ready() {
        return;
    }
    assert_shape("buffer_load_format_x repeated", &load_x_repeated(), true, load_x_shape);
    assert_same("buffer_load_format_x repeated", &load_x_repeated(), &load_format_x_inputs(0), true);
}

/// s_endpgm before the last block (`endpgm_early`), each of its three paths.
#[test]
fn gpu_compact_endpgm_before_last_block() {
    if !gpu_ready() {
        return;
    }
    assert_shape("s_endpgm early", &endpgm_early(), true, endpgm_shape);
    for path in 0..3u32 {
        let inputs = Inputs::lanes(Vec::new()).vgpr(1, |l| hash(l, 1)).sgprs(30, &[path]);
        // s31: 1 at the first s_endpgm, 3 at the second, 4 at the last
        let want = [3, 1, 4][path as usize];
        for (form, _, r) in assert_same(&format!("s_endpgm early (s30 = {path})"), &endpgm_early(), &inputs, true) {
            assert_eq!(dword(&r, 31), want, "s_endpgm early (s30 = {path}, {form}): s31");
        }
    }
}

/// `XCL_SAMPLE_BIG_CS=<file>`: a retail program's code (e.g. 56c0cd3b82a00c6e, 488 744 bytes),
/// run as a kernel through both backends: v0 = lane index, the code as memory from address 0
/// plus 64 KiB of zeros, every other input 0. Does nothing when the variable is unset.
/// DXC takes minutes on programs this size. A program the compact form refuses (e.g. its table
/// limit) is SKIPPED; when the driver cannot create `translate`'s pipeline (e.g. E_OUTOFMEMORY),
/// the A/B comparison is SKIPPED and the compact form must still compile and run.
#[test]
fn gpu_compact_sample_big_cs() {
    let Some(path) = std::env::var_os("XCL_SAMPLE_BIG_CS") else { return };
    if !gpu_ready() {
        return;
    }
    let code = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", std::path::Path::new(&path).display()));
    let wave64 = super::detect_wave64(&code);
    let opt = Options { wave64, split_wave64: wave64 && runner::split_wave64(), ..Default::default() };
    let t0 = std::time::Instant::now();
    let normal = translate(&code, &opt).unwrap_or_else(|e| panic!("translate: {e}"));
    let compact = match compact::translate_compact(&code, &opt) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("SKIPPED gpu_compact_sample_big_cs: compact form refused: {e}");
            return;
        }
    };
    eprintln!(
        "big CS: {} instructions, wave64 {wave64}, split {}; HLSL {} / {} bytes (translate / compact), {:.1} s",
        normal.instructions,
        opt.split_wave64,
        normal.hlsl.len(),
        compact.hlsl.len(),
        t0.elapsed().as_secs_f64()
    );
    assert_eq!(normal.unsupported, compact.unsupported, "unsupported instructions");
    let inputs = Inputs::lanes(code_mem(&code, 0x4000));
    let mut results = Vec::new();
    for (t, backend) in [(&normal, "translate"), (&compact, "translate_compact")] {
        let t1 = std::time::Instant::now();
        let dxil = super::dxc::compile(&t.hlsl, "main", "cs_6_6", &[]).unwrap_or_else(|e| panic!("{backend}: DXC: {e}"));
        eprintln!("big CS {backend}: DXIL {} bytes in {:.1} s", dxil.len(), t1.elapsed().as_secs_f64());
        let t2 = std::time::Instant::now();
        match runner::run_with_statics(&dxil, &inputs.state, &inputs.mem, &[], &[], &t.static_samplers) {
            Ok(r) => {
                eprintln!("big CS {backend}: ran in {:.2} s", t2.elapsed().as_secs_f64());
                results.push(Some(bytes(&r)));
            }
            // only `translate`'s pipeline may fail to be created; the compact form must run
            Err(e) if backend == "translate" && e.starts_with("CreateComputePipelineState") => {
                eprintln!("big CS translate: {e} after {:.1} s", t2.elapsed().as_secs_f64());
                results.push(None);
            }
            Err(e) => panic!("{backend}: run: {e}"),
        }
    }
    let (Some(a), Some(b)) = (&results[0], &results[1]) else {
        eprintln!("SKIPPED gpu_compact_sample_big_cs: A/B comparison (translate's pipeline was not created)");
        return;
    };
    let d = differences(a, b, slot);
    assert!(d.is_empty(), "big CS: translate / translate_compact differ:\n{}", d.join("\n"));
}
