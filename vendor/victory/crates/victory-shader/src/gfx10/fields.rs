//! Operand fields of GFX10 instructions, by encoding.
//!
//! Layouts follow AMD's RDNA ISA reference. Register ranges carry their size
//! in dwords, which depends on the opcode (read from its name) and, for lane
//! masks (VCC, EXEC, compare results), on the wave size.

use super::{Encoding, Instruction};

/// A source or destination of an instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    /// Scalar registers by operand code: 0-105 `s0`-`s105`, 106/107
    /// `vcc_lo`/`vcc_hi`, 108-123 `ttmp0`-`ttmp15`, 124 `m0`, 125 `null`,
    /// 126/127 `exec_lo`/`exec_hi`. The decoder gives no code above 127; the
    /// IR names scalars parked in VGPR lanes by the codes from 128 up
    /// (`crate::ir::PARKED`).
    Scalar {
        code: u8,
        dwords: u8,
    },
    Vector {
        reg: u8,
        dwords: u8,
    },
    /// Inline constant or read-only special source, by operand code
    /// (128-192: 0-64, 193-208: -1 to -16, 240-248: floats, 251-254: vccz,
    /// execz, scc, lds_direct).
    Const(u8),
    /// The 32-bit literal that follows the instruction.
    Literal(u32),
    /// Interpolation attribute and channel (0-3 = x-w); `high` selects the
    /// upper f16 half.
    Attr {
        index: u8,
        chan: u8,
        high: bool,
    },
}

