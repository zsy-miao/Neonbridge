//! RDNA2 (Xbox Series X|S GPU) instruction decoder and disassembler.
//!
//! Tables are generated from AMD's machine-readable ISA specification
//! (`scripts/gen_rdna2.py`); this file implements the decoding rules on top:
//! encoding identification, literal constants, SDWA/DPP extension dwords and MIMG NSA.
//! It is the first stage of the shader recompiler (console microcode → PC shaders).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::OnceLock;

use super::rdna2_spec::{ENCODINGS, INSTRUCTIONS};
use super::rdna2_xbox::CONSOLE_INSTRUCTIONS;

pub struct Field {
    pub name: &'static str,
    /// (bit offset, bit count), least significant part first
    pub ranges: &'static [(u8, u8)],
}

pub struct Encoding {
    pub name: &'static str,
    pub bits: u16,
    pub order: i16,
    pub primary: bool,
    pub fixed_mask: u64,
    pub fixed_value: u64,
    pub fields: &'static [Field],
}

pub struct Operand {
    pub field: Option<&'static str>,
    pub ty: &'static str,
    pub size: u16,
    pub input: bool,
    pub output: bool,
    pub implicit: bool,
}

pub struct InstEnc {
    pub encoding: u16,
    pub opcode: u16,
    pub operands: &'static [Operand],
}

pub struct Inst {
    pub name: &'static str,
    pub flags: u8,
    pub always_literal: bool,
    pub encs: &'static [InstEnc],
}

pub const F_BRANCH: u8 = 1;
pub const F_CONDITIONAL: u8 = 2;
pub const F_INDIRECT: u8 = 4;
pub const F_TERMINATOR: u8 = 8;

impl Encoding {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }
    pub fn field_width(&self, name: &str) -> u32 {
        self.field(name).map(|f| f.ranges.iter().map(|r| r.1 as u32).sum()).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Plain,
    Sdwa,
    Dpp16,
    Dpp8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    Unknown(u32),
}

pub struct Decoded {
    pub pc: u64,
    pub inst: &'static Inst,
    pub ienc: &'static InstEnc,
    pub enc: &'static Encoding,
    /// raw instruction dwords (up to 5 for MIMG NSA3)
    pub words: Vec<u32>,
    pub len: usize,
    pub literal: Option<u32>,
    pub variant: Variant,
    /// constant target of an `s_setpc_b64` that ends a far jump (`resolve_far_jumps`)
    pub far_target: Option<u64>,
}

struct Tables {
    /// primary encodings ordered by spec priority
    primary: Vec<usize>,
    by_op: HashMap<(usize, u16), (&'static Inst, usize)>,
    /// encodings without an OP field (EXP) → their single instruction
    no_op: HashMap<usize, (&'static Inst, usize)>,
}

/// Every instruction the decoder knows: the RDNA2 spec first, then the console
/// extensions (which only fill opcodes the spec leaves unassigned).
pub fn all_instructions() -> impl Iterator<Item = &'static Inst> {
    INSTRUCTIONS.iter().chain(CONSOLE_INSTRUCTIONS.iter())
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut primary: Vec<usize> = (0..ENCODINGS.len()).filter(|&i| ENCODINGS[i].primary).collect();
        primary.sort_by_key(|&i| ENCODINGS[i].order);
        let mut by_op = HashMap::new();
        let mut no_op = HashMap::new();
        for inst in all_instructions() {
            for (ei, e) in inst.encs.iter().enumerate() {
                by_op.entry((e.encoding as usize, e.opcode)).or_insert((inst, ei));
                if ENCODINGS[e.encoding as usize].field("OP").is_none() {
                    no_op.entry(e.encoding as usize).or_insert((inst, ei));
                }
            }
        }
        Tables { primary, by_op, no_op }
    })
}

fn bits(words: &[u32], off: u32, cnt: u32) -> u64 {
    let mut v = 0u64;
    for i in 0..cnt {
        let b = off + i;
        let w = words.get((b / 32) as usize).copied().unwrap_or(0);
        v |= (((w >> (b % 32)) & 1) as u64) << i;
    }
    v
}

pub fn field_value(enc: &Encoding, words: &[u32], name: &str) -> Option<u64> {
    let f = enc.field(name)?;
    let mut v = 0u64;
    let mut shift = 0u32;
    for &(off, cnt) in f.ranges {
        v |= bits(words, off as u32, cnt as u32) << shift;
        shift += cnt as u32;
    }
    Some(v)
}

