//! The generator of `patterns.rs`: reads the swizzle patterns of Mesa's
//! addrlib (`src/amd/addrlib/src/gfx10/gfx10SwizzlePattern.h`, MIT) and
//! writes the blocks the console's modes use as Rust.
//!
//! The header describes each block pattern by a row of a `*_PATINFO` table
//! (one per mode, with the RB+ tables for GFX10.3) whose four indices pick
//! address bits 0-7, 8-11, 12-15 and 16-19 from the `NIBBLE` tables. A
//! table's rows run over pipe configurations, five element sizes each; for
//! the modes without pipe or bank XOR only the first five rows count.
//!
//! Regenerate with the header at hand (a Mesa checkout's `src/amd/addrlib`
//! or the file itself):
//! `VICTORY_ADDRLIB=<path> VICTORY_WRITE_PATTERNS=1 cargo test -p victory-formats
//! patterns_are_generated_from_addrlib -- --ignored`. Without
//! `VICTORY_WRITE_PATTERNS` the test compares instead. `patterns.rs` was made
//! from Mesa commit 265a2d4f0f842a28e0dfd150637fa91fb70345a2 ("amd: import
//! addrlib v10.9").

use std::collections::HashMap;
use std::fmt::Write;

use super::{Pipes, Swizzle};

/// The blocks written out: the table, the mode, and whether it is the 3D
/// pattern of a thick block.
#[rustfmt::skip]
const BLOCKS: &[(&str, Swizzle, bool)] = &[
    ("GFX10_SW_256_S_RBPLUS_PATINFO", Swizzle::SW_256B_S, false),
    ("GFX10_SW_256_D_RBPLUS_PATINFO", Swizzle::SW_256B_D, false),
    ("GFX10_SW_4K_S_RBPLUS_PATINFO", Swizzle::SW_4KB_S, false),
    ("GFX10_SW_4K_D_RBPLUS_PATINFO", Swizzle::SW_4KB_D, false),
    ("GFX10_SW_64K_S_RBPLUS_PATINFO", Swizzle::SW_64KB_S, false),
    ("GFX10_SW_64K_D_RBPLUS_PATINFO", Swizzle::SW_64KB_D, false),
    ("GFX10_SW_4K_S3_RBPLUS_PATINFO", Swizzle::SW_4KB_S, true),
    ("GFX10_SW_64K_S3_RBPLUS_PATINFO", Swizzle::SW_64KB_S, true),
    ("GFX10_SW_64K_S_T_RBPLUS_PATINFO", Swizzle::SW_64KB_S_T, false),
    ("GFX10_SW_64K_D_T_RBPLUS_PATINFO", Swizzle::SW_64KB_D_T, false),
    ("GFX10_SW_4K_S_X_RBPLUS_PATINFO", Swizzle::SW_4KB_S_X, false),
    ("GFX10_SW_4K_D_X_RBPLUS_PATINFO", Swizzle::SW_4KB_D_X, false),
    ("GFX10_SW_64K_Z_X_1xaa_RBPLUS_PATINFO", Swizzle::SW_64KB_Z_X, false),
    ("GFX10_SW_64K_S_X_RBPLUS_PATINFO", Swizzle::SW_64KB_S_X, false),
    ("GFX10_SW_64K_D_X_RBPLUS_PATINFO", Swizzle::SW_64KB_D_X, false),
    ("GFX10_SW_64K_R_X_1xaa_RBPLUS_PATINFO", Swizzle::SW_64KB_R_X, false),
    ("GFX10_SW_64K_S3_T_RBPLUS_PATINFO", Swizzle::SW_64KB_S_T, true),
    ("GFX10_SW_4K_S3_X_RBPLUS_PATINFO", Swizzle::SW_4KB_S_X, true),
    ("GFX10_SW_64K_S3_X_RBPLUS_PATINFO", Swizzle::SW_64KB_S_X, true),
    ("GFX10_SW_64K_D3_X_RBPLUS_PATINFO", Swizzle::SW_64KB_D_X, true),
];

/// The pipe configurations written out for the XOR modes.
const CONFIGS: [(&str, Pipes); 2] = [("SERIES_X", Pipes::SERIES_X), ("SERIES_S", Pipes::SERIES_S)];

