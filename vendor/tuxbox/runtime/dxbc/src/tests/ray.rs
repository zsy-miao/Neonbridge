use super::exports;

fn utf16(data: &mut Vec<u8>, name: &str) {
    let bytes: Vec<_> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    data.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    data.extend(bytes);
}

fn record(bytes: &mut Vec<u8>, name: &str, kind: u32, flag: usize, components: [&str; 3]) {
    utf16(bytes, name);
    let mut header = [0; 15];
    header[8..12].copy_from_slice(&kind.to_le_bytes());
    if flag != 0 {
        header[flag] = 1;
    }
    bytes.extend(header);
    utf16(bytes, name);
    for component in components {
        utf16(bytes, component);
    }
}

#[test]
fn resolves_group_members_across_traversal_and_rejects_missing_imports() {
    let mut bytes = vec![0; 20];
    bytes.extend_from_slice(&4_u32.to_le_bytes());
    record(&mut bytes, "Secondary", 1, 0, [""; 3]);
    record(&mut bytes, "Group", 0, 12, ["Primary", "Secondary", ""]);
    record(&mut bytes, "XDXR_TraceRay", 0, 13, [""; 3]);
    record(&mut bytes, "Primary", 2, 0, [""; 3]);
    let parsed = exports(&bytes).unwrap();
    assert_eq!(parsed.primary, "Primary");
    assert_eq!(parsed.group.as_deref(), Some("Group"));
    assert_eq!(parsed.any_hit.as_deref(), Some("Secondary"));
    assert!(exports(&bytes[..bytes.len() - 1]).is_none());
    bytes[20..24].copy_from_slice(&3_u32.to_le_bytes());
    assert!(exports(&bytes).is_none());
}