fn read_words(code: &[u8], n: usize) -> Vec<u32> {
    (0..n)
        .map(|i| {
            code.get(i * 4..i * 4 + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .unwrap_or(0)
        })
        .collect()
}

/// Operand types that are encoded as a scalar/vector source code and therefore can
/// select the 32-bit literal (code 255).
fn is_src_type(ty: &str) -> bool {
    matches!(
        ty,
        "OPR_SRC" | "OPR_SRC_NOLDS" | "OPR_SRC_VGPR" | "OPR_SSRC" | "OPR_SSRC_NOLDS" | "OPR_SSRC_LANESEL" | "OPR_SIMM32"
    )
}

fn is_valu32(enc: &Encoding) -> bool {
    matches!(enc.name, "ENC_VOP1" | "ENC_VOP2" | "ENC_VOPC")
}

fn is_valu(enc: &Encoding) -> bool {
    is_valu32(enc) || matches!(enc.name, "ENC_VOP3" | "ENC_VOP3P")
}

pub fn decode(code: &[u8], pc: u64) -> Result<Decoded, DecodeError> {
    if code.len() < 4 {
        return Err(DecodeError::Truncated);
    }
    let head = read_words(code, 2);
    let raw64 = head[0] as u64 | (head[1] as u64) << 32;
    let t = tables();
    let mut found = None;
    for &ei in &t.primary {
        let enc = &ENCODINGS[ei];
        if raw64 & enc.fixed_mask != enc.fixed_value {
            continue;
        }
        let hit = match field_value(enc, &head, "OP") {
            Some(op) => t.by_op.get(&(ei, op as u16)),
            None => t.no_op.get(&ei),
        };
        if let Some(&(inst, ie)) = hit {
            found = Some((ei, inst, ie));
            break;
        }
    }
    let Some((ei, inst, ie)) = found else { return Err(DecodeError::Unknown(head[0])) };
    let enc = &ENCODINGS[ei];
    let ienc = &inst.encs[ie];
    let base = enc.bits as usize / 8;
    let base_words = read_words(code, base / 4);

    let mut variant = Variant::Plain;
    let mut extra = 0usize;
    let mut has_literal = inst.always_literal;
    if is_valu32(enc) {
        match field_value(enc, &base_words, "SRC0") {
            Some(0xF9) => variant = Variant::Sdwa,
            Some(0xFA) => variant = Variant::Dpp16,
            Some(0xE9) | Some(0xEA) => variant = Variant::Dpp8,
            _ => {}
        }
        if variant != Variant::Plain {
            extra = 4;
        }
    }
    if variant == Variant::Plain && !has_literal {
        has_literal = ienc.operands.iter().any(|o| {
            !o.implicit
                && is_src_type(o.ty)
                && o.field.and_then(|f| field_value(enc, &base_words, f)) == Some(255)
        });
        // VALU: the literal is fetched whenever a source field selects it, even one the
        // opcode does not read – console compilers hide the shader hash in v_nop literals
        if !has_literal && is_valu(enc) {
            has_literal = ["SRC0", "SRC1", "SRC2"].iter().any(|f| field_value(enc, &base_words, f) == Some(255));
        }
    }
    if has_literal {
        extra += 4;
    }
    if enc.name == "ENC_MIMG" {
        extra += 4 * field_value(enc, &base_words, "NSA").unwrap_or(0) as usize;
    }
    let len = base + extra;
    if code.len() < len {
        return Err(DecodeError::Truncated);
    }
    let words = read_words(code, len / 4);
    let literal = has_literal.then(|| words[base / 4]);
    Ok(Decoded { pc, inst, ienc, enc, words, len, literal, variant, far_target: None })
}

// ---------------------------------------------------------------------------------------
// formatting (LLVM-like, not byte-for-byte identical)
// ---------------------------------------------------------------------------------------

fn reg_range(prefix: &str, n: u64, size: u16) -> String {
    let k = (size as u64).div_ceil(32).max(1);
    if k == 1 {
        format!("{prefix}{n}")
    } else {
        format!("{prefix}[{n}:{}]", n + k - 1)
    }
}

pub fn scalar_operand(code: u64, size: u16, literal: Option<u32>) -> String {
    let wide = size > 32;
    match code {
        0..=105 => reg_range("s", code, size),
        106 => if wide { "vcc".into() } else { "vcc_lo".into() },
        107 => "vcc_hi".into(),
        108..=123 => reg_range("ttmp", code - 108, size),
        124 => "m0".into(),
        125 => "null".into(),
        126 => if wide { "exec".into() } else { "exec_lo".into() },
        127 => "exec_hi".into(),
        128 => "0".into(),
        129..=192 => format!("{}", code - 128),
        193..=208 => format!("-{}", code - 192),
        235 => "src_shared_base".into(),
        236 => "src_shared_limit".into(),
        237 => "src_private_base".into(),
        238 => "src_private_limit".into(),
        239 => "src_pops_exiting_wave_id".into(),
        240 => "0.5".into(),
        241 => "-0.5".into(),
        242 => "1.0".into(),
        243 => "-1.0".into(),
        244 => "2.0".into(),
        245 => "-2.0".into(),
        246 => "4.0".into(),
        247 => "-4.0".into(),
        248 => "0.15915494".into(),
        251 => "src_vccz".into(),
        252 => "src_execz".into(),
        253 => "src_scc".into(),
        254 => "src_lds_direct".into(),
        255 => format!("{:#x}", literal.unwrap_or(0)),
        c => format!("?{c}"),
    }
}

fn operand_text(d: &Decoded, o: &Operand) -> Option<String> {
    if o.implicit {
        return None;
    }
    let f = o.field?;
    if f == "LITERAL" {
        return Some(format!("{:#x}", d.literal.unwrap_or(0)));
    }
    if f == "SRC0" && d.variant != Variant::Plain {
        // SDWA / DPP: the real src0 lives in the extension dword
        let ext = *d.words.get(1)?;
        let v = (ext & 0xff) as u64;
        return Some(if d.variant == Variant::Sdwa && ext >> 23 & 1 == 1 {
            scalar_operand(v, o.size, None)
        } else {
            reg_range("v", v, o.size)
        });
    }
    let v = field_value(d.enc, &d.words, f)? * register_scale(f);
    let width = d.enc.field_width(f);
    Some(match o.ty {
        // 9-bit source field (e.g. v_readfirstlane src0): 256+ are VGPRs, below are scalar codes
        "OPR_VGPR" | "OPR_VGPR_OR_LDS" if width >= 9 => {
            if v >= 256 {
                reg_range("v", v - 256, o.size)
            } else {
                scalar_operand(v, o.size, d.literal)
            }
        }
        "OPR_VGPR" | "OPR_VGPR_OR_LDS" => reg_range("v", v, o.size),
        "OPR_SRC" | "OPR_SRC_NOLDS" | "OPR_SRC_VGPR" | "OPR_SRC_SIMPLE" | "OPR_ATTR" | "OPR_PARAM" if width >= 9 => {
            if v >= 256 {
                reg_range("v", v - 256, o.size)
            } else {
                scalar_operand(v, o.size, d.literal)
            }
        }
        "OPR_SRC_SIMPLE" => reg_range("v", v, o.size),
        "OPR_SIMM16" | "OPR_WAITCNT" | "OPR_HWREG" | "OPR_SENDMSG" | "OPR_CLAUSE" | "OPR_VERSION" | "OPR_WAITCNT_DEPCTR" | "OPR_TGT" => {
            format!("{v:#x}")
        }
        "OPR_LABEL" => {
            let off = (v as u16 as i16 as i64) * 4;
            format!("{:#x}", (d.pc as i64 + 4 + off) as u64)
        }
        "OPR_SMEM_OFFSET" => {
            if v == 125 {
                return None; // SOFFSET = null → immediate offset only
            }
            scalar_operand(v, 32, d.literal)
        }
        _ => scalar_operand(v, o.size, d.literal),
    })
}

/// Fields that hold a scalar register number in units of several SGPRs: SMEM SBASE is
/// an SGPR pair index, MUBUF/MTBUF/MIMG SRSRC and SSAMP are SGPR quad indices.
pub fn register_scale(field: &str) -> u64 {
    match field {
        "SBASE" => 2,
        "SRSRC" | "SSAMP" => 4,
        _ => 1,
    }
}

/// Assemble one instruction (spec name such as "V_ADD_F32", encoding name such as
/// "ENC_VOP2" or "VOP3_SDST_ENC"), setting the given fields; for tests and tools.
pub fn encode(name: &str, enc: &str, fields: &[(&str, u64)], literal: Option<u32>) -> Result<Vec<u8>, String> {
    let ei = ENCODINGS.iter().position(|e| e.name == enc).ok_or_else(|| format!("unknown encoding {enc}"))?;
    let e = &ENCODINGS[ei];
    let inst = all_instructions()
        .find(|i| i.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("unknown instruction {name}"))?;
    // SDWA / DPP layouts describe the words of the base VOP1/VOP2/VOPC encoding
    let base = ["VOP1", "VOP2", "VOPC"]
        .iter()
        .find(|b| enc.starts_with(&format!("{b}_VOP_")))
        .and_then(|b| ENCODINGS.iter().position(|e| e.name == format!("ENC_{b}")))
        .unwrap_or(ei);
    let ie = inst
        .encs
        .iter()
        .find(|x| x.encoding as usize == base)
        .ok_or_else(|| format!("{name} has no encoding {enc}"))?;
    let mut words = vec![0u32; (e.bits as usize / 32).max(1)];
    let fixed = e.fixed_value & e.fixed_mask;
    for b in 0..64u32 {
        if e.fixed_mask >> b & 1 == 1 {
            let w = (b / 32) as usize;
            if w < words.len() {
                words[w] |= (((fixed >> b) & 1) as u32) << (b % 32);
            }
        }
    }
    fn put(words: &mut [u32], f: &Field, v: u64) {
        let mut v = v;
        for &(off, cnt) in f.ranges {
            for i in 0..cnt as u32 {
                let b = off as u32 + i;
                let w = (b / 32) as usize;
                if w < words.len() {
                    words[w] = (words[w] & !(1 << (b % 32))) | (((v & 1) as u32) << (b % 32));
                }
                v >>= 1;
            }
        }
    }
    if let Some(f) = e.field("OP") {
        put(&mut words, f, ie.opcode as u64);
    }
    for &(n, v) in fields {
        let f = e.field(n).ok_or_else(|| format!("{enc} has no field {n}"))?;
        put(&mut words, f, v);
    }
    let mut out: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    if let Some(l) = literal {
        out.extend_from_slice(&l.to_le_bytes());
    }
    Ok(out)
}

/// An encoding by its spec name (e.g. the extension layouts "VOP1_VOP_SDWA", "VOP2_VOP_DPP16",
/// whose fields describe the same instruction words as the base VOP1/VOP2 encoding).
pub fn encoding(name: &str) -> Option<&'static Encoding> {
    ENCODINGS.iter().find(|e| e.name == name)
}