/// Operand code of `null` (no register; reads as 0).
pub const NULL: u8 = 125;
const VCC: u8 = 106;
const EXEC: u8 = 126;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cache {
    pub glc: bool,
    pub slc: bool,
    pub dlc: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fields {
    /// SOP1, SOP2, SOPC, SOPK; `imm` is SOPK's 16-bit immediate.
    Sop {
        dst: Option<Operand>,
        src: Vec<Operand>,
        imm: Option<u16>,
    },
    /// The 16-bit immediate: branch offset in dwords, wait counts, message...
    Sopp {
        imm: u16,
    },
    Smem {
        data: Option<Operand>,
        base: Operand,
        /// Signed byte offset, added to `soffset`.
        offset: i32,
        soffset: Operand,
        cache: Cache,
    },
    Valu(Valu),
    Vintrp {
        dst: Operand,
        /// The i/j VGPR, or for `v_interp_mov_f32` the parameter (0 `p10`,
        /// 1 `p20`, 2 `p0`) as [`Operand::Const`].
        src: Operand,
        attr: Operand,
    },
    Ds {
        dst: Option<Operand>,
        addr: Option<Operand>,
        data: Vec<Operand>,
        offset0: u8,
        offset1: u8,
        gds: bool,
    },
    /// MUBUF, and MTBUF with `format` set.
    Buffer {
        /// Loaded, stored, or the atomic operand (and result with `glc`).
        data: Option<Operand>,
        addr: Operand,
        rsrc: Operand,
        soffset: Operand,
        offset: u16,
        offen: bool,
        idxen: bool,
        format: Option<u8>,
        lds: bool,
        tfe: bool,
        cache: Cache,
    },
    Image {
        data: Operand,
        /// One range, or one VGPR per address with the NSA form.
        addr: Vec<Operand>,
        rsrc: Operand,
        sampler: Option<Operand>,
        dmask: u8,
        dim: u8,
        unorm: bool,
        r128: bool,
        a16: bool,
        d16: bool,
        tfe: bool,
        lwe: bool,
        cache: Cache,
    },
    /// FLAT, GLOBAL and SCRATCH.
    Flat {
        dst: Option<Operand>,
        addr: Option<Operand>,
        data: Option<Operand>,
        saddr: Option<Operand>,
        offset: i16,
        lds: bool,
        cache: Cache,
    },
    Exp {
        target: u8,
        /// Per channel; `None` when disabled.
        src: [Option<Operand>; 4],
        compr: bool,
        done: bool,
        vm: bool,
    },
}

/// VOP1, VOP2, VOPC, VOP3 and VOP3P, with their SDWA and DPP forms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Valu {
    /// A VGPR range, an SGPR (`v_readlane_b32`...) or a lane mask (compares).
    pub dst: Option<Operand>,
    /// Carry-out or the second result of VOP3B (`v_add_co_u32`, `v_div_scale_f32`...).
    pub carry: Option<Operand>,
    /// In operand order, including the implicit VCC of VOP2 `v_cndmask_b32`
    /// and carry-in, and the literal of `v_madmk`/`v_madak` forms.
    pub src: Vec<Operand>,
    /// Bit i applies to `src[i]`. For VOP3P, `neg` is the low half's.
    pub neg: u8,
    pub abs: u8,
    /// VOP3P: negate the high half (`v_fma_mix*`: absolute value).
    pub neg_hi: u8,
    /// VOP3: bit 3 is the destination. VOP3P with `opsel_hi`: the halves used.
    pub opsel: u8,
    pub opsel_hi: u8,
    pub clamp: bool,
    /// Output modifier: 1 ×2, 2 ×4, 3 ÷2.
    pub omod: u8,
    pub sdwa: Option<Sdwa>,
    pub dpp: Option<Dpp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sdwa {
    /// Selects: 0-3 byte, 4-5 word, 6 dword. Compares have no `dst_sel`.
    pub dst_sel: u8,
    /// 0 pad with zeros, 1 sign-extend, 2 preserve.
    pub dst_unused: u8,
    pub src_sel: [u8; 2],
    /// Bit i sign-extends `src[i]`.
    pub sext: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dpp {
    /// DPP16: `ctrl` is the 9-bit permutation (quad_perm, row shifts...).
    Row {
        ctrl: u16,
        row_mask: u8,
        bank_mask: u8,
        bound_ctrl: bool,
        fi: bool,
    },
    /// DPP8: eight 3-bit lane selects.
    Lanes { sel: u32, fi: bool },
}

impl Instruction {
    /// Decodes the operand fields. `wave64` sizes lane masks.
    pub fn fields(&self, wave64: bool) -> Fields {
        let name = self.name().unwrap_or("");
        let w = |i: usize| self.words.get(i).copied().unwrap_or(0);
        let (w0, w1) = (w(0), w(1));
        let bit = |word: u32, n: u32| word >> n & 1 != 0;
        let cache = |glc, slc, dlc| Cache { glc, slc, dlc };
        match self.encoding {
            Encoding::Sop2 | Encoding::Sop1 | Encoding::Sopc => {
                let (dst, src0, src1) = salu_dwords(name);
                let sdst = scalar((w0 >> 16 & 0x7F) as u8, dst);
                let ssrc = |shift: u32, dwords| source(w0 >> shift & 0xFF, dwords, w1);
                let (dst, src) = match self.encoding {
                    Encoding::Sop2 => (Some(sdst), vec![ssrc(0, src0), ssrc(8, src1)]),
                    Encoding::Sopc => (None, vec![ssrc(0, src0), ssrc(8, src1)]),
                    _ => match name {
                        "s_getpc_b64" => (Some(sdst), vec![]),
                        "s_setpc_b64" | "s_rfe_b64" => (None, vec![ssrc(0, src0)]),
                        _ => (Some(sdst), vec![ssrc(0, src0)]),
                    },
                };
                Fields::Sop {
                    dst,
                    src,
                    imm: None,
                }
            }
            Encoding::Sopk => {
                let sdst = scalar((w0 >> 16 & 0x7F) as u8, 1 + u8::from(name == "s_call_b64"));
                let (dst, src) = match name {
                    "s_setreg_imm32_b32" => (None, vec![Operand::Literal(w1)]),
                    "s_version" => (None, vec![]),
                    n if n.starts_with("s_cmpk_")
                        || n.starts_with("s_waitcnt_")
                        || n == "s_setreg_b32" =>
                    {
                        (None, vec![sdst])
                    }
                    _ => (Some(sdst), vec![]),
                };
                Fields::Sop {
                    dst,
                    src,
                    imm: Some(w0 as u16),
                }
            }
            Encoding::Sopp => Fields::Sopp { imm: w0 as u16 },
            Encoding::Smem => {
                let data = match name {
                    "s_gl1_inv" | "s_dcache_inv" | "s_dcache_wb" => None,
                    n if n.starts_with("s_dcache_discard") => None,
                    "s_memtime" | "s_memrealtime" => Some(2),
                    _ => Some(data_dwords(name)),
                };
                let base = 2 + 2 * u8::from(name.starts_with("s_buffer_"));
                Fields::Smem {
                    data: data.map(|d| scalar((w0 >> 6 & 0x7F) as u8, d)),
                    base: scalar(((w0 & 0x3F) << 1) as u8, base),
                    offset: (w1 << 11) as i32 >> 11,
                    soffset: scalar((w1 >> 25) as u8, 1),
                    cache: cache(bit(w0, 16), false, bit(w0, 14)),
                }
            }
            Encoding::Vop1 | Encoding::Vop2 | Encoding::Vopc | Encoding::Vop3 | Encoding::Vop3p => {
                Fields::Valu(self.valu(name, if wave64 { 2 } else { 1 }))
            }
            Encoding::Vintrp => {
                let vsrc = (w0 & 0xFF) as u8;
                Fields::Vintrp {
                    dst: vector(w0 >> 18, 1),
                    src: if name == "v_interp_mov_f32" {
                        Operand::Const(vsrc)
                    } else {
                        vector(vsrc.into(), 1)
                    },
                    attr: Operand::Attr {
                        index: (w0 >> 10 & 0x3F) as u8,
                        chan: (w0 >> 8 & 3) as u8,
                        high: false,
                    },
                }
            }
            Encoding::Ds => {
                let ty = data_dwords(name);
                let pair = ds_pair(name);
                let returns = name.contains("read")
                    || name.contains("_rtn")
                    || matches!(
                        name,
                        "ds_swizzle_b32"
                            | "ds_permute_b32"
                            | "ds_bpermute_b32"
                            | "ds_append"
                            | "ds_consume"
                            | "ds_ordered_count"
                    );
                let no_addr = name.starts_with("ds_gws")
                    || name.contains("addtid")
                    || matches!(name, "ds_nop" | "ds_append" | "ds_consume");
                let data = if name.contains("read")
                    || name.contains("src2")
                    || matches!(
                        name,
                        "ds_swizzle_b32"
                            | "ds_nop"
                            | "ds_append"
                            | "ds_consume"
                            | "ds_ordered_count"
                            | "ds_gws_sema_v"
                            | "ds_gws_sema_p"
                            | "ds_gws_sema_release_all"
                    ) {
                    0
                } else if pair || name.contains("cmpst") || name.contains("mskor") {
                    2
                } else {
                    1
                };
                Fields::Ds {
                    dst: returns.then(|| vector(w1 >> 24, ty * (1 + u8::from(pair)))),
                    addr: (!no_addr).then(|| vector(w1 & 0xFF, 1)),
                    data: (0..data)
                        .map(|i| vector(w1 >> (8 + 8 * i) & 0xFF, ty))
                        .collect(),
                    offset0: w0 as u8,
                    offset1: (w0 >> 8) as u8,
                    gds: bit(w0, 17),
                }
            }
            Encoding::Mubuf | Encoding::Mtbuf => {
                let (offen, idxen, tfe) = (bit(w0, 12), bit(w0, 13), bit(w1, 23));
                let mtbuf = self.encoding == Encoding::Mtbuf;
                Fields::Buffer {
                    data: (!name.ends_with("_inv"))
                        .then(|| vector(w1 >> 8 & 0xFF, data_dwords(name) + u8::from(tfe))),
                    addr: vector(w1 & 0xFF, 1 + u8::from(offen && idxen)),
                    rsrc: scalar(((w1 >> 16 & 0x1F) << 2) as u8, 4),
                    soffset: source(w1 >> 24, 1, 0),
                    offset: (w0 & 0xFFF) as u16,
                    offen,
                    idxen,
                    format: mtbuf.then_some((w0 >> 19 & 0x7F) as u8),
                    lds: !mtbuf && bit(w0, 16),
                    tfe,
                    cache: cache(bit(w0, 14), bit(w1, 22), bit(w0, 15)),
                }
            }
            Encoding::Mimg => {
                let (dim, dmask, a16, d16) = (
                    (w0 >> 3 & 7) as u8,
                    (w0 >> 8 & 0xF) as u8,
                    bit(w1, 30),
                    bit(w1, 31),
                );
                let (r128, tfe, lwe) = (bit(w0, 15), bit(w0, 16), bit(w0, 17));
                let mut data = if name.starts_with("image_gather4") {
                    4
                } else {
                    dmask.count_ones().max(1) as u8
                };
                if d16 {
                    data = data.div_ceil(2);
                }
                let count = image_addr_dwords(name, dim, a16);
                let addr = if w0 >> 1 & 3 == 0 {
                    vec![vector(w1 & 0xFF, count)]
                } else {
                    // NSA: one VGPR per address, the first in dword 1, the rest
                    // in the extra dwords.
                    (0..usize::from(count))
                        .map(|i| match i {
                            0 => vector(w1 & 0xFF, 1),
                            i => vector(w(2 + (i - 1) / 4) >> (8 * ((i - 1) % 4)) & 0xFF, 1),
                        })
                        .collect()
                };
                let sampled = ["image_sample", "image_gather4", "image_get_lod"]
                    .iter()
                    .any(|p| name.starts_with(p));
                Fields::Image {
                    data: vector(w1 >> 8 & 0xFF, data + u8::from(tfe || lwe)),
                    addr,
                    rsrc: scalar(((w1 >> 16 & 0x1F) << 2) as u8, if r128 { 4 } else { 8 }),
                    sampler: sampled.then(|| scalar(((w1 >> 21 & 0x1F) << 2) as u8, 4)),
                    dmask,
                    dim,
                    unorm: bit(w0, 12),
                    r128,
                    a16,
                    d16,
                    tfe,
                    lwe,
                    cache: cache(bit(w0, 13), bit(w0, 25), bit(w0, 7)),
                }
            }
            Encoding::Flat | Encoding::Global | Encoding::Scratch => {
                let saddr = (w1 >> 16 & 0x7F) as u8;
                let glc = bit(w0, 16);
                let n = data_dwords(name);
                let (load, atomic) = (name.contains("_load_"), name.contains("_atomic_"));
                let addr = match self.encoding {
                    Encoding::Flat => Some(vector(w1 & 0xFF, 2)),
                    Encoding::Global => Some(vector(w1 & 0xFF, 1 + u8::from(saddr == NULL))),
                    // With an SGPR base, scratch takes no VGPR.
                    _ => (saddr == NULL).then(|| vector(w1 & 0xFF, 1)),
                };
                let ret = if atomic && name.contains("cmpswap") {
                    n / 2
                } else {
                    n
                };
                Fields::Flat {
                    dst: (load || atomic && glc).then(|| vector(w1 >> 24, ret)),
                    addr,
                    data: (!load).then(|| vector(w1 >> 8 & 0xFF, n)),
                    saddr: (self.encoding != Encoding::Flat)
                        .then(|| scalar(saddr, 1 + u8::from(self.encoding == Encoding::Global))),
                    offset: match self.encoding {
                        Encoding::Flat => (w0 & 0x7FF) as i16,
                        _ => ((w0 << 4) as i16) >> 4,
                    },
                    lds: bit(w0, 13),
                    cache: cache(glc, bit(w0, 17), bit(w0, 12)),
                }
            }
            Encoding::Exp => {
                let compr = bit(w0, 10);
                Fields::Exp {
                    target: (w0 >> 4 & 0x3F) as u8,
                    src: std::array::from_fn(|i| {
                        let slot = if compr { i / 2 } else { i };
                        bit(w0, i as u32).then(|| vector(w1 >> (8 * slot) & 0xFF, 1))
                    }),
                    compr,
                    done: bit(w0, 11),
                    vm: bit(w0, 12),
                }
            }
        }
    }

    fn valu(&self, name: &str, lanes: u8) -> Valu {
        // No operands; the console compiler stores a shader hash in their fields.
        if matches!(name, "v_nop" | "v_pipeflush" | "v_clrexcp") {
            return Valu::default();
        }
        let w = |i: usize| self.words.get(i).copied().unwrap_or(0);
        let w0 = w(0);
        let (dst_n, src_n) = valu_dwords(name);
        let compare = match self.encoding {
            Encoding::Vopc => true,
            Encoding::Vop3 => self.opcode < 0x100,
            _ => false,
        };
        let vop3b = VOP3B.contains(&name);
        let carry_in = name.ends_with("_co_ci_u32") || name == "v_cndmask_b32";
        let mut v = Valu::default();
        match self.encoding {
            Encoding::Vop1 | Encoding::Vop2 | Encoding::Vopc => {
                let ext = w(1);
                let mut src0 = w0 & 0x1FF;
                let vsrc1 = 256 + (w0 >> 9 & 0xFF);
                let mut src1 = vsrc1;
                match src0 {
                    0xF9 => {
                        let sdwa = Sdwa {
                            dst_sel: (ext >> 8 & 7) as u8,
                            dst_unused: (ext >> 11 & 3) as u8,
                            src_sel: [(ext >> 16 & 7) as u8, (ext >> 24 & 7) as u8],
                            sext: (ext >> 19 & 1 | ext >> 26 & 2) as u8,
                        };
                        // The s0/s1 bits select an SGPR or constant instead of a VGPR.
                        src0 = (ext & 0xFF) | if ext >> 23 & 1 == 0 { 256 } else { 0 };
                        if ext >> 31 == 1 {
                            src1 = vsrc1 - 256;
                        }
                        v.neg = (ext >> 20 & 1 | ext >> 27 & 2) as u8;
                        v.abs = (ext >> 21 & 1 | ext >> 28 & 2) as u8;
                        if compare {
                            if ext >> 15 & 1 == 1 {
                                v.dst = Some(scalar((ext >> 8 & 0x7F) as u8, lanes));
                            }
                            v.sdwa = Some(Sdwa {
                                dst_sel: 6,
                                dst_unused: 0,
                                ..sdwa
                            });
                        } else {
                            v.clamp = ext >> 13 & 1 == 1;
                            v.omod = (ext >> 14 & 3) as u8;
                            v.sdwa = Some(sdwa);
                        }
                    }
                    0xFA => {
                        src0 = 256 + (ext & 0xFF);
                        v.neg = (ext >> 20 & 1 | ext >> 21 & 2) as u8;
                        v.abs = (ext >> 21 & 1 | ext >> 22 & 2) as u8;
                        v.dpp = Some(Dpp::Row {
                            ctrl: (ext >> 8 & 0x1FF) as u16,
                            row_mask: (ext >> 28) as u8,
                            bank_mask: (ext >> 24 & 0xF) as u8,
                            bound_ctrl: ext >> 19 & 1 == 1,
                            fi: ext >> 18 & 1 == 1,
                        });
                    }
                    0xE9 | 0xEA => {
                        v.dpp = Some(Dpp::Lanes {
                            sel: ext >> 8,
                            fi: src0 == 0xEA,
                        });
                        src0 = 256 + (ext & 0xFF);
                    }
                    _ => {}
                }
                let op0 = source(src0, src_n[0], ext);
                let op1 = source(src1, src_n[1], ext);
                match self.encoding {
                    Encoding::Vopc => {
                        let mask = if name.starts_with("v_cmpx") {
                            EXEC
                        } else {
                            VCC
                        };
                        v.dst = v.dst.or(Some(scalar(mask, lanes)));
                        v.src = vec![op0, op1];
                    }
                    Encoding::Vop1 => {
                        v.dst = Some(valu_dst(name, w0 >> 17 & 0xFF, dst_n));
                        v.src = vec![op0];
                    }
                    _ => {
                        v.dst = Some(vector(w0 >> 17 & 0xFF, dst_n));
                        v.src = match name {
                            n if n.ends_with("mk_f32") || n.ends_with("mk_f16") => {
                                vec![op0, Operand::Literal(ext), op1]
                            }
                            n if n.ends_with("ak_f32") || n.ends_with("ak_f16") => {
                                vec![op0, op1, Operand::Literal(ext)]
                            }
                            _ => vec![op0, op1],
                        };
                        if carry_in {
                            v.src.push(scalar(VCC, lanes));
                        }
                        if name.ends_with("_co_ci_u32") {
                            v.carry = Some(scalar(VCC, lanes));
                        }
                    }
                }
            }
            Encoding::Vop3 | Encoding::Vop3p => {
                let w1 = w(1);
                let field = |i: u32| w1 >> (9 * i) & 0x1FF;
                let count = match self.opcode {
                    _ if self.encoding == Encoding::Vop3p => {
                        if name.starts_with("v_pk_")
                            && !name.contains("_fma_")
                            && !name.contains("_mad_")
                        {
                            2
                        } else {
                            3
                        }
                    }
                    0x000..0x100 => 2,
                    0x100..0x140 => 2 + usize::from(carry_in),
                    0x180..0x200 => 1,
                    _ if VOP3_TWO_SOURCES.contains(&name) => 2,
                    _ => 3,
                };
                v.src = (0..count)
                    .map(|i| {
                        let code = field(i as u32);
                        if i == 0 && name.starts_with("v_interp_") {
                            Operand::Attr {
                                index: (code & 0x3F) as u8,
                                chan: (code >> 6 & 3) as u8,
                                high: code >> 8 & 1 == 1,
                            }
                        } else if i == 2 && carry_in {
                            source(code, lanes, w(2))
                        } else {
                            source(code, src_n[i], w(2))
                        }
                    })
                    .collect();
                v.clamp = w0 >> 15 & 1 == 1;
                v.neg = (w1 >> 29) as u8;
                if self.encoding == Encoding::Vop3p {
                    v.neg_hi = (w0 >> 8 & 7) as u8;
                    v.opsel = (w0 >> 11 & 7) as u8;
                    v.opsel_hi = (w1 >> 27 & 3 | w0 >> 12 & 4) as u8;
                } else {
                    v.omod = (w1 >> 27 & 3) as u8;
                    if vop3b {
                        v.carry = Some(scalar((w0 >> 8 & 0x7F) as u8, lanes));
                    } else {
                        v.abs = (w0 >> 8 & 7) as u8;
                        v.opsel = (w0 >> 11 & 0xF) as u8;
                    }
                }
                let vdst = w0 & 0xFF;
                v.dst = Some(if compare {
                    scalar(vdst as u8, lanes)
                } else {
                    valu_dst(name, vdst, dst_n)
                });
            }
            _ => unreachable!("not a VALU encoding"),
        }
        v
    }
}

/// VOP3-only opcodes that take two sources (the rest take three).
const VOP3_TWO_SOURCES: &[&str] = &[
    "v_add_f64",
    "v_mul_f64",
    "v_min_f64",
    "v_max_f64",
    "v_ldexp_f64",
    "v_mul_lo_u32",
    "v_mul_hi_u32",
    "v_mul_lo_i32",
    "v_mul_hi_i32",
    "v_trig_preop_f64",
    "v_lshlrev_b64",
    "v_lshrrev_b64",
    "v_ashrrev_i64",
    "v_add_nc_u16",
    "v_sub_nc_u16",
    "v_mul_lo_u16",
    "v_lshrrev_b16",
    "v_ashrrev_i16",
    "v_max_u16",
    "v_max_i16",
    "v_min_u16",
    "v_min_i16",
    "v_add_nc_i16",
    "v_sub_nc_i16",
    "v_add_co_u32",
    "v_sub_co_u32",
    "v_subrev_co_u32",
    "v_pack_b32_f16",
    "v_cvt_pknorm_i16_f16",
    "v_cvt_pknorm_u16_f16",
    "v_lshlrev_b16",
    "v_interp_p1ll_f16",
    "v_readlane_b32",
    "v_writelane_b32",
    "v_ldexp_f32",
    "v_bfm_b32",
    "v_bcnt_u32_b32",
    "v_mbcnt_lo_u32_b32",
    "v_mbcnt_hi_u32_b32",
    "v_cvt_pknorm_i16_f32",
    "v_cvt_pknorm_u16_f32",
    "v_cvt_pk_u16_u32",
    "v_cvt_pk_i16_i32",
    "v_sub_nc_i32",
    "v_add_nc_i32",
];

/// VOP3 opcodes whose bits 14:8 hold a scalar destination instead of `abs`/`opsel`.
const VOP3B: &[&str] = &[
    "v_add_co_u32",
    "v_sub_co_u32",
    "v_subrev_co_u32",
    "v_add_co_ci_u32",
    "v_sub_co_ci_u32",
    "v_subrev_co_ci_u32",
    "v_div_scale_f32",
    "v_div_scale_f64",
    "v_mad_u64_u32",
    "v_mad_i64_i32",
];

fn scalar(code: u8, dwords: u8) -> Operand {
    Operand::Scalar { code, dwords }
}

fn vector(reg: u32, dwords: u8) -> Operand {
    Operand::Vector {
        reg: reg as u8,
        dwords,
    }
}

/// A 9-bit source field (8-bit scalar fields are the same codes below 256).
fn source(code: u32, dwords: u8, literal: u32) -> Operand {
    match code {
        0..=127 => scalar(code as u8, dwords),
        255 => Operand::Literal(literal),
        256.. => vector(code - 256, dwords),
        _ => Operand::Const(code as u8),
    }
}

/// VALU destinations that are SGPRs.
fn valu_dst(name: &str, field: u32, dwords: u8) -> Operand {
    match name {
        "v_readlane_b32" | "v_readfirstlane_b32" => scalar(field as u8, 1),
        _ => vector(field, dwords),
    }
}

/// LDS opcodes on two addresses (`ds_read2_b32`...), with `offset0`/`offset1`.
pub(super) fn ds_pair(name: &str) -> bool {
    name.split('_').any(|t| {
        ["read2", "write2", "wrxchg2"]
            .iter()
            .any(|p| t.starts_with(p))
    })
}

/// Dwords of a type suffix such as `b64` or `f32`.
fn type_dwords(token: &str) -> u8 {
    match token {
        "b64" | "u64" | "i64" | "f64" => 2,
        _ => 1,
    }
}

/// (sdst, ssrc0, ssrc1) dwords of a scalar ALU opcode.
fn salu_dwords(name: &str) -> (u8, u8, u8) {
    match name {
        "s_lshl_b64" | "s_lshr_b64" | "s_ashr_i64" | "s_bfe_u64" | "s_bfe_i64"
        | "s_bitcmp0_b64" | "s_bitcmp1_b64" => (2, 2, 1),
        "s_bfm_b64" | "s_bitset0_b64" | "s_bitset1_b64" | "s_bitreplicate_b64_b32" => (2, 1, 1),
        "s_bcnt0_i32_b64" | "s_bcnt1_i32_b64" | "s_ff0_i32_b64" | "s_ff1_i32_b64"
        | "s_flbit_i32_b64" | "s_flbit_i32_i64" => (1, 2, 2),
        _ => {
            let n = type_dwords(name.rsplit('_').next().unwrap_or(""));
            (n, n, n)
        }
    }
}

/// (vdst, [src0, src1, src2]) dwords of a VALU opcode.
fn valu_dwords(name: &str) -> (u8, [u8; 3]) {
    let parts: Vec<&str> = name.split('_').collect();
    match name {
        "v_lshlrev_b64" | "v_lshrrev_b64" | "v_ashrrev_i64" => (2, [1, 2, 2]),
        "v_ldexp_f64" | "v_trig_preop_f64" | "v_cmp_class_f64" | "v_cmpx_class_f64" => {
            (2, [2, 1, 2])
        }
        "v_mad_u64_u32" | "v_mad_i64_i32" => (2, [1, 1, 2]),
        "v_qsad_pk_u16_u8" | "v_mqsad_pk_u16_u8" => (2, [2, 1, 2]),
        "v_mqsad_u32_u8" => (4, [2, 1, 4]),
        "v_frexp_exp_i32_f64" => (1, [2; 3]),
        _ if parts.len() == 4 && parts[1] == "cvt" => {
            (type_dwords(parts[2]), [type_dwords(parts[3]); 3])
        }
        _ => {
            let n = type_dwords(parts.last().copied().unwrap_or(""));
            (n, [n; 3])
        }
    }
}

/// Dwords of loaded, stored or atomic data, from the opcode name.
fn data_dwords(name: &str) -> u8 {
    let mut n: u8 = 1;
    for token in name.split('_') {
        n = match token {
            "dwordx2" | "x2" | "xy" | "b64" | "u64" | "i64" | "f64" => 2,
            "dwordx3" | "xyz" | "b96" => 3,
            "dwordx4" | "xyzw" | "b128" => 4,
            "dwordx8" => 8,
            "dwordx16" => 16,
            _ => n,
        };
    }
    if name.contains("cmpswap") {
        n *= 2;
    }
    if name.contains("format_d16") {
        n = n.div_ceil(2);
    }
    n
}

/// Address dwords of an image opcode: offset, bias, compare, derivatives,
/// coordinates, then LOD/clamp/mip, packed in pairs with `a16`.
fn image_addr_dwords(name: &str, dim: u8, a16: bool) -> u8 {
    // (coordinates, derivatives per direction) for 1D 2D 3D CUBE 1D_ARRAY
    // 2D_ARRAY 2D_MSAA 2D_MSAA_ARRAY.
    const DIMS: [(u8, u8); 8] = [
        (1, 1),
        (2, 2),
        (3, 3),
        (3, 2),
        (2, 1),
        (3, 2),
        (3, 2),
        (4, 2),
    ];
    let (coords, grads) = DIMS[usize::from(dim & 7)];
    match name {
        "image_bvh_intersect_ray" => return if a16 { 8 } else { 11 },
        "image_bvh64_intersect_ray" => return if a16 { 9 } else { 12 },
        "image_get_resinfo" => return 1,
        _ => {}
    }
    let (mut whole, mut derivs, mut packed): (u8, u8, u8) = (0, 0, coords);
    if let Some(rest) = name
        .strip_prefix("image_sample")
        .or_else(|| name.strip_prefix("image_gather4"))
    {
        for token in rest.split('_') {
            match token {
                "o" | "b" | "c" => whole += 1,
                "d" | "cd" => derivs = grads,
                "l" | "cl" => packed += 1,
                _ => {}
            }
        }
    } else if name.contains("_mip") {
        packed += 1;
    }
    if a16 {
        whole + 2 * derivs.div_ceil(2) + packed.div_ceil(2)
    } else {
        whole + 2 * derivs + packed
    }
}
