//! BCPack (Xbox Series DirectStorage texture compression) decoder: a 16- or
//! 12-lane rANS stream that rebuilds BC7 (modes 0-7) or BC1 blocks.
//! Ported from Xbox2Windows src/startup_stubs/bcpack_decoder.h (3795037e,
//! 90e84561, MIT), itself adapted from bcpack-rs by Ahmet Dedeler
//! (https://github.com/Ahmet-Dedeler/bcpack-rs, MIT OR Apache-2.0) with the
//! per-lane decoder from Microsoft silicon-rans (MIT). Notices in
//! bcpack/NOTICE.md. A stream that does not decode exactly (a flushed
//! fragment off the 0x1000000 home state, an unmapped BC7 control code,
//! trailing non-zero bytes) is refused; the caller falls back to grey blocks.
//! TUXBOX_BCPACK_DECODE=0 skips the decoder.
mod tables;
#[cfg(test)]
mod vectors;

use tables::*;

pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_BCPACK_DECODE").map_or(true, |v| v.trim() != "0"))
}

const HOME: u32 = 0x100_0000;

fn model_forward(eab: usize, q: usize, sym: u16) -> (u32, u32) {
    let s = u32::from(sym);
    let seg = (32 - s.leading_zeros()) as usize;
    let st = if seg == 0 { 0 } else { 1u32 << (seg - 1) };
    let fa = u32::from(FREQ_TABLE[eab][q][seg]);
    let cfa = u32::from(BASE_TABLE[eab][q][seg]).wrapping_add(fa.wrapping_mul(s.wrapping_sub(st))) & 0xffff;
    if cfa == 0 { (fa.wrapping_sub(1) & 0xffff, 1) } else { (fa, cfa) }
}

fn model_advance(eab: usize, q: usize, x: u32, sym: u16) -> u32 {
    let (f, cf) = model_forward(eab, q, sym);
    f.wrapping_mul(x >> 16).wrapping_add(x & 0xffff).wrapping_sub(cf)
}

fn model_inverse(eab: usize, q: usize, cf: u16) -> u16 {
    let c = u32::from(cf);
    let mut seg = 0;
    for k in 1..eab + 2 {
        if c >= u32::from(BASE_TABLE[eab][q][k]) { seg = k } else { break }
    }
    let base_x = if seg == 0 { 0 } else { 1u32 << (seg - 1) };
    let dist = c.wrapping_sub(u32::from(BASE_TABLE[eab][q][seg])).wrapping_add(u32::from(RN_TABLE[eab][q][seg])) & 0xffff;
    let mul = dist.wrapping_mul(RF_TABLE[eab][q][seg]);
    let steps = ((mul >> 15) >> RS_TABLE[eab][q][seg]) & 0x1ff;
    ((base_x + steps) & 0x1ff) as u16
}

fn unzigzag(v: u16) -> i32 {
    let v = i32::from(v);
    if v & 1 != 0 { -((v + 1) >> 1) } else { v >> 1 }
}

fn residual(sym: u16, prec: u32) -> u8 {
    (unzigzag(sym) & ((1 << prec) - 1)) as u8
}

fn interleave(even: u16, odd: u16) -> u8 {
    (0..4).fold(0u8, |out, k| out | (((even >> k) & 1) as u8) << (2 * k) | (((odd >> k) & 1) as u8) << (2 * k + 1))
}

fn deinterleave(b: u8) -> (u16, u16) {
    (0..4).fold((0, 0), |(even, odd), k| {
        (even | u16::from((b >> (2 * k)) & 1) << k, odd | u16::from((b >> (2 * k + 1)) & 1) << k)
    })
}

fn index_pair(sym: u16, row: usize) -> (u16, u16) {
    let (even, odd) = deinterleave(sym as u8);
    if row < 3 { (odd, even) } else { (even, odd) }
}

#[derive(Clone, Copy, PartialEq, PartialOrd)]
enum State {
    Idle,
    Hdr0,
    Hdr1,
    Hdr2,
    Proc,
    Drain,
}

#[derive(Clone, Copy)]
struct Lane {
    ab: u8,
    target: u32,
    state: State,
    ib: u8,
    ib_full: bool,
    sym: u16,
    sym_full: bool,
    in_rem: u32,
    out_rem: u32,
    p0: u32,
    p1: u32,
    hdr3: bool,
    flush: bool,
    virtual_zeros: u8,
    q: usize,
    eab: usize,
    done: bool,
    go: bool,
    stall: bool,
    nsyms: u32,
    final_state: u32,
}