/// The rows of one table, or the nibbles of one `NIBBLE` table: the
/// comma-separated fields between braces, and the line's comment.
fn table<'a>(header: &'a str, name: &str) -> Result<Vec<(Vec<&'a str>, &'a str)>, String> {
    let start = header
        .find(&format!(" {name}[]"))
        .ok_or_else(|| format!("no table {name}"))?;
    let body = &header[start..];
    let open = body
        .find("=\n{")
        .ok_or_else(|| format!("{name}: no body"))?;
    let end = body.find("\n};").ok_or_else(|| format!("{name}: no end"))?;
    let mut rows = vec![];
    for line in body[open + 3..end].lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (fields, comment) = line.split_once("//").unwrap_or((line, ""));
        let fields = fields
            .trim()
            .strip_prefix('{')
            .and_then(|f| f.trim_end_matches([' ', ',']).strip_suffix('}'))
            .ok_or_else(|| format!("{name}: odd row {line}"))?;
        let fields = fields
            .split(',')
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .collect();
        rows.push((fields, comment.trim()));
    }
    Ok(rows)
}

/// One `NIBBLE` entry, `0` or terms such as `Y3^Y6^X7`, as (axis, bit)
/// pairs: 0 x, 1 y, 2 z, 3 sample.
fn terms(entry: &str) -> Result<Vec<(usize, u32)>, String> {
    if entry == "0" {
        return Ok(vec![]);
    }
    entry
        .split('^')
        .map(|t| {
            let axis = match t.as_bytes().first() {
                Some(b'X') => 0,
                Some(b'Y') => 1,
                Some(b'Z') => 2,
                Some(b'S') => 3,
                _ => return Err(format!("odd term {t}")),
            };
            let bit = t[1..].parse().map_err(|_| format!("odd term {t}"))?;
            Ok((axis, bit))
        })
        .collect()
}

