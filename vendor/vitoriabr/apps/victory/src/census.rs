//! Which imports do titles need, and which do we cover?

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Result;
use victory_hle::{HleResolver, HostPolicy};
use victory_loader::Provenance;
use victory_pe::{ImageInfo, ImportSymbol};

#[derive(Default)]
struct DllStats {
    functions: BTreeSet<ImportSymbol>,
    hle: usize,
    host: usize,
    missing: usize,
    images: BTreeSet<PathBuf>,
}

pub fn run(paths: Vec<PathBuf>, strict: bool, list_missing: bool) -> Result<()> {
    let paths = crate::default_paths(paths)?;
    let resolver = HleResolver::new(if strict {
        HostPolicy::none()
    } else {
        HostPolicy::default()
    });

    let mut images = 0usize;
    let mut failed = Vec::new();
    let mut dlls: BTreeMap<String, DllStats> = BTreeMap::new();
    let mut missing: BTreeMap<(String, String), BTreeSet<PathBuf>> = BTreeMap::new();

    for root in &paths {
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if !entry.file_type().is_file() || !looks_like_pe(path) {
                continue;
            }
            let info = match std::fs::read(path)
                .map_err(anyhow::Error::from)
                .and_then(|d| ImageInfo::parse(&d).map_err(anyhow::Error::from))
            {
                Ok(info) => info,
                Err(e) => {
                    failed.push((path.to_path_buf(), e.to_string()));
                    continue;
                }
            };
            images += 1;
            for module in info.imports.iter().chain(&info.delay_imports) {
                let key = module.dll.to_ascii_lowercase();
                let stats = dlls.entry(key.clone()).or_default();
                stats.images.insert(path.to_path_buf());
                for e in &module.entries {
                    let class = resolver.classify(&module.dll, &e.symbol);
                    if class.is_none() {
                        missing
                            .entry((key.clone(), e.symbol.to_string()))
                            .or_default()
                            .insert(path.to_path_buf());
                    }
                    // Count each dll!function once across all images.
                    if stats.functions.insert(e.symbol.clone()) {
                        match class {
                            Some(Provenance::Hle) => stats.hle += 1,
                            Some(Provenance::Host) => stats.host += 1,
                            Some(Provenance::Title) => unreachable!("census maps no title DLLs"),
                            None => stats.missing += 1,
                        }
                    }
                }
            }
        }
    }

    println!("{images} images scanned, {} unreadable", failed.len());
    for (p, e) in &failed {
        println!("  skipped {}: {e}", p.display());
    }
    println!(
        "\n{:<56} {:>6} {:>6} {:>6} {:>8} {:>7}",
        "dll", "funcs", "hle", "host", "missing", "images"
    );
    let (mut f, mut h, mut o, mut m) = (0, 0, 0, 0);
    for (dll, s) in &dlls {
        println!(
            "{dll:<56} {:>6} {:>6} {:>6} {:>8} {:>7}",
            s.functions.len(),
            s.hle,
            s.host,
            s.missing,
            s.images.len()
        );
        f += s.functions.len();
        h += s.hle;
        o += s.host;
        m += s.missing;
    }
    println!("{:<56} {f:>6} {h:>6} {o:>6} {m:>8}", "TOTAL");

    if list_missing {
        let mut rows: Vec<_> = missing.into_iter().collect();
        rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
        println!("\nmissing (by number of images using it)");
        for ((dll, sym), users) in rows {
            println!("  {:>4}  {dll}!{sym}", users.len());
        }
    }
    Ok(())
}

/// Cheap pre-filter: an `MZ` header, regardless of extension (console
/// packages do not always use .exe/.dll).
fn looks_like_pe(path: &Path) -> bool {
    let mut magic = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && &magic == b"MZ"
}