/// VGPRs an instruction reads and writes (explicit operands; register ranges by operand
/// size). SDWA/DPP src0 comes from the extension dword.
/// SGPRs (codes 0–127 without NULL: s0–s105, VCC, M0, EXEC…) read and written by the
/// explicit operands: (reads, writes).
pub fn sgpr_uses(d: &Decoded) -> (Vec<u64>, Vec<u64>) {
    let (mut r, mut w) = (Vec::new(), Vec::new());
    for o in d.ienc.operands {
        let Some(f) = o.field else { continue };
        if f == "LITERAL" || o.implicit {
            continue;
        }
        if !["SREG", "SSRC", "SDST", "OPR_SRC"].iter().any(|k| o.ty.contains(k)) {
            continue;
        }
        if f == "SRC0" && d.variant != Variant::Plain {
            continue;
        }
        let Some(v) = field_value(d.enc, &d.words, f) else { continue };
        let v = v * register_scale(f);
        if v > 127 || v == 125 {
            continue;
        }
        let n = (o.size as u64).div_ceil(32).max(1);
        let dst = if o.output { &mut w } else { &mut r };
        dst.extend(v..(v + n).min(128));
    }
    (r, w)
}

pub fn vgpr_uses(d: &Decoded) -> (Vec<u64>, Vec<u64>) {
    let (mut r, mut w) = (Vec::new(), Vec::new());
    for o in d.ienc.operands {
        let Some(f) = o.field else { continue };
        if f == "LITERAL" || o.implicit {
            continue;
        }
        let Some(mut v) = field_value(d.enc, &d.words, f) else { continue };
        let width = d.enc.field_width(f);
        let vgpr = if o.ty.starts_with("OPR_VGPR") && width <= 8 {
            true
        } else if width >= 9 && (o.ty.starts_with("OPR_SRC") || o.ty.starts_with("OPR_VGPR")) && v >= 256 {
            v -= 256;
            true
        } else if f == "SRC0" && d.variant != Variant::Plain {
            v = (d.words.get(1).copied().unwrap_or(0) & 0xff) as u64;
            !(d.variant == Variant::Sdwa && d.words.get(1).copied().unwrap_or(0) >> 23 & 1 == 1)
        } else {
            false
        };
        if !vgpr {
            continue;
        }
        let mut n = (o.size as u64).div_ceil(32).max(1);
        // MUBUF / MTBUF VADDR: only the index and / or offset VGPRs actually used
        if f == "VADDR" && matches!(d.enc.name, "ENC_MUBUF" | "ENC_MTBUF") {
            n = field_value(d.enc, &d.words, "IDXEN").unwrap_or(0) + field_value(d.enc, &d.words, "OFFEN").unwrap_or(0);
        }
        let dst = if o.output { &mut w } else { &mut r };
        dst.extend(v..v + n);
    }
    // read-modify-write accumulators
    if matches!(d.inst.name, "V_FMAC_F32" | "V_MAC_F32" | "V_FMAC_LEGACY_F32") {
        r.extend(w.iter().copied());
    }
    (r, w)
}

