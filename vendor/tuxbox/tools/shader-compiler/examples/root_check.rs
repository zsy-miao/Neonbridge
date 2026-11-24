fn main() {
    for p in std::env::args().skip(1) {
        let b = std::fs::read(&p).unwrap();
        match tuxbox_dxbc::rootsig::parse_rts0(&b) {
            Ok(r) => println!("ok params={} samplers={} flags={:#x}", r.params.len(), r.static_samplers.len(), r.flags),
            Err(e) => println!("ERR {p}: {e}"),
        }
    }
}