impl Lane {
    fn new(ab: u8, target: u32) -> Self {
        Self {
            ab, target, state: State::Idle, ib: 0, ib_full: false, sym: 0, sym_full: false, in_rem: 0, out_rem: 0,
            p0: 0, p1: 0, hdr3: false, flush: false, virtual_zeros: 0, q: 0, eab: 0, done: false, go: true,
            stall: false, nsyms: 0, final_state: 0,
        }
    }

    /// One cycle of the lane's state machine; false refuses the stream.
    fn step(&mut self) -> bool {
        match self.state {
            State::Idle => {
                if self.go {
                    self.go = false;
                    self.state = State::Hdr0;
                    self.out_rem = self.target;
                    self.p0 = 0;
                    self.p1 = 0;
                }
            }
            State::Hdr0 => {
                if self.ib_full {
                    let b = self.ib;
                    self.ib_full = false;
                    self.in_rem = 0;
                    self.state = State::Hdr1;
                    self.virtual_zeros = 0;
                    self.q = usize::from(b & 15);
                    self.hdr3 = b & 0x20 != 0;
                    self.flush = b & 0x10 != 0;
                    let e = (i32::from(self.ab) - i32::from(b >> 6)) & 15;
                    self.eab = if e < 10 { e as usize } else { 9 };
                    if self.eab > 8 {
                        return false;
                    }
                }
            }
            State::Hdr1 => {
                if self.ib_full {
                    self.ib_full = false;
                    if self.hdr3 {
                        self.in_rem = u32::from(self.ib);
                        self.state = State::Hdr2;
                    } else {
                        self.in_rem = u32::from(self.ib) + 1;
                        self.state = State::Proc;
                    }
                }
            }
            State::Hdr2 => {
                if self.ib_full {
                    self.ib_full = false;
                    self.in_rem = ((u32::from(self.ib) << 8) | (self.in_rem & 0xff)) + 257;
                    self.state = State::Proc;
                }
            }
            State::Proc | State::Drain => {
                let is_proc = self.state == State::Proc;
                let will = is_proc && self.p0 < HOME && self.in_rem > 0 && self.ib_full;
                let mut x = if will { ((self.p0 & 0xff_ffff) << 8) | u32::from(self.ib) } else { self.p0 };
                let remaining = self.in_rem - u32::from(will);
                if self.flush && remaining == 0 {
                    while x > 0 && x < HOME && self.virtual_zeros < 2 {
                        x <<= 8;
                        self.virtual_zeros += 1;
                    }
                }
                let nsf = x > HOME && self.out_rem > 0;
                if nsf {
                    self.out_rem -= 1;
                    self.sym = model_inverse(self.eab, self.q, x as u16);
                }
                let new_out = self.out_rem;
                self.p1 = x;
                self.sym_full = nsf;
                let finish = |lane: &mut Self| {
                    lane.done = true;
                    lane.state = State::Idle;
                    lane.final_state = if nsf { model_advance(lane.eab, lane.q, x, lane.sym) } else { x };
                };
                if is_proc {
                    if will {
                        self.in_rem -= 1;
                        self.ib_full = false;
                    }
                    if self.in_rem == 0 {
                        if new_out > 0 {
                            if x == HOME {
                                self.state = State::Hdr0;
                                self.virtual_zeros = 0;
                                if self.flush {
                                    self.p1 = 0;
                                }
                            } else {
                                self.state = State::Drain;
                            }
                        } else {
                            finish(self);
                        }
                    }
                } else if x <= HOME {
                    if new_out > 0 {
                        // A flushed fragment must come back to the home state
                        // exactly; anything below it is a damaged fragment.
                        if self.flush && x != HOME {
                            return false;
                        }
                        self.state = State::Hdr0;
                        self.virtual_zeros = 0;
                        if self.flush {
                            self.p0 = 0;
                            self.p1 = 0;
                        }
                    } else {
                        finish(self);
                    }
                }
            }
        }
        true
    }
}