/// Target of a direct branch (s_branch, s_cbranch_*), or None for anything else
/// (including indirect jumps like s_setpc_b64).
pub fn branch_target(d: &Decoded) -> Option<u64> {
    if d.far_target.is_some() {
        return d.far_target;
    }
    if d.inst.flags & F_BRANCH == 0 || d.inst.flags & F_INDIRECT != 0 {
        return None;
    }
    let o = d.ienc.operands.iter().find(|o| o.ty == "OPR_LABEL")?;
    let v = field_value(d.enc, &d.words, o.field?)?;
    Some((d.pc as i64 + 4 + (v as u16 as i16 as i64) * 4) as u64)
}

/// Value of an integer scalar source operand that is a constant (inline or literal).
fn scalar_int(d: &Decoded, code: u64) -> Option<u32> {
    Some(match code {
        128..=192 => (code - 128) as u32,
        193..=208 => (-((code - 192) as i32)) as u32,
        255 => d.literal?,
        _ => return None,
    })
}

/// Far jumps. A branch whose target is out of the 16-bit `s_branch` range (±128 KB) is
/// built by the compiler as
/// `s_getpc_b64 s[n:n+1]; s_add_u32 sn, sn, lo; s_addc_u32 sn+1, sn+1, hi; s_setpc_b64 s[n:n+1]`
/// (`s_sub_u32` / `s_subb_u32` for a negative offset): a jump to the address after the
/// `s_getpc_b64` plus the 64-bit constant hi:lo. Each such `s_setpc_b64` gets that target
/// (`far_target`), so it is an unconditional branch like `s_branch` to everything that
/// follows the control flow. Any other `s_setpc_b64` stays an indirect jump.
pub fn resolve_far_jumps(insts: &mut [Decoded]) {
    for k in 3..insts.len() {
        let [get, lo, hi, set] = &insts[k - 3..=k] else { continue };
        if get.inst.name != "S_GETPC_B64" || set.inst.name != "S_SETPC_B64" {
            continue;
        }
        let sub = match (lo.inst.name, hi.inst.name) {
            ("S_ADD_U32", "S_ADDC_U32") => false,
            ("S_SUB_U32", "S_SUBB_U32") => true,
            _ => continue,
        };
        let f = |d: &Decoded, n: &str| field_value(d.enc, &d.words, n);
        let Some(r) = f(get, "SDST") else { continue };
        if f(set, "SSRC0") != Some(r) {
            continue;
        }
        // `s = s ± c` on one half of the address; the constant may be either source of an add
        let half = |d: &Decoded, reg: u64| -> Option<u32> {
            if f(d, "SDST")? != reg {
                return None;
            }
            match (f(d, "SSRC0")?, f(d, "SSRC1")?) {
                (a, c) if a == reg => scalar_int(d, c),
                (c, a) if a == reg && !sub => scalar_int(d, c),
                _ => None,
            }
        };
        let (Some(l), Some(h)) = (half(lo, r), half(hi, r + 1)) else { continue };
        let offset = (h as u64) << 32 | l as u64;
        let base = get.pc + get.len as u64;
        insts[k].far_target = Some(if sub { base.wrapping_sub(offset) } else { base.wrapping_add(offset) });
    }
}

