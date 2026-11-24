//! Skip flags as bits of a few `uint` words.
//!
//! The structured form keeps one `bool jT` per forward-branch target and guards each
//! run of instructions with `if (!(jA || jB || ...))`. Every flag becomes an i1 phi of
//! constants in LLVM, the shape jump threading looks for: in 60310787 (200 flags, up
//! to 15 per guard) it was 80% of the 34 s that dxc -O1 spent, and 43% in a64767c5.
//! Packed into words, a set is `tb_jW |= K`, a clear `tb_jW &= ~K`, and a guard one
//! mask test per word, which jump threading cannot resolve per predecessor. Same
//! values on every path. TB_SKIP_FLAG_BITS=0 keeps the bools.
//!
//! Only programs with at least MIN_FLAGS flags change. Below that the bools compile in
//! a few seconds, and jump threading over them also removes code: 727f287e (36 flags)
//! compiled 3.9 s -> 1.1 s packed but its DXIL grew from 23 KB to 58 KB. Programs with
//! 135-200 flags took half the time packed with DXIL within 5%.
use std::collections::HashMap;

const MIN_FLAGS: usize = 128;

pub(super) fn enabled() -> bool {
    std::env::var("TB_SKIP_FLAG_BITS").map_or(true, |v| v.trim() != "0")
}

fn ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Flag number of `jN` at `at`, and its length, if it is a whole identifier.
fn flag_at(s: &[u8], at: usize) -> Option<(usize, usize)> {
    if s.get(at) != Some(&b'j') || (at > 0 && ident_char(s[at - 1])) {
        return None;
    }
    let digits = s[at + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || s.get(at + 1 + digits).is_some_and(|&c| ident_char(c)) {
        return None;
    }
    let n = std::str::from_utf8(&s[at + 1..at + 1 + digits]).ok()?.parse().ok()?;
    Some((n, 1 + digits))
}

fn mask_expr(bits: &HashMap<usize, (usize, u32)>, flags: &[usize]) -> String {
    let mut words: Vec<(usize, u32)> = Vec::new();
    for f in flags {
        let (w, b) = bits[f];
        match words.iter_mut().find(|(x, _)| *x == w) {
            Some((_, m)) => *m |= b,
            None => words.push((w, b)),
        }
    }
    words.sort_unstable();
    let terms: Vec<String> = words.iter().map(|(w, m)| format!("(tb_j{w} & {m:#x}u)")).collect();
    if terms.len() == 1 { terms[0].clone() } else { format!("({})", terms.join(" | ")) }
}

/// Rewrite the flags declared as `bool jN = false;` in `body`. None when there are fewer
/// than MIN_FLAGS.
pub(super) fn pack(body: &str) -> Option<String> {
    pack_from(body, MIN_FLAGS)
}

fn pack_from(body: &str, min_flags: usize) -> Option<String> {
    let mut order = Vec::new();
    for line in body.lines() {
        if let Some(n) = line.trim().strip_prefix("bool j").and_then(|r| r.strip_suffix(" = false;")) {
            if let Ok(n) = n.parse::<usize>() {
                order.push(n);
            }
        }
    }
    if order.is_empty() || order.len() < min_flags {
        return None;
    }
    let bits: HashMap<usize, (usize, u32)> =
        order.iter().enumerate().map(|(i, &n)| (n, (i / 32, 1u32 << (i % 32)))).collect();
    let words = order.len().div_ceil(32);
    let s = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut declared = false;
    let mut i = 0;
    while i < s.len() {
        // `bool jN = false;` declarations: the first becomes the words, the rest go.
        if s[i..].starts_with(b"bool j") && (i == 0 || !ident_char(s[i - 1])) {
            if let Some((n, len)) = flag_at(s, i + 5) {
                let tail = i + 5 + len;
                if bits.contains_key(&n) && s[tail..].starts_with(b" = false;") {
                    if !declared {
                        declared = true;
                        let decl: Vec<String> = (0..words).map(|w| format!("uint tb_j{w} = 0u;")).collect();
                        out.push_str(&decl.join(" "));
                    }
                    i = tail + b" = false;".len();
                    continue;
                }
            }
        }
        // `!(jA || jB ...)` and `!jN`: one mask test.
        if s[i] == b'!' {
            if s.get(i + 1) == Some(&b'(') {
                let mut at = i + 2;
                let mut flags = Vec::new();
                let ok = loop {
                    let Some((n, len)) = flag_at(s, at).filter(|(n, _)| bits.contains_key(n)) else { break false };
                    flags.push(n);
                    at += len;
                    if s[at..].starts_with(b" || ") {
                        at += 4;
                    } else {
                        break s.get(at) == Some(&b')');
                    }
                };
                if ok {
                    out.push_str(&format!("!{}", mask_expr(&bits, &flags)));
                    i = at + 1;
                    continue;
                }
            }
            if let Some((n, len)) = flag_at(s, i + 1).filter(|(n, _)| bits.contains_key(n)) {
                out.push_str(&format!("!{}", mask_expr(&bits, &[n])));
                i += 1 + len;
                continue;
            }
        }
        if let Some((n, len)) = flag_at(s, i).filter(|(n, _)| bits.contains_key(n)) {
            let (w, b) = bits[&n];
            let tail = &s[i + len..];
            if tail.starts_with(b" = true;") {
                out.push_str(&format!("tb_j{w} |= {b:#x}u;"));
                i += len + b" = true;".len();
            } else if tail.starts_with(b" = false;") {
                out.push_str(&format!("tb_j{w} &= ~{b:#x}u;"));
                i += len + b" = false;".len();
            } else {
                out.push_str(&format!("({} != 0u)", mask_expr(&bits, &[n])));
                i += len;
            }
            continue;
        }
        // Copy one UTF-8 character.
        let ch = body[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::pack_from;

    #[test]
    fn flags_become_word_bits() {
        let body = "    bool j4 = false;\n    bool j9 = false;\n    if (scc != 0u) j4 = true;\n    if (!(j4 || j9)) {\n    x = !j9 && y;\n    tb_c3 = tb_c3 || j4;\n    j9 = false;\n    adj12 = j41;\n";
        let want = "    uint tb_j0 = 0u;\n    \n    if (scc != 0u) tb_j0 |= 0x1u;\n    if (!(tb_j0 & 0x3u)) {\n    x = !(tb_j0 & 0x2u) && y;\n    tb_c3 = tb_c3 || ((tb_j0 & 0x1u) != 0u);\n    tb_j0 &= ~0x2u;\n    adj12 = j41;\n";
        assert_eq!(pack_from(body, 1).unwrap(), want);
        assert!(pack_from(body, 3).is_none());
    }
}