/// Decodes `per_lane` symbols for each lane (alphabet bits per lane), lane
/// after lane in the result. The lanes take input bytes in turn, at most 8
/// per cycle, as the hardware does.
fn decode_lanes(input: &[u8], alphabet_bits: &[u8], per_lane: usize) -> Option<Vec<u16>> {
    let lanes_n = alphabet_bits.len();
    if lanes_n == 0 || lanes_n > 16 || per_lane == 0 || alphabet_bits.iter().any(|&b| b > 8) {
        return None;
    }
    let target = u32::try_from(per_lane).ok()?;
    let mut symbols = Vec::new();
    symbols.try_reserve_exact(lanes_n * per_lane).ok()?;
    symbols.resize(lanes_n * per_lane, 0u16);
    let mut lanes: Vec<Lane> = alphabet_bits.iter().map(|&ab| Lane::new(ab, target)).collect();
    let (mut slots, mut pos, mut exhausted) = (0u32, 0usize, 0u32);
    let max_cycles = 64u64 * (lanes_n * per_lane + input.len()) as u64 + 1000;
    let (mut total_syms, mut last_pos, mut last_total, mut last_sum, mut last_change) = (0u64, usize::MAX, u64::MAX, u32::MAX, 0u64);
    let mut cycle = 0u64;
    while cycle < max_cycles {
        let mut served = 0;
        for (i, lane) in lanes.iter_mut().enumerate() {
            let bit = 1u32 << i;
            let blocked_out = lane.sym_full && slots & bit != 0;
            let need = !lane.ib_full
                && ((State::Hdr0..=State::Hdr2).contains(&lane.state) || (lane.state == State::Proc && lane.in_rem > 0));
            let mut byte = None;
            if need && !blocked_out && served < 8 && exhausted & bit == 0 {
                match input.get(pos) {
                    Some(&b) => byte = Some(b),
                    None => exhausted |= bit,
                }
            }
            lane.stall = (need && byte.is_none()) || blocked_out;
            if lane.stall {
                continue;
            }
            if matches!(lane.state, State::Proc | State::Drain) {
                lane.p0 = if lane.sym_full { model_advance(lane.eab, lane.q, lane.p1, lane.sym) } else { lane.p1 };
            }
            if let Some(b) = byte {
                lane.ib = b;
                lane.ib_full = true;
                pos += 1;
                served += 1;
            }
            if lane.sym_full {
                slots |= bit;
                *symbols.get_mut(i * per_lane + lane.nsyms as usize)? = lane.sym;
                lane.nsyms += 1;
                total_syms += 1;
                lane.sym_full = false;
            }
        }
        let (mut active, mut finished, mut state_sum) = (0u32, true, 0u32);
        for (i, lane) in lanes.iter_mut().enumerate() {
            if !lane.stall && !lane.step() {
                return None;
            }
            if !(lane.done && !lane.sym_full && slots & (1 << i) == 0) {
                active |= 1 << i;
            }
            finished &= lane.done && !lane.sym_full;
            state_sum += lane.state as u32;
        }
        if slots != 0 && slots & active == active {
            slots = 0;
        }
        cycle += 1;
        if finished {
            let homes = lanes.iter().all(|l| matches!(l.final_state, HOME | 0x10000 | 0x100));
            return (homes && input[pos.min(input.len())..].iter().all(|&b| b == 0)).then_some(symbols);
        }
        if pos != last_pos || total_syms != last_total || state_sum != last_sum {
            (last_pos, last_total, last_sum, last_change) = (pos, total_syms, state_sum, cycle);
        } else if cycle - last_change > 64 {
            return None;
        }
    }
    None
}

