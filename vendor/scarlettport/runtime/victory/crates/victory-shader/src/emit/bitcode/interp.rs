//! A CPU interpreter for lowered modules (tests only): runs `main` of a
//! [`Module`] the lowering built, one lane, with every `dx.op` call answered
//! by the test (resources, inputs) so that what a translated program
//! computes can be compared with what the console program means.
//! Grown from the optimiser's interpreter of random programs (`opt.rs`),
//! with aggregates (`extractvalue` of `dx.op` results, constant structs).

use super::{Bin, Cast, Const, Module, Op, Term, TypeDef, V, pred};
use std::cell::RefCell;

thread_local! {
    static LAST: RefCell<Option<Module>> = const { RefCell::new(None) };
}

/// Keeps the module a lowering finished on this thread (`Shader::finish`).
pub(crate) fn keep(m: Module) {
    LAST.with(|l| *l.borrow_mut() = Some(m));
}

/// The module the last lowering on this thread finished.
pub(crate) fn last() -> Option<Module> {
    LAST.with(|l| l.borrow_mut().take())
}

fn mask(bits: u32) -> u64 {
    if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 }
}

fn signed(bits: u32, x: u64) -> i64 {
    let s = 64 - bits.min(64);
    ((x << s) as i64) >> s
}

impl Module {
    /// Bits of an integer type; None for floats and the rest.
    fn bits_of(&self, ty: super::Ty) -> Option<u32> {
        match self.types[ty.0 as usize] {
            TypeDef::Int(b) => Some(b),
            _ => None,
        }
    }

    fn const_value(&self, c: u32) -> Vec<u64> {
        let (ty, k) = &self.consts[c as usize];
        match k {
            Const::Int(x) => vec![*x as u64 & self.bits_of(*ty).map_or(u64::MAX, mask)],
            Const::Float(bits) => vec![u64::from(*bits)],
            Const::Undef => vec![0; self.width(*ty)],
            Const::Aggregate(fields) => fields.iter().flat_map(|&f| self.const_value(f)).collect(),
        }
    }

    /// Scalars in a value of type `ty` (structs flattened).
    fn width(&self, ty: super::Ty) -> usize {
        match &self.types[ty.0 as usize] {
            TypeDef::Struct(_, fields) => fields.iter().map(|&f| self.width(f)).sum(),
            TypeDef::Array(n, of) => *n as usize * self.width(*of),
            _ => 1,
        }
    }
}

