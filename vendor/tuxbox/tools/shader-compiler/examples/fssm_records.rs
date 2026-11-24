//! Print FSSM used-resource and user-data records for Xbox shader containers.
//! usage: cargo run --example fssm_records -- <shader.dxbc>...
fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("read shader");
        let ex = match tuxbox_xshader::extract(&bytes) {
            Ok(ex) => ex,
            Err(e) => { println!("{path}: {e}"); continue; }
        };
        println!("{path} stage={:?} entry={:?}", ex.stage, ex.entry);
        for (label, rows) in [
            ("used", tuxbox_dxbc::compute_table::records(&ex.fssm).map(|r| r.collect::<Vec<_>>())),
            ("user", tuxbox_dxbc::compute_table::user_data_records(&ex.fssm).map(|r| r.collect::<Vec<_>>())),
        ] {
            for [class, pos, reg, shape, loc] in rows.unwrap_or_default() {
                println!("  {label} class={class:#x} pos={pos:#x} reg={reg} shape={shape:#x} loc={loc:#x}");
            }
        }
    }
}