const SUBSETS: [usize; 8] = [3, 2, 3, 2, 1, 1, 1, 2];
const CHANNEL_BITS: [[u32; 4]; 8] = [
    [4, 4, 4, 0], [6, 6, 6, 0], [5, 5, 5, 0], [7, 7, 7, 0],
    [5, 5, 5, 6], [7, 7, 7, 8], [7, 7, 7, 7], [5, 5, 5, 5],
];
const INDEX_BITS: [[u32; 2]; 8] = [[3, 0], [3, 0], [2, 0], [2, 0], [3, 2], [2, 2], [4, 0], [2, 0]];
const PARTITION_BITS: [u32; 8] = [4, 6, 6, 6, 0, 0, 0, 6];
const ANCHOR_2: [u8; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15,
    15, 2, 8, 2, 2, 8, 8, 15, 2, 8, 2, 2, 8, 8, 2, 2,
    15, 15, 6, 8, 2, 8, 15, 15, 2, 8, 2, 2, 2, 15, 15, 6,
    6, 2, 6, 8, 15, 15, 2, 2, 15, 15, 15, 15, 15, 2, 2, 15,
];
const ANCHOR_3A: [u8; 64] = [
    3, 3, 15, 15, 8, 3, 15, 15, 8, 8, 6, 6, 6, 5, 3, 3,
    3, 3, 8, 15, 3, 3, 6, 10, 5, 8, 8, 6, 8, 5, 15, 15,
    8, 15, 3, 5, 6, 10, 8, 15, 15, 3, 15, 5, 15, 15, 15, 15,
    3, 15, 5, 5, 5, 8, 5, 10, 5, 10, 8, 13, 15, 12, 3, 3,
];
const ANCHOR_3B: [u8; 64] = [
    15, 8, 8, 3, 15, 15, 3, 8, 15, 15, 15, 15, 15, 15, 15, 8,
    15, 8, 15, 3, 15, 8, 15, 8, 3, 15, 6, 10, 15, 15, 10, 8,
    15, 3, 15, 10, 10, 8, 9, 10, 6, 15, 8, 15, 3, 6, 6, 8,
    15, 3, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 3, 15, 15, 8,
];
const INDEX_LANES: [[usize; 4]; 2] = [[11, 12, 13, 14], [6, 7, 8, 10]];

fn endpoint_lane(subsets: usize, row: usize, ch: usize) -> Option<usize> {
    match (row, ch) {
        (0 | 1, 3) if subsets == 3 => None,
        (0, _) => Some(2 + ch),
        (1, _) => Some(6 + ch),
        (2, 0) => Some(10),
        (2, 1) if subsets == 3 => Some(5),
        (2, 2) if subsets == 3 => Some(9),
        _ => None,
    }
}

struct Bits {
    value: u128,
    at: u32,
}

impl Bits {
    fn put(&mut self, value: u32, n: u32) {
        if n == 0 {
            return;
        }
        if self.at < 128 {
            self.value |= u128::from(value & ((1u32 << n) - 1)) << self.at;
        }
        self.at += n;
    }
}

#[derive(Clone, Copy, Default)]
struct Bc7State {
    subsets: usize,
    ep: [[u8; 8]; 3],
    ix: [[u8; 16]; 2],
}

fn has_pbits(mode: usize) -> bool {
    matches!(mode, 0 | 1 | 3 | 6 | 7)
}

fn precisions(c: &Bc7Control) -> [u32; 4] {
    let mode = usize::from(c.mode);
    let mut prec = CHANNEL_BITS[mode];
    if c.rotation != 0 {
        prec.swap(3, usize::from(c.rotation) - 1);
    } else if has_pbits(mode) {
        prec[0] += 1;
    }
    prec
}

fn pack_bc7_block(c: &Bc7Control, st: &Bc7State, out: &mut [u8]) {
    let mode = usize::from(c.mode);
    let subsets = SUBSETS[mode];
    let prec = precisions(c);
    let pbits = has_pbits(mode);
    let mut w = Bits { value: 0, at: 0 };
    w.put(1 << mode, mode as u32 + 1);
    w.put(u32::from(c.partition), PARTITION_BITS[mode]);
    if mode == 4 || mode == 5 {
        w.put(u32::from(c.rotation), 2);
    }
    if mode == 4 {
        w.put(u32::from(c.index_selection), 1);
    }
    let mut order = [0usize, 1, 2, 3];
    if c.rotation != 0 {
        order.swap(3, usize::from(c.rotation) - 1);
    }
    for ch in order {
        let p = prec[ch];
        if p == 0 {
            continue;
        }
        let drop = u32::from(ch == 0 && pbits);
        for e in 0..2 * subsets {
            let field = u32::from(st.ep[e / 2][2 * ch + e % 2]) >> (8 - p);
            w.put(field >> drop, p - drop);
        }
    }
    if mode == 1 {
        for row in 0..subsets {
            w.put((u32::from(st.ep[row][0]) >> (8 - prec[0])) & 1, 1);
        }
    } else if pbits {
        for e in 0..2 * subsets {
            w.put((u32::from(st.ep[e / 2][e % 2]) >> (8 - prec[0])) & 1, 1);
        }
    }
    let part = usize::from(c.partition & 63);
    let anchors: u32 = match subsets {
        2 => 1 | 1 << ANCHOR_2[part],
        3 => 1 | 1 << ANCHOR_3A[part] | 1 << ANCHOR_3B[part],
        _ => 1,
    };
    let write_set = |w: &mut Bits, s: usize| {
        let b = INDEX_BITS[mode][s];
        for p in 0..16 {
            w.put(u32::from(st.ix[s][p]) >> (4 - b), b - ((anchors >> p) & 1));
        }
    };
    match mode {
        4 => { write_set(&mut w, 1); write_set(&mut w, 0); }
        5 => { write_set(&mut w, 0); write_set(&mut w, 1); }
        _ => write_set(&mut w, 0),
    }
    out[..16].copy_from_slice(&w.value.to_le_bytes());
}