/// Rust for one address bit.
fn rust(terms: &[(usize, u32)]) -> String {
    if terms.is_empty() {
        return "0".into();
    }
    let names = ["x", "y", "z"];
    terms
        .iter()
        .map(|&(axis, bit)| format!("{}({bit})", names[axis]))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// `patterns.rs` from the header's text.
pub(super) fn patterns(header: &str) -> Result<String, String> {
    let copyright = header
        .lines()
        .map(|l| l.trim_start_matches(['*', ' ']))
        .find(|l| l.starts_with("Copyright"))
        .ok_or("no copyright line")?;
    if !header.contains("SPDX-License-Identifier: MIT") {
        return Err("the header is not under MIT".into());
    }
    let mut nibbles = HashMap::new();
    for (name, width) in [("01", 8), ("2", 4), ("3", 4), ("4", 4)] {
        let rows = table(header, &format!("GFX10_SW_PATTERN_NIBBLE{name}"))?;
        for (k, (fields, comment)) in rows.iter().enumerate() {
            if fields.len() != width || comment.parse() != Ok(k) {
                return Err(format!("NIBBLE{name} row {k} is odd"));
            }
        }
        nibbles.insert(name, rows);
    }
    let mut out = String::new();
    out.push_str(
        "//! Swizzle patterns of the console GPU (RB+, GFX10.3) for each mode and\n\
         //! element size, and for the pipe and bank XOR modes each pipe\n\
         //! configuration. Generated from Mesa's addrlib\n\
         //! (`src/amd/addrlib/src/gfx10/gfx10SwizzlePattern.h`) by\n\
         //! `texture/generate.rs`: do not edit. addrlib carries this notice:\n\
         //!\n",
    );
    writeln!(out, "//! {copyright}").unwrap();
    out.push_str(
        "//! SPDX-License-Identifier: MIT\n\
         //!\n\
         //! Permission is hereby granted, free of charge, to any person obtaining a\n\
         //! copy of this software and associated documentation files (the\n\
         //! \"Software\"), to deal in the Software without restriction, including\n\
         //! without limitation the rights to use, copy, modify, merge, publish,\n\
         //! distribute, sublicense, and/or sell copies of the Software, and to permit\n\
         //! persons to whom the Software is furnished to do so, subject to the\n\
         //! following conditions: the above copyright notice and this permission\n\
         //! notice shall be included in all copies or substantial portions of the\n\
         //! Software. THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND,\n\
         //! EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF\n\
         //! MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.\n\
         //! IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,\n\
         //! DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR\n\
         //! OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE\n\
         //! USE OR OTHER DEALINGS IN THE SOFTWARE.\n\
         \n\
         use super::{Pattern, Pipes, Swizzle, x, y, z};\n\
         \n\
         pub(super) const PATTERNS: &[Pattern] = &[\n",
    );
    for &(name, swizzle, thick) in BLOCKS {
        let rows = table(header, name)?;
        let block_log2 = swizzle.block_log2().ok_or("a mode without blocks")?;
        let configs: &[(&str, Pipes)] = if swizzle.xor() {
            &CONFIGS
        } else {
            &[("ANY", Pipes::SERIES_X)]
        };
        for &(config, pipes) in configs {
            for element_log2 in 0..5u32 {
                // `GetSwizzlePatternInfo`: the XOR modes' rows start at the
                // configuration's (`HwlInitGlobalParams`, RB+).
                let (index, want) = if swizzle.xor() {
                    let (p, k) = (pipes.pipes_log2, pipes.packers_log2);
                    let base = 5 * (p + if k >= 2 { 2 * k - 2 } else { 0 });
                    let want = format!(
                        "{} pipes ({} PKRs) {} bpe",
                        1 << pipes.pipes_log2,
                        1 << pipes.packers_log2,
                        1 << element_log2
                    );
                    (base + element_log2, want)
                } else {
                    (element_log2, format!(" {} bpe", 1 << element_log2))
                };
                let (fields, comment) = rows
                    .get(index as usize)
                    .ok_or_else(|| format!("{name}: no row {index}"))?;
                if !comment.contains(&want) || fields.len() != 5 {
                    return Err(format!("{name} row {index} is {comment}, not {want}"));
                }
                let mut bits = vec![];
                for (field, part) in fields[1..].iter().zip(["01", "2", "3", "4"]) {
                    let k: usize = field.parse().map_err(|_| format!("{name}: {field}"))?;
                    let (row, _) = nibbles[part]
                        .get(k)
                        .ok_or_else(|| format!("{name}: no NIBBLE{part} row {k}"))?;
                    for entry in row {
                        bits.push(terms(entry)?);
                    }
                }
                if bits[block_log2 as usize..].iter().any(|b| !b.is_empty())
                    || bits.iter().flatten().any(|&(axis, _)| axis == 3)
                {
                    return Err(format!("{name} row {index}: bits past the block"));
                }
                let bits: Vec<String> = bits[..block_log2 as usize]
                    .iter()
                    .map(|b| rust(b))
                    .collect();
                let pipes = if swizzle.xor() {
                    format!("Some(Pipes::{config})")
                } else {
                    "None".into()
                };
                writeln!(
                    out,
                    "    // {name}[{index}]: {comment}\n    \
                     Pattern {{ swizzle: Swizzle({}), thick: {thick}, pipes: {pipes}, element_log2: {element_log2}, bits: &[\n        \
                     {}]\n    }},",
                    swizzle.0,
                    bits.join(", "),
                )
                .unwrap();
            }
        }
    }
    out.push_str("];\n");
    Ok(out)
}

/// The checked-in file is what the generator makes of Mesa's header
/// (`VICTORY_ADDRLIB`: the header, or an addrlib directory holding it).
#[test]
#[ignore = "needs Mesa's addrlib sources: VICTORY_ADDRLIB"]
fn patterns_are_generated_from_addrlib() {
    let path = std::path::PathBuf::from(std::env::var_os("VICTORY_ADDRLIB").expect("VICTORY_ADDRLIB"));
    let path = if path.is_dir() {
        path.join("src/gfx10/gfx10SwizzlePattern.h")
    } else {
        path
    };
    let header = std::fs::read_to_string(&path).unwrap();
    let made = patterns(&header).unwrap();
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/texture/patterns.rs");
    if std::env::var_os("VICTORY_WRITE_PATTERNS").is_some() {
        std::fs::write(&file, &made).unwrap();
    }
    let checked_in = std::fs::read_to_string(&file)
        .unwrap()
        .replace("\r\n", "\n");
    assert!(
        checked_in == made,
        "{} differs from what {} generates",
        file.display(),
        path.display()
    );
}
