//! Count call sites per import of a PE executable (offline; idea from leonida
//! aa7223ec, re-derived; ported from import_call_sites.py). For each IAT slot
//! of the delay-load imports (and, with --all, the ordinary imports) it counts
//! the RIP-relative references in executable sections: call [rip+x] (FF 15),
//! jmp [rip+x] (FF 25) and mov reg,[rip+x] (48/4C 8B modrm 05..3D), so the
//! imports the title really uses can be ranked against our stubs.
//!
//! usage: cargo run --example import_call_sites -- GAME.exe [--all] [--top N] [--csv OUT]
use std::collections::HashMap;
use std::process::ExitCode;

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

struct Section {
    va: u32,
    size: u32,
    raw: u32,
    rsize: u32,
    flags: u32,
}

struct Pe {
    data: Vec<u8>,
    base: u64,
    dirs: usize,
    sections: Vec<Section>,
}

impl Pe {
    fn new(data: Vec<u8>) -> Result<Self, String> {
        let nt = u32_at(&data, 0x3c) as usize;
        if data.get(nt..nt + 4) != Some(b"PE\0\0") {
            return Err("not a PE file".into());
        }
        let fh = nt + 4;
        let count = u16_at(&data, fh + 2) as usize;
        let opt = fh + 20;
        if u16_at(&data, opt) != 0x20b {
            return Err("not a PE32+ image".into());
        }
        let base = u64_at(&data, opt + 24);
        let at = opt + u16_at(&data, fh + 16) as usize;
        let sections = (0..count).map(|i| {
            let s = at + 40 * i;
            let (vsize, va, rsize, raw, flags) = (u32_at(&data, s + 8), u32_at(&data, s + 12), u32_at(&data, s + 16), u32_at(&data, s + 20), u32_at(&data, s + 36));
            Section { va, size: vsize.max(rsize), raw, rsize, flags }
        }).collect();
        Ok(Pe { data, base, dirs: opt + 112, sections })
    }

    fn directory(&self, index: usize) -> (u32, u32) {
        (u32_at(&self.data, self.dirs + 8 * index), u32_at(&self.data, self.dirs + 8 * index + 4))
    }

    fn off(&self, rva: u32) -> Option<usize> {
        self.sections.iter().find(|s| s.va <= rva && rva < s.va + s.size && rva - s.va < s.rsize)
            .map(|s| (s.raw + rva - s.va) as usize)
    }

    fn cstr(&self, rva: u32) -> String {
        let Some(o) = self.off(rva) else { return "?".into() };
        let end = self.data[o..].iter().position(|&b| b == 0).map_or(self.data.len(), |n| o + n);
        String::from_utf8_lossy(&self.data[o..end]).into_owned()
    }

    /// (iat slot rva, import name) for one DLL's name/IAT arrays.
    fn thunks(&self, names_rva: u32, iat_rva: u32) -> Vec<(u32, String)> {
        let mut out = Vec::new();
        for i in 0.. {
            let Some(o) = self.off(names_rva + 8 * i) else { break };
            let entry = u64_at(&self.data, o);
            if entry == 0 {
                break;
            }
            let name = if entry >> 63 != 0 { format!("#{}", entry & 0xffff) } else { self.cstr((entry & 0x7fff_ffff) as u32 + 2) };
            out.push((iat_rva + 8 * i, name));
        }
        out
    }

    fn delay_imports(&self) -> Vec<(u32, String, &'static str)> {
        let mut out = Vec::new();
        let (rva, _) = self.directory(13);
        let mut o = if rva != 0 { self.off(rva) } else { None };
        while let Some(at) = o {
            let [attrs, mut dll, _, mut iat, mut names] = [0, 4, 8, 12, 16].map(|k| u32_at(&self.data, at + k));
            if dll == 0 {
                break;
            }
            if attrs & 1 == 0 {
                // old VA-based descriptors
                [dll, iat, names] = [dll, iat, names].map(|x| (u64::from(x).wrapping_sub(self.base)) as u32);
            }
            let lib = self.cstr(dll);
            out.extend(self.thunks(names, iat).into_iter().map(|(slot, name)| (slot, format!("{lib}!{name}"), "delay")));
            o = Some(at + 32);
        }
        out
    }