fn decode_bc7_blocks(symbols: &[u16], blocks: usize, output: &mut [u8]) -> bool {
    if output.len() < blocks * 16 {
        return false;
    }
    let target = blocks * 2;
    let sym = |lane: usize, n: usize| symbols[lane * target + n];
    let mut hist = [Bc7State::default(); 256];
    for i in 0..blocks {
        let d_byte = interleave(sym(0, 2 * i), sym(0, 2 * i + 1));
        let x_byte = interleave(sym(1, 2 * i), sym(1, 2 * i + 1));
        let er = ((i32::from(x_byte) + unzigzag(u16::from(d_byte))) & 0xff) as usize;
        let xr = usize::from(x_byte);
        let code = usize::from(((sym(15, 2 * i + 1) & 15) << 5) | (sym(15, 2 * i) & 31));
        // An unmapped control code means a damaged stream (the reference
        // emits a zero block; we refuse it).
        let Some(c) = BC7_CONTROL_TABLE.get(code).filter(|c| c.valid) else { return false };
        let source = |r: usize| -> Bc7State {
            if r == 0 {
                Bc7State::default()
            } else if r <= i {
                hist[(i - r) & 255]
            } else {
                BC7_DICTIONARY.get(r - i - 1).map_or_else(Bc7State::default, |e| Bc7State {
                    subsets: usize::from(e.subsets),
                    ep: e.ep,
                    ix: e.ix,
                })
            }
        };
        let (er_state, xr_state) = (source(er), source(xr));
        let mode = usize::from(c.mode);
        let mut st = Bc7State { subsets: SUBSETS[mode], ..Default::default() };
        let prec = precisions(c);
        let common = st.subsets.min(er_state.subsets);
        for (ch, &p) in prec.iter().enumerate() {
            if p == 0 {
                continue;
            }
            for row in 0..st.subsets {
                let from = if row < common { row } else { 0 };
                let lane = endpoint_lane(st.subsets, row, ch);
                for e in 0..2 {
                    let col = 2 * ch + e;
                    let res = lane.map_or(0, |l| residual(sym(l, 2 * i + e), p));
                    let base = u32::from(er_state.ep[from][col]) >> (8 - p);
                    st.ep[row][col] = (((base + u32::from(res)) & ((1 << p) - 1)) << (8 - p)) as u8;
                }
            }
        }
        for set in 0..2 {
            let b = INDEX_BITS[mode][set];
            if b == 0 {
                continue;
            }
            for row in 0..4 {
                let lane = INDEX_LANES[set][row];
                for half in 0..2 {
                    let (l, r) = index_pair(sym(lane, 2 * i + half), row);
                    for (h, v) in [l, r].into_iter().enumerate() {
                        let p = 4 * row + 2 * half + h;
                        let res = residual(v, b);
                        let base = u32::from(xr_state.ix[set][p]) >> (4 - b);
                        st.ix[set][p] = (((base + u32::from(res)) & ((1 << b) - 1)) << (4 - b)) as u8;
                    }
                }
            }
        }
        pack_bc7_block(c, &st, &mut output[i * 16..]);
        hist[i & 255] = st;
    }
    true
}

