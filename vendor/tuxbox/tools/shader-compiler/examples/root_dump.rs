//! Print a serialized root signature (a DXBC container with an RTS0 chunk, or a
//! bare RTS0 payload): flags, static samplers, and each parameter's ranges.
//! usage: root_dump <file>...
fn payload(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.get(..4) != Some(b"DXBC") {
        return Some(bytes);
    }
    let count = u32::from_le_bytes(bytes.get(28..32)?.try_into().ok()?) as usize;
    for index in 0..count {
        let at = 32 + 4 * index;
        let offset = u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
        if bytes.get(offset..offset + 4) == Some(b"RTS0") {
            let size = u32::from_le_bytes(bytes.get(offset + 4..offset + 8)?.try_into().ok()?) as usize;
            return bytes.get(offset + 8..offset + 8 + size);
        }
    }
    None
}

fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("read");
        let Some(body) = payload(&bytes) else {
            println!("{path}: no RTS0 chunk");
            continue;
        };
        match tuxbox_dxbc::rootsig::parse_rts0(body) {
            Ok(root) => {
                println!(
                    "{path}: v{} flags={:#x} static_samplers={} params={}",
                    root.version,
                    root.flags,
                    root.static_samplers.len(),
                    root.params.len()
                );
                for (index, param) in root.params.iter().enumerate() {
                    println!("  [{index}] visibility={} {:?}", param.visibility, param.kind);
                }
            }
            Err(error) => println!("{path}: {error}"),
        }
    }
}