    fn imports(&self) -> Vec<(u32, String, &'static str)> {
        let mut out = Vec::new();
        let (rva, _) = self.directory(1);
        let mut o = if rva != 0 { self.off(rva) } else { None };
        while let Some(at) = o {
            let [names, _, _, dll, iat] = [0, 4, 8, 12, 16].map(|k| u32_at(&self.data, at + k));
            if dll == 0 {
                break;
            }
            let lib = self.cstr(dll);
            out.extend(self.thunks(if names != 0 { names } else { iat }, iat).into_iter().map(|(slot, name)| (slot, format!("{lib}!{name}"), "import")));
            o = Some(at + 20);
        }
        out
    }
}

/// RIP-relative references to each slot rva in executable sections.
fn references(pe: &Pe, slots: &HashMap<u32, (String, &str)>) -> HashMap<u32, u32> {
    let mut counts = HashMap::new();
    for s in pe.sections.iter().filter(|s| s.flags & 0x2000_0000 != 0) {
        let code = &pe.data[s.raw as usize..(s.raw + s.rsize) as usize];
        let disp = |at: usize| i32::from_le_bytes(code[at..at + 4].try_into().unwrap());
        for i in 0..code.len().saturating_sub(1) {
            // call/jmp [rip+disp32]
            if code[i] == 0xff && matches!(code[i + 1], 0x15 | 0x25) && i + 6 <= code.len() {
                let target = (s.va as i64 + i as i64 + 6 + i64::from(disp(i + 2))) as u32;
                if slots.contains_key(&target) {
                    *counts.entry(target).or_insert(0) += 1;
                }
            }
            // mov r64, [rip+disp32]: REX.W (48/4C) 8B modrm with mod=00 rm=101.
            if matches!(code[i], 0x48 | 0x4c) && code[i + 1] == 0x8b && i + 7 <= code.len() && code[i + 2] & 0xc7 == 0x05 {
                let target = (s.va as i64 + i as i64 + 7 + i64::from(disp(i + 3))) as u32;
                if slots.contains_key(&target) {
                    *counts.entry(target).or_insert(0) += 1;
                }
            }
        }
    }
    counts
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help") {
        println!("usage: import_call_sites GAME.exe [--all] [--top N] [--csv OUT]");
        return Ok(());
    }
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let everything = args.iter().any(|a| a == "--all");
    let top: usize = value("--top").map(|v| v.parse().map_err(|_| "bad --top")).transpose()?.unwrap_or(0);
    let pe = Pe::new(std::fs::read(&args[0]).map_err(|e| format!("{}: {e}", args[0]))?)?;
    let mut entries = pe.delay_imports();
    if everything {
        entries.extend(pe.imports());
    }
    let slots: HashMap<u32, (String, &str)> = entries.into_iter().map(|(s, n, k)| (s, (n, k))).collect();
    let counts = references(&pe, &slots);
    let mut rows: Vec<(u32, &str, &str)> = slots.iter().map(|(s, (n, k))| (counts.get(s).copied().unwrap_or(0), n.as_str(), *k)).collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    if top != 0 {
        rows.truncate(top);
    }
    let used = rows.iter().filter(|r| r.0 != 0).count();
    println!("{} import slots ({}), {used} referenced", slots.len(), if everything { "delay + ordinary" } else { "delay-load only" });
    for (count, name, kind) in &rows {
        println!("{count:6} {kind:6} {name}");
    }
    if let Some(csv) = value("--csv") {
        let mut text = String::from("calls,kind,import\n");
        for (count, name, kind) in &rows {
            text += &format!("{count},{kind},{name}\n");
        }
        std::fs::write(&csv, text).map_err(|e| format!("{csv}: {e}"))?;
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("import_call_sites: {e}");
            ExitCode::FAILURE
        }
    }
}
