// Consecutive per-lane VGPR writes all observe the same EXEC bit. Keep them in
// one predicated region. Resource, control-flow and mask-changing statements
// always close the region before they execute.
fn assignment(line: &str) -> Option<&str> {
    let assignment = line.strip_prefix("    if (exec_lo & 1u) ")?;
    let (dest, expr) = assignment.strip_suffix(';')?.split_once(" = ")?;
    let reg = dest.strip_prefix('v')?;
    if reg.is_empty()
        || !reg.bytes().all(|c| c.is_ascii_digit())
        || expr.contains(['[', ']', ';', '{', '}'])
        || expr.contains("++")
        || expr.contains("--")
    {
        return None;
    }
    // Only scalar temporaries, register components, literals and named pure
    // ALU helpers. In particular, no indexed, method, atomic or wave access.
    for token in expr
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
    {
        let first = token.as_bytes()[0];
        if first.is_ascii_digit()
            || matches!(
                token,
                "x" | "y"
                    | "z"
                    | "w"
                    | "vcc_lo"
                    | "vcc_hi"
                    | "tb_fp_mode"
                    | "float"
                    | "int"
                    | "uint"
                    | "asuint"
                    | "asfloat"
                    | "asint"
                    | "dot4add_i8packed"
                    | "tb_mull"
                    | "rsqrt"
                    | "sqrt"
                    | "f16tof32"
                    | "max"
                    | "min"
                    | "tb_mulhi_u32"
                    | "abs"
                    | "floor"
                    | "trunc"
                    | "sin"
                    | "tb_mode_half"
                    | "tb_f16_rne"
                    | "saturate"
                    | "ceil"
            )
        {
            continue;
        }
        let Some(tail) = token
            .strip_prefix("tb_s")
            .or_else(|| token.strip_prefix(['t', 'v', 's']))
        else {
            return None;
        };
        if !tail.as_bytes().first().is_some_and(u8::is_ascii_digit)
            || !tail
                .bytes()
                .all(|c| c.is_ascii_digit() || matches!(c, b'a' | b'b' | b'_'))
        {
            return None;
        }
    }
    Some(assignment)
}

pub(super) fn group(body: &str) -> String {
    use std::fmt::Write;
    let mut output = String::with_capacity(body.len());
    let mut open = false;
    for line in body.lines() {
        if let Some(write) = assignment(line) {
            if !open {
                output.push_str("    if (exec_lo & 1u) {\n");
                open = true;
            }
            writeln!(output, "        {write}").unwrap();
        } else {
            if open && !line.trim_start().starts_with("//") {
                output.push_str("    }\n");
                open = false;
            }
            writeln!(output, "{line}").unwrap();
        }
    }
    if open {
        output.push_str("    }\n");
    }
    output
}

#[cfg(test)]
#[path = "../tests/hlsl/exec_group.rs"]
mod tests;
