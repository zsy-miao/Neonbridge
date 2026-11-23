//! Version 2 Xbox serialized ray state objects with three traversal programs.

use crate::Dxbc;

pub struct State<'a> {
    pub traversal: [&'a [u8]; 3],
    pub program: &'a [u8],
    pub root: &'a [u8],
    pub symbols: &'a [u8],
    pub name: String,
    pub kind: u32,
    pub hit_group: Option<String>,
    pub any_hit: Option<String>,
}

fn word(bytes: &mut &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(*bytes.split_off(..4)?.first_chunk()?))
}

fn record<'a>(bytes: &mut &'a [u8]) -> Option<&'a [u8]> {
    let size = word(bytes)? as usize;
    bytes.split_off(..size)
}

fn name(bytes: &mut &[u8]) -> Option<String> {
    let data = record(bytes)?;
    if data.len() > 8192 || data.len() % 2 != 0 {
        return None;
    }
    char::decode_utf16(
        data.as_chunks::<2>()
            .0
            .iter()
            .copied()
            .map(u16::from_le_bytes),
    )
    .map(|ch| ch.ok().filter(|ch| *ch != '\0'))
    .collect()
}

struct Exports {
    primary: String,
    kind: u32,
    group: Option<String>,
    any_hit: Option<String>,
}

fn exports(metadata: &[u8]) -> Option<Exports> {
    let mut bytes = metadata.get(20..)?;
    let count = word(&mut bytes)? as usize;
    if count > bytes.len() / 35 {
        return None;
    }
    let mut functions = Vec::new();
    let mut hit_group = None;
    let mut traversal = false;
    for _ in 0..count {
        let export = name(&mut bytes)?;
        let header = bytes.split_off(..15)?;
        let kind = u32::from_le_bytes(*header.get(8..)?.first_chunk()?);
        if export.is_empty() || name(&mut bytes)? != export {
            return None;
        }
        let closest = name(&mut bytes)?;
        let any = name(&mut bytes)?;
        let intersection = name(&mut bytes)?;
        match (header[12], header[13]) {
            (1, 0) if hit_group.is_none() && !closest.is_empty() && intersection.is_empty() => {
                hit_group = Some((export, closest, (!any.is_empty()).then_some(any)));
            }
            (0, 1)
                if !traversal
                    && export == "XDXR_TraceRay"
                    && closest.is_empty()
                    && any.is_empty()
                    && intersection.is_empty() =>
            {
                traversal = true
            }
            (0, 0) if closest.is_empty() && any.is_empty() && intersection.is_empty() => {
                functions.push((export, kind));
            }
            _ => return None,
        }
    }
    if !traversal || bytes.iter().any(|&b| b != 0) {
        return None;
    }
    if let Some((group, primary, any_hit)) = hit_group {
        let defined = |name: &str, stage| functions.iter().any(|(f, k)| f == name && *k == stage);
        if functions.len() != 1 + usize::from(any_hit.is_some())
            || !defined(&primary, 2)
            || any_hit.as_ref().is_some_and(|f| !defined(f, 1))
        {
            return None;
        }
        Some(Exports {
            primary,
            kind: 2,
            group: Some(group),
            any_hit,
        })
    } else if functions.len() == 1 {
        let (primary, kind) = functions.pop()?;
        Some(Exports {
            primary,
            kind,
            group: None,
            any_hit: None,
        })
    } else {
        None
    }
}

impl<'a> State<'a> {
    /// Parses a raw state object, after the UE archive wrapper has been removed.
    pub fn parse(mut bytes: &'a [u8]) -> Option<Self> {
        if word(&mut bytes)? != 2 {
            return None;
        }
        bytes.split_off(..16)?;
        if word(&mut bytes)? != 4 {
            return None;
        }
        bytes.split_off(..8)?;
        let mut traversal = [&[][..]; 3];
        for index in 0..4 {
            let root_pos = bytes.windows(4).position(|tag| tag == b"DXBC")?;
            let meta = bytes.split_off(..root_pos.checked_sub(4)?)?;
            let root = record(&mut bytes)?;
            if root.get(28..32)? != 1_u32.to_le_bytes() {
                return None;
            }
            let dxbc = Dxbc::parse(root).ok()?;
            if dxbc.total_size as usize != root.len() || dxbc.chunk(b"RTS0").is_none() {
                return None;
            }
            if record(&mut bytes)?.len() != 28 || record(&mut bytes)?.len() != 20 {
                return None;
            }
            let program = record(&mut bytes)?;
            if program.is_empty() || program.len() % 4 != 0 {
                return None;
            }
            let symbols = record(&mut bytes)?;
            if index < 3 {
                let mut export = meta.get(28..)?;
                if name(&mut export)? != "XDXR_TraceRay" {
                    return None;
                }
                traversal[index] = program;
                continue;
            }
            if !bytes.is_empty() {
                return None;
            }
            let exports = exports(meta)?;
            return Some(Self {
                traversal,
                program,
                root,
                symbols,
                name: exports.primary,
                kind: exports.kind,
                hit_group: exports.group,
                any_hit: exports.any_hit,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
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
}