pub fn mnemonic(d: &Decoded) -> String {
    let mut m = d.inst.name.to_ascii_lowercase();
    match d.variant {
        Variant::Sdwa => m.push_str("_sdwa"),
        Variant::Dpp16 => m.push_str("_dpp"),
        Variant::Dpp8 => m.push_str("_dpp8"),
        Variant::Plain => {}
    }
    m
}

pub fn format(d: &Decoded) -> String {
    let mut s = mnemonic(d);
    let ops: Vec<String> = d.ienc.operands.iter().filter_map(|o| operand_text(d, o)).collect();
    if !ops.is_empty() {
        s.push(' ');
        s.push_str(&ops.join(", "));
    }
    if let Some(lit) = d.literal {
        let shown = d.inst.always_literal
            || d.ienc.operands.iter().any(|o| {
                !o.implicit && o.field.is_some_and(|f| f == "LITERAL" || field_value(d.enc, &d.words, f) == Some(255) && is_src_type(o.ty))
            });
        if !shown {
            let _ = write!(s, " ; lit {lit:#010x}");
        }
    }
    match d.enc.name {
        "ENC_SMEM" => {
            if let Some(off) = field_value(d.enc, &d.words, "OFFSET") {
                let _ = write!(s, ", {off:#x}");
            }
        }
        "ENC_MUBUF" | "ENC_MTBUF" | "ENC_FLAT" | "ENC_FLAT_GLBL" | "ENC_FLAT_SCRATCH" | "ENC_DS" => {
            for (f, label) in [("OFFSET", "offset"), ("OFFSET0", "offset0"), ("OFFSET1", "offset1")] {
                if let Some(v) = field_value(d.enc, &d.words, f).filter(|&v| v != 0) {
                    let _ = write!(s, " {label}:{v}");
                }
            }
            for (f, label) in [("OFFEN", "offen"), ("IDXEN", "idxen"), ("GLC", "glc"), ("SLC", "slc"), ("DLC", "dlc"), ("GDS", "gds")] {
                if field_value(d.enc, &d.words, f) == Some(1) {
                    let _ = write!(s, " {label}");
                }
            }
        }
        _ => {}
    }
    s
}

