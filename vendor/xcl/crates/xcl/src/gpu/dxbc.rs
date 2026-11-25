//! DXBC input/output signatures (ISG1/OSG1/PSG1 and the older ISGN/OSGN/PCSG): the
//! semantics that link console VS outputs to PS inputs on PC.

use super::shader::container_parts;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigElement {
    pub name: String,
    pub index: u32,
    /// D3D_NAME system value (0 = none, 1 = SV_Position, …; 64 = SV_Target …)
    pub system_value: u32,
    /// 1 uint, 2 sint, 3 float …
    pub component_type: u32,
    pub register: u32,
    pub mask: u8,
    pub rw_mask: u8,
    pub stream: u32,
}

impl SigElement {
    pub fn semantic(&self) -> String {
        format!("{}{}", self.name, self.index)
    }
}

fn rd32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
}

fn cstr(b: &[u8], o: usize) -> String {
    let s = b.get(o..).unwrap_or(&[]);
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf8_lossy(&s[..n]).into_owned()
}

/// Parse one signature part (`ISG1`, `OSG1`, `ISGN`, …) of a DXBC container.
pub fn signature(blob: &[u8], fourcc: &str) -> Option<Vec<SigElement>> {
    let parts = container_parts(blob)?;
    let p = parts.iter().find(|p| p.fourcc == fourcc)?;
    let d = &blob[p.offset..p.offset + p.size];
    let count = rd32(d, 0)? as usize;
    let first = rd32(d, 4)? as usize;
    let v1 = fourcc.ends_with('1');
    let size = if v1 { 32 } else { 24 };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let mut e = first + i * size;
        let stream = if v1 {
            let s = rd32(d, e)?;
            e += 4;
            s
        } else {
            0
        };
        out.push(SigElement {
            name: cstr(d, rd32(d, e)? as usize),
            index: rd32(d, e + 4)?,
            system_value: rd32(d, e + 8)?,
            component_type: rd32(d, e + 12)?,
            register: rd32(d, e + 16)?,
            mask: *d.get(e + 20)?,
            rw_mask: *d.get(e + 21)?,
            stream,
        });
    }
    Some(out)
}

/// Input and output signature of a shader (whichever version the container has).
pub fn signatures(blob: &[u8]) -> (Vec<SigElement>, Vec<SigElement>) {
    let i = signature(blob, "ISG1").or_else(|| signature(blob, "ISGN")).unwrap_or_default();
    let o = signature(blob, "OSG1").or_else(|| signature(blob, "OSGN")).unwrap_or_default();
    (i, o)
}

/// Facts from the `PSV0` part (PSVRuntimeInfo0..2): shader kind, mesh-shader limits, thread group.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Psv {
    /// PSVShaderKind (v1+): 0 PS, 1 VS, 2 GS, 3 HS, 4 DS, 5 CS, 13 MS, 14 AS
    pub kind: Option<u8>,
    pub ms_groupshared: u32,
    pub ms_payload: u32,
    pub ms_max_vertices: u16,
    pub ms_max_primitives: u16,
    /// 1 line, 2 triangle
    pub ms_topology: u8,
    pub min_wave: u32,
    pub max_wave: u32,
    /// GS: input primitive (3 = triangle), output topology (5 = triangle strip), max vertices
    pub gs_input_primitive: u32,
    pub gs_output_topology: u32,
    pub gs_max_vertex_count: u16,
    /// v2+
    pub numthreads: Option<[u32; 3]>,
}

pub fn psv(blob: &[u8]) -> Option<Psv> {
    let parts = container_parts(blob)?;
    let p = parts.iter().find(|p| p.fourcc == "PSV0")?;
    let d = &blob[p.offset..p.offset + p.size];
    let size = rd32(d, 0)? as usize;
    let r = d.get(4..4 + size)?;
    let u16at = |o: usize| r.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let mut v = Psv { min_wave: rd32(r, 16)?, max_wave: rd32(r, 20)?, ..Default::default() };
    if size >= 36 {
        v.kind = Some(r[24]);
    }
    if v.kind == Some(13) {
        v.ms_groupshared = rd32(r, 0)?;
        v.ms_payload = rd32(r, 8)?;
        v.ms_max_vertices = u16at(12)?;
        v.ms_max_primitives = u16at(14)?;
        v.ms_topology = r[27];
    }
    if v.kind == Some(2) {
        v.gs_input_primitive = rd32(r, 0)?;
        v.gs_output_topology = rd32(r, 4)?;
        v.gs_max_vertex_count = u16at(26)?;
    }
    if size >= 48 {
        v.numthreads = Some([rd32(r, 36)?, rd32(r, 40)?, rd32(r, 44)?]);
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ISG1 with POSITION (xyz) at register 0 and SV_VertexID at register 1.
    fn isg1() -> Vec<u8> {
        let mut d = Vec::new();
        let names = b"POSITION\0SV_VertexID\0";
        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(&8u32.to_le_bytes());
        let base = 8 + 2 * 32;
        for (name_off, sv, ct, reg, mask) in [(0u32, 0u32, 3u32, 0u32, 7u8), (9, 6, 1, 1, 1)].iter() {
            d.extend_from_slice(&0u32.to_le_bytes()); // stream
            d.extend_from_slice(&(base + name_off).to_le_bytes());
            d.extend_from_slice(&0u32.to_le_bytes()); // semantic index
            d.extend_from_slice(&sv.to_le_bytes());
            d.extend_from_slice(&ct.to_le_bytes());
            d.extend_from_slice(&reg.to_le_bytes());
            d.extend_from_slice(&[*mask, *mask, 0, 0]);
            d.extend_from_slice(&0u32.to_le_bytes()); // min precision
        }
        d.extend_from_slice(names);
        crate::gpu::xbox::tests::container(&[(b"ISG1", &d)])
    }

    #[test]
    fn parses_mesh_psv() {
        let mut r = vec![0u8; 48];
        r[0..4].copy_from_slice(&1024u32.to_le_bytes()); // groupshared
        r[12..14].copy_from_slice(&128u16.to_le_bytes());
        r[14..16].copy_from_slice(&126u16.to_le_bytes());
        r[16..20].copy_from_slice(&64u32.to_le_bytes());
        r[20..24].copy_from_slice(&64u32.to_le_bytes());
        r[24] = 13; // mesh
        r[27] = 2; // triangles
        r[36..40].copy_from_slice(&128u32.to_le_bytes());
        r[40..44].copy_from_slice(&1u32.to_le_bytes());
        r[44..48].copy_from_slice(&1u32.to_le_bytes());
        let mut part = 48u32.to_le_bytes().to_vec();
        part.extend_from_slice(&r);
        part.extend_from_slice(&0u32.to_le_bytes()); // resources
        let p = psv(&crate::gpu::xbox::tests::container(&[(b"PSV0", &part)])).unwrap();
        assert_eq!(p.kind, Some(13));
        assert_eq!((p.ms_max_vertices, p.ms_max_primitives, p.ms_topology, p.ms_groupshared), (128, 126, 2, 1024));
        assert_eq!(p.numthreads, Some([128, 1, 1]));
    }

    #[test]
    fn parses_v1_signature() {
        let s = signature(&isg1(), "ISG1").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].semantic().as_str(), s[0].register, s[0].mask, s[0].component_type), ("POSITION0", 0, 7, 3));
        assert_eq!((s[1].name.as_str(), s[1].system_value, s[1].register), ("SV_VertexID", 6, 1));
        assert_eq!(signatures(&isg1()).0, s);
    }
}
