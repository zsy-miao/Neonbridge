//! anyxboxx_rdna2 : décode des instructions RDNA 2 avec le désassembleur de TomBox.
//!
//! Entrée (stdin) : une instruction par ligne, en octets hexadécimaux (« 01 02 7f 80 » ou
//! « 0102 7f80 »). Sortie (stdout) : une ligne par instruction d'entrée,
//! « <mots de 32 bits> <encodage> <mnémonique> », pour la première instruction décodée.
//! Ligne vide ou illisible : « 0 erreur - ».
use std::io::{self, BufRead, BufWriter, Write};

use tombox_xshader::rdna2::disassemble;

fn parse_hex(line: &str) -> Option<Vec<u8>> {
    let digits: String = line.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if digits.is_empty() || digits.len() % 2 != 0 {
        return None;
    }
    (0..digits.len()).step_by(2).map(|i| u8::from_str_radix(&digits[i..i + 2], 16).ok()).collect()
}

fn describe(bytes: &[u8]) -> String {
    let mut padded = bytes.to_vec();
    padded.resize(bytes.len().div_ceil(4) * 4, 0);
    match disassemble(&padded).first() {
        Some(insn) => format!("{} {:?} {}", insn.dwords, insn.enc, insn.mnemonic),
        None => "0 erreur -".to_string(),
    }
}

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut out = BufWriter::new(io::stdout().lock());
    for line in stdin.lock().lines() {
        let line = line?;
        let text = parse_hex(&line).map_or_else(|| "0 erreur -".to_string(), |bytes| describe(&bytes));
        writeln!(out, "{text}")?;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_known_encodings() {
        assert_eq!(describe(&[0x00, 0x00, 0x81, 0xbf]), "1 Sopp s_endpgm");
        assert!(describe(&[0x01, 0x02, 0x7f, 0x80]).starts_with("1 Sop2 s_add_u32"));
    }

    #[test]
    fn rejects_bad_hex() {
        assert_eq!(parse_hex("zz"), None);
        assert_eq!(parse_hex("0102 7f80"), Some(vec![1, 2, 0x7f, 0x80]));
    }
}