/// Runs `main` of `m` (one lane). `call` answers each `dx.op` call: its
/// name without `dx.op.` and its arguments (the opcode first; each a list
/// of scalars, aggregates flattened); it returns the result's scalars (none
/// for void). Integers are kept masked to their width, floats as f32 bits.
/// Panics on an instruction it does not know or after `steps` blocks.
pub(crate) fn run(m: &Module, call: &mut dyn FnMut(&str, &[Vec<u64>]) -> Vec<u64>) {
    let mut vals: Vec<Option<Vec<u64>>> = vec![None; m.insts.len()];
    let eval = |vals: &[Option<Vec<u64>>], v: V| -> Vec<u64> {
        match v {
            V::Inst(i) => vals[i as usize].clone().expect("a value defined before its use"),
            V::Const(c) => m.const_value(c),
            _ => panic!("a global or function as a value"),
        }
    };
    let one = |vals: &[Option<Vec<u64>>], v: V| eval(vals, v)[0];
    let (mut b, mut from) = (0usize, usize::MAX);
    for _ in 0..100_000 {
        let blk = &m.blocks[b];
        let joined: Vec<(u32, Vec<u64>)> = blk
            .phis
            .iter()
            .map(|&p| {
                let Op::Phi(incoming) = &m.insts[p as usize].op else {
                    panic!("a join")
                };
                let v = incoming.iter().find(|x| x.1 == from).expect("a value for the edge taken");
                (p, eval(&vals, v.0))
            })
            .collect();
        for (p, x) in joined {
            vals[p as usize] = Some(x);
        }
        for &i in &blk.body {
            let inst = &m.insts[i as usize];
            let bits = m.bits_of(inst.ty);
            let float = |x: u64| f32::from_bits(x as u32);
            let r: Vec<u64> = match &inst.op {
                Op::Bin(op, x, y) => {
                    let (x, y) = (one(&vals, *x), one(&vals, *y));
                    vec![match bits {
                        Some(bits) => {
                            let r = match op {
                                Bin::Add => x.wrapping_add(y),
                                Bin::Sub => x.wrapping_sub(y),
                                Bin::Mul => x.wrapping_mul(y),
                                Bin::UDiv => x.checked_div(y).unwrap_or(0),
                                Bin::URem => x.checked_rem(y).unwrap_or(0),
                                Bin::SDiv => {
                                    signed(bits, x).checked_div(signed(bits, y)).unwrap_or(0) as u64
                                }
                                Bin::And => x & y,
                                Bin::Or => x | y,
                                Bin::Xor => x ^ y,
                                Bin::Shl => x.checked_shl(y as u32).unwrap_or(0),
                                Bin::LShr => x.checked_shr(y as u32).unwrap_or(0),
                                Bin::AShr => (signed(bits, x) >> y.min(63)) as u64,
                            };
                            r & mask(bits)
                        }
                        None => {
                            let (x, y) = (float(x), float(y));
                            let r = match op {
                                Bin::Add => x + y,
                                Bin::Sub => x - y,
                                Bin::Mul => x * y,
                                Bin::SDiv | Bin::UDiv => x / y,
                                _ => panic!("a float operation {op:?}"),
                            };
                            u64::from(r.to_bits())
                        }
                    }]
                }
                Op::Cmp(p, x, y) => {
                    let wide = m.bits_of(m.type_of(*x));
                    let (x, y) = (one(&vals, *x), one(&vals, *y));
                    let holds = match wide {
                        Some(bits) => {
                            let (sx, sy) = (signed(bits, x), signed(bits, y));
                            match *p {
                                pred::EQ => x == y,
                                pred::NE => x != y,
                                pred::IUGT => x > y,
                                pred::IUGE => x >= y,
                                pred::IULT => x < y,
                                pred::IULE => x <= y,
                                pred::SGT => sx > sy,
                                pred::SGE => sx >= sy,
                                pred::SLT => sx < sy,
                                _ => sx <= sy,
                            }
                        }
                        None => {
                            let (x, y) = (float(x), float(y));
                            if x.is_nan() || y.is_nan() {
                                p & 8 != 0
                            } else {
                                (p & 1 != 0 && x == y) || (p & 2 != 0 && x > y) || (p & 4 != 0 && x < y)
                            }
                        }
                    };
                    vec![u64::from(holds)]
                }
                Op::Select(c, t, f) => {
                    if one(&vals, *c) != 0 {
                        eval(&vals, *t)
                    } else {
                        eval(&vals, *f)
                    }
                }
                Op::Cast(op, x) => {
                    let from_bits = m.bits_of(m.type_of(*x));
                    let x = one(&vals, *x);
                    vec![match (op, from_bits, bits) {
                        (Cast::Bitcast, _, _) | (Cast::ZExt, _, _) => x,
                        (Cast::SExt, Some(f), Some(t)) => signed(f, x) as u64 & mask(t),
                        (Cast::Trunc, _, Some(t)) => x & mask(t),
                        (Cast::UiToFp, _, None) => u64::from((x as f32).to_bits()),
                        (Cast::SiToFp, Some(f), None) => u64::from((signed(f, x) as f32).to_bits()),
                        (Cast::FpToUi, _, Some(t)) => float(x) as u64 & mask(t),
                        (Cast::FpToSi, _, Some(t)) => float(x) as i64 as u64 & mask(t),
                        _ => panic!("a cast {op:?}"),
                    }]
                }
                Op::Extract(agg, k) => {
                    // Fields of the structs the lowering extracts from are
                    // scalars: field k is scalar k.
                    vec![eval(&vals, *agg)[*k as usize]]
                }
                Op::Call(f, args) => {
                    let a: Vec<Vec<u64>> = args.iter().map(|&v| eval(&vals, v)).collect();
                    let name = m.funcs[*f as usize].name.as_str();
                    let name = name.strip_prefix("dx.op.").unwrap_or(name);
                    call(name, &a)
                }
                op => panic!("an instruction the interpreter does not run: {op:?}"),
            };
            vals[i as usize] = Some(r);
        }
        from = b;
        b = match blk.term.expect("a terminator") {
            Term::Br(t) => t,
            Term::CondBr(c, t, f) => {
                if one(&vals, c) != 0 {
                    t
                } else {
                    f
                }
            }
            Term::Ret => return,
        };
    }
    panic!("no end after 100,000 blocks");
}