fn decode_bc1_blocks(symbols: &[u16], target: usize, output: &mut [u8]) -> bool {
    if output.len() < target * 8 {
        return false;
    }
    const WIDTHS: [u32; 6] = [5, 6, 5, 5, 6, 5];
    let mut hist = [[0u8; 6]; 256];
    for i in 0..target {
        let v = usize::from(symbols[i] & 0xff);
        let d = if v & 1 != 0 { 255 - (v >> 1) } else { v >> 1 };
        let pred = if d == 0 {
            [0; 6]
        } else if d <= i {
            hist[(i - d) & 255]
        } else {
            BC1_WINDOW[(i as isize - d as isize) as usize & 255]
        };
        let mut e = [0u8; 6];
        for k in 0..6 {
            let res = unzigzag(symbols[(2 + k) * target + i]);
            e[k] = ((i32::from(pred[k]) + res) & ((1 << WIDTHS[k]) - 1)) as u8;
        }
        hist[i & 255] = e;
        let c0 = u16::from(e[0]) << 11 | u16::from(e[1]) << 5 | u16::from(e[2]);
        let c1 = u16::from(e[3]) << 11 | u16::from(e[4]) << 5 | u16::from(e[5]);
        let w = (0..4).fold(0u32, |w, q| w | BC1_QUAD_MASKS[q][usize::from(symbols[(8 + q) * target + i] & 0xff)]);
        let block = &mut output[i * 8..i * 8 + 8];
        block[..2].copy_from_slice(&c0.to_le_bytes());
        block[2..4].copy_from_slice(&c1.to_le_bytes());
        block[4..].copy_from_slice(&w.to_le_bytes());
    }
    true
}

/// Which BCPack modes decode: BC1 (1) and BC7 (7) here; BC3, BC4, BC5 and
/// BC6H (3-6) through `bcpack_victory` (TUXBOX_BCPACK_VICTORY=0 fills them
/// with solid blocks, as before). test140 filled 64+ BC5 requests solid.
pub(crate) fn decodes(mode: u8) -> bool {
    matches!(mode, 1 | 7) || (matches!(mode, 3..=6) && victory_on())
}

fn victory_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_BCPACK_VICTORY").map_or(true, |v| v.trim() != "0"))
}

/// Decodes a BCPack stream of `mode` filling all of `output`; false refuses it.
pub(crate) fn decode(input: &[u8], mode: u8, output: &mut [u8]) -> bool {
    if input.is_empty() || output.is_empty() {
        return false;
    }
    match mode {
        7 if output.len().is_multiple_of(16) => {
            let blocks = output.len() / 16;
            const AB: [u8; 16] = [3, 3, 7, 7, 7, 7, 7, 6, 6, 4, 5, 7, 7, 7, 7, 4];
            decode_lanes(input, &AB, blocks * 2).is_some_and(|s| decode_bc7_blocks(&s, blocks, output))
        }
        1 if output.len().is_multiple_of(8) => {
            let blocks = output.len() / 8;
            const AB: [u8; 12] = [7, 7, 4, 5, 4, 4, 5, 4, 7, 7, 7, 7];
            decode_lanes(input, &AB, blocks).is_some_and(|s| decode_bc1_blocks(&s, blocks, output))
        }
        3..=6 if victory_on() => match crate::bcpack_victory::decode(mode, input, output.len()) {
            Ok(blocks) => {
                output.copy_from_slice(&blocks);
                true
            }
            Err(_) => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, vectors::*};

    #[test]
    fn bc7_mode6_blocks_decode_exactly() {
        let mut out = [0u8; 32];
        assert!(decode(&BC7_PACKED, 7, &mut out));
        assert_eq!(out, BC7_EXPECTED);
    }

    #[test]
    fn bc1_stream_decodes_exactly() {
        let mut out = [0u8; 512];
        assert!(decode(&BC1_PACKED, 1, &mut out));
        assert_eq!(out, BC1_EXPECTED);
    }

    #[test]
    fn truncated_and_trailing_streams_are_refused() {
        let mut out = [0u8; 32];
        assert!(!decode(&BC7_PACKED[..32], 7, &mut out));
        let mut trailing = BC1_PACKED.to_vec();
        trailing.push(1);
        let mut out = [0u8; 512];
        assert!(!decode(&trailing, 1, &mut out));
        assert!(!decode(&BC1_PACKED, 2, &mut out));
    }
}