pub struct Line {
    pub pc: u64,
    pub bytes: Vec<u8>,
    pub text: String,
    pub ok: bool,
    pub flags: u8,
}

/// Disassemble a code stream. Stops after `s_endpgm` when `stop_at_end` is set.
pub fn disassemble(code: &[u8], base_pc: u64, stop_at_end: bool) -> Vec<Line> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 4 <= code.len() {
        let pc = base_pc + off as u64;
        match decode(&code[off..], pc) {
            Ok(d) => {
                let end = d.inst.flags & F_TERMINATOR != 0;
                out.push(Line { pc, bytes: code[off..off + d.len].to_vec(), text: format(&d), ok: true, flags: d.inst.flags });
                off += d.len;
                if end && stop_at_end {
                    break;
                }
            }
            Err(_) => {
                let w = u32::from_le_bytes([code[off], code[off + 1], code[off + 2], code[off + 3]]);
                out.push(Line { pc, bytes: code[off..off + 4].to_vec(), text: format!(".long {w:#010x}"), ok: false, flags: 0 });
                off += 4;
            }
        }
    }
    out
}

/// Heuristic: offsets of plausible shader programs in an unknown blob (ending in s_endpgm).
pub fn find_programs(blob: &[u8]) -> Vec<(usize, usize)> {
    const S_ENDPGM: u32 = 0xBF81_0000;
    let mut ends = Vec::new();
    let mut i = 0;
    while i + 4 <= blob.len() {
        let w = u32::from_le_bytes([blob[i], blob[i + 1], blob[i + 2], blob[i + 3]]);
        if w == S_ENDPGM {
            ends.push(i);
        }
        i += 4;
    }
    let mut progs = Vec::new();
    let mut start = 0usize;
    for e in ends {
        // walk forward from `start` and require every dword up to s_endpgm to decode
        let slice = &blob[start..e + 4];
        let lines = disassemble(slice, 0, false);
        let good = lines.iter().filter(|l| l.ok).count();
        if !lines.is_empty() && good * 10 >= lines.len() * 9 {
            progs.push((start, e + 4 - start));
        }
        start = e + 4;
    }
    progs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dis(bytes: &[u8]) -> String {
        format(&decode(bytes, 0).unwrap())
    }

    #[test]
    fn basic_scalar_and_vector() {
        assert_eq!(dis(&[0x01, 0x03, 0x80, 0xbe]), "s_mov_b32 s0, s1");
        assert_eq!(dis(&[0x00, 0x00, 0x81, 0xbf]), "s_endpgm");
        let d = decode(&[0x01, 0x05, 0x0a, 0x06], 0).unwrap();
        assert_eq!(d.inst.name, "V_ADD_F32");
        assert_eq!(d.len, 4);
        // s_mov_b32 s0, 0x12345678 (literal)
        let d = decode(&[0xff, 0x03, 0x80, 0xbe, 0x78, 0x56, 0x34, 0x12], 0).unwrap();
        assert_eq!(d.len, 8);
        assert_eq!(format(&d), "s_mov_b32 s0, 0x12345678");
    }

    #[test]
    fn vgpr_in_nine_bit_field() {
        // v_readfirstlane_b32 s5, v1 (LLVM gfx10_vop1.txt): SRC0 = 257 is v1, not "v257"
        assert_eq!(dis(&[0x01, 0x05, 0x0a, 0x7e]), "v_readfirstlane_b32 s5, v1");
    }

    #[test]
    fn scalar_register_fields_are_scaled() {
        // SBASE counts SGPR pairs, SRSRC SGPR quads (seen in retail code)
        assert_eq!(dis(&[0x08, 0x02, 0x08, 0xf4, 0x20, 0x09, 0x00, 0xfa]), "s_load_dwordx4 s[8:11], s[16:17], 0x920");
        assert_eq!(dis(&[0x04, 0x02, 0x28, 0xf4, 0x40, 0x00, 0x00, 0xfa]), "s_buffer_load_dwordx4 s[8:11], s[8:11], 0x40");
    }

    #[test]
    fn encoder_round_trips() {
        let b = encode("V_ADD_F32", "ENC_VOP2", &[("VDST", 1), ("SRC0", 256 + 2), ("VSRC1", 3)], None).unwrap();
        assert_eq!(dis(&b), "v_add_f32 v1, v2, v3");
        let b = encode("S_MOV_B32", "ENC_SOP1", &[("SDST", 0), ("SSRC0", 255)], Some(0x1234_5678)).unwrap();
        assert_eq!(dis(&b), "s_mov_b32 s0, 0x12345678");
        let b = encode("V_FMA_F32", "ENC_VOP3", &[("VDST", 0), ("SRC0", 256 + 4), ("SRC1", 244), ("SRC2", 243)], None).unwrap();
        assert_eq!(dis(&b), "v_fma_f32 v0, v4, 2.0, -1.0");
        let b = encode("V_ADD_CO_U32", "VOP3_SDST_ENC", &[("VDST", 1), ("SDST", 106), ("SRC0", 256 + 2), ("SRC1", 256 + 3)], None).unwrap();
        assert_eq!(decode(&b, 0).unwrap().inst.name, "V_ADD_CO_U32");
        assert_eq!(dis(&encode("S_ENDPGM", "ENC_SOPP", &[], None).unwrap()), "s_endpgm");
    }

    #[test]
    fn literal_in_unused_valu_field() {
        // v_nop (VOP3) with SRC2 = literal: retail console code carries the shader hash this way
        let d = decode(&[0xdb, 0xb7, 0x80, 0xd5, 0xb9, 0x33, 0xff, 0x03, 0x46, 0x66, 0x47, 0x9d], 0).unwrap();
        assert_eq!(d.inst.name, "V_NOP");
        assert_eq!(d.len, 12);
        assert_eq!(format(&d), "v_nop ; lit 0x9d476646");
    }

    #[test]
    fn sdwa_and_dpp_src0() {
        // v_mov_b32_sdwa v1, v2 (SDWA dword: src0 = v2)
        assert_eq!(dis(&[0xf9, 0x02, 0x02, 0x7e, 0x02, 0x06, 0x06, 0x00]), "v_mov_b32_sdwa v1, v2");
        // same with S0 set: src0 = s2
        assert_eq!(dis(&[0xf9, 0x02, 0x02, 0x7e, 0x02, 0x06, 0x86, 0x00]), "v_mov_b32_sdwa v1, s2");
        // v_mov_b32_dpp v1, v3
        assert_eq!(dis(&[0xfa, 0x02, 0x02, 0x7e, 0x03, 0x1b, 0x00, 0xff]), "v_mov_b32_dpp v1, v3");
    }

    #[test]
    fn console_legacy_mad_mac() {
        // v_mac_f32 v6, s40, v21 (VOP2 0x1f)
        assert_eq!(dis(&[0x28, 0x2a, 0x0c, 0x3e]), "v_mac_f32 v6, s40, v21");
        // v_mad_f32 (VOP3 0x141) is 8 bytes – the next dword must not be decoded on its own
        let d = decode(&[0x07, 0x00, 0x41, 0xd5, 0x2a, 0x1a, 0x62, 0x04], 0).unwrap();
        assert_eq!((d.inst.name, d.len), ("V_MAD_F32", 8));
        // v_madmk_f32 v1, v2, 0x3f800000, v3
        let d = decode(&[0x02, 0x07, 0x02, 0x40, 0x00, 0x00, 0x80, 0x3f], 0).unwrap();
        assert_eq!(d.len, 8);
        assert_eq!(format(&d), "v_madmk_f32 v1, v2, 0x3f800000, v3");
        // spec instructions keep their opcodes
        assert_eq!(decode(&[0x01, 0x05, 0x0a, 0x56], 0).unwrap().inst.name, "V_FMAC_F32");
    }

    #[test]
    fn truncated_and_unknown() {
        assert_eq!(decode(&[0x01, 0x02], 0).err(), Some(DecodeError::Truncated));
        assert_eq!(decode(&[0xff, 0x03, 0x80, 0xbe], 0).err(), Some(DecodeError::Truncated));
    }

    /// Normalise an LLVM mnemonic to the AMD spec name (lower case, no encoding suffix).
    fn llvm_base(m: &str) -> String {
        let mut s = m.to_ascii_lowercase();
        for suf in ["_e32", "_e64", "_sdwa", "_dpp8", "_dpp"] {
            if let Some(x) = s.strip_suffix(suf) {
                s = x.to_string();
                break;
            }
        }
        s
    }

    /// LLVM's gfx10 tests also cover RDNA1 (gfx1010). These were removed (or had their
    /// opcode reused) in RDNA2 / gfx1030, so the RDNA2 spec rightly does not know them.
    fn removed_in_rdna2(m: &str) -> bool {
        m.starts_with("s_store_")
            || m.starts_with("s_buffer_store_")
            || m.starts_with("s_atomic_")
            || m.starts_with("s_buffer_atomic_")
            || m.starts_with("s_scratch_")
            || (m.starts_with("ds_") && m.contains("_src2_"))
            || m.starts_with("s_atc_probe")
            || m.starts_with("s_dcache_discard")
            || matches!(
                m,
                "v_mac_f32" | "v_mad_f32" | "v_madak_f32" | "v_madmk_f32" | "s_dcache_wb" | "s_get_waveid_in_workgroup" | "s_wait_idle"
            )
    }

    fn renamed_in_rdna2(llvm: &str, spec: &str) -> bool {
        matches!((llvm, spec), ("v_mac_legacy_f32", "v_fmac_legacy_f32") | ("v_mad_legacy_f32", "v_fma_legacy_f32"))
    }

    /// Decode every instruction in LLVM's gfx10 disassembler tests and compare the
    /// mnemonic and instruction length with LLVM's expectations.
    #[test]
    fn llvm_gfx10_corpus() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/llvm-amdgpu");
        let mut total = 0usize;
        let mut decoded = 0usize;
        let mut name_ok = 0usize;
        let mut len_ok = 0usize;
        let mut bad: HashMap<String, usize> = HashMap::new();
        let mut failed_decode: HashMap<String, usize> = HashMap::new();
        for e in std::fs::read_dir(&dir).expect("tests/data/llvm-amdgpu") {
            let p = e.unwrap().path();
            if p.extension().map(|x| x != "txt").unwrap_or(true) {
                continue;
            }
            for line in std::fs::read_to_string(&p).unwrap().lines() {
                let Some(rest) = line.strip_prefix('#') else { continue };
                let Some((_, rest)) = rest.split_once(':') else { continue };
                let Some((asm, enc)) = rest.split_once("; encoding: [") else { continue };
                let Some(enc) = enc.split(']').next() else { continue };
                let bytes: Vec<u8> = enc
                    .split(',')
                    .filter_map(|b| u8::from_str_radix(b.trim().trim_start_matches("0x"), 16).ok())
                    .collect();
                let asm = asm.trim();
                let Some(m) = asm.split_whitespace().next() else { continue };
                if bytes.len() < 4 {
                    continue;
                }
                total += 1;
                let want = llvm_base(m);
                match decode(&bytes, 0) {
                    Ok(d) => {
                        decoded += 1;
                        let got = d.inst.name.to_ascii_lowercase();
                        if got == want || renamed_in_rdna2(&want, &got) {
                            name_ok += 1;
                        } else {
                            *bad.entry(format!("{want} -> {got}")).or_default() += 1;
                        }
                        if d.len == bytes.len() {
                            len_ok += 1;
                        } else {
                            *bad.entry(format!("LEN {want}: {} != {}", d.len, bytes.len())).or_default() += 1;
                        }
                    }
                    Err(_) => *failed_decode.entry(want).or_default() += 1,
                }
            }
        }
        let mut worst: Vec<_> = bad.into_iter().collect();
        worst.sort_by(|a, b| b.1.cmp(&a.1));
        let mut fails: Vec<_> = failed_decode.into_iter().collect();
        fails.sort_by(|a, b| b.1.cmp(&a.1));
        println!("LLVM gfx10 corpus: {total} instructions, decoded {decoded}, name match {name_ok}, length match {len_ok}");
        println!("top mismatches: {:?}", &worst[..worst.len().min(25)]);
        println!("top decode failures: {:?}", &fails[..fails.len().min(25)]);
        let unexpected: Vec<_> = fails.iter().filter(|(m, _)| !removed_in_rdna2(m)).collect();
        assert!(total > 10_000, "corpus too small: {total}");
        assert!(unexpected.is_empty(), "instructions RDNA2 should know but failed to decode: {unexpected:?}");
        assert_eq!(len_ok, decoded, "instruction length mismatches: {worst:?}");
        assert_eq!(name_ok, decoded, "mnemonic mismatches: {worst:?}");
    }
}
