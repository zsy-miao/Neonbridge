//! Lower verified RDNA2 ray continuations to DXR library exports.
//!
//! The native scheduler uses v8=42 to request traversal and a stack of return
//! PCs to resume each lane. A DXR TraceRay call performs both operations. The
//! fingerprints cover the scheduler, continuations and padding. Only operands
//! decoded separately below are masked. Names are metadata, never match keys.

mod alu;
mod continuation;
mod material;
mod lighting;
mod resources;

use std::fmt::Write;

use crate::rdna2::{instructions, Opnd};
use crate::rootsig::{parse_rts0, RootParamKind, RootSig};
use tuxbox_dxbc::{ray::State, Dxbc};

const PAYLOAD: &str =
    "struct Payload { float t; uint primitive; uint instance; float2 bary; uint id; };\n";
const TRIANGLE_TRAVERSAL: [(usize, u64); 3] = [
    (3432, 0xd5491d5c38456cc0),
    (1896, 0xf5a63f484de0a007),
    (4456, 0x38524260d1f3e550),
];

const ANY_HIT_TRAVERSAL: [(usize, u64); 3] = [
    (3688, 0xad13a8c4d981eb00),
    TRIANGLE_TRAVERSAL[1],
    (4968, 0x5daf1f6dd6b3904e),
];

fn fingerprint(bytes: &[u8], operands: &[std::ops::Range<usize>]) -> u64 {
    bytes
        .iter()
        .enumerate()
        .fold(0xcbf29ce484222325, |h, (i, &byte)| {
            let byte = if operands.iter().any(|r| r.contains(&i)) {
                0
            } else {
                byte
            };
            (h ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
}

// A register-relative move wraps the architectural VGPR number to eight bits.
// Keep the smaller register file on shaders with only direct operands.
fn vgpr_count(state: &State<'_>) -> usize {
    if instructions(state.program).any(|op| matches!(op.mnemonic.as_str(), "v_movrels_b32" | "v_movreld_b32")) {
        256
    } else {
        128
    }
}

fn operand(value: &Opnd) -> Result<String, String> {
    Ok(match *value {
        Opnd::Vgpr(n) if n < 128 => format!("v[{n}]"),
        Opnd::Sgpr(n) if n < 128 => format!("s[{n}]"),
        Opnd::ExecLo => "exec".into(),
        Opnd::Scc => "scc".into(),
        Opnd::VccLo => "vcc".into(),
        Opnd::M0 => "m0".into(),
        Opnd::Null => "0u".into(),
        Opnd::ConstInt(i) => format!("0x{:08x}u", i as u32),
        Opnd::ConstF32(f) => format!("0x{:08x}u", f.to_bits()),
        Opnd::Literal(w) => format!("0x{w:08x}u"),
        _ => return Err(format!("unsupported ray operand {value:?}")),
    })
}

fn immediate(isa: &[u8], pc: usize, dwords: usize) -> Option<u32> {
    let ins = instructions(isa.get(pc..)?).next()?;
    if !ins.is_complete()
        || ins.dwords != dwords
        || !matches!(ins.mnemonic.as_str(), "v_mov_b32_e32" | "v_mov_b32")
    {
        return None;
    }
    match ins.src.first()? {
        Opnd::ConstInt(n) => Some(*n as u32),
        Opnd::ConstF32(f) => Some(f.to_bits()),
        Opnd::Literal(w) => Some(*w),
        _ => None,
    }
}

fn binding(sig: &RootSig, parameter: usize, index: u32, kind: u32) -> Option<String> {
    let RootParamKind::Table(ranges) = &sig.params.get(parameter)?.kind else {
        return None;
    };
    let mut next = 0;
    for range in ranges {
        let first = if range.offset_in_table == u32::MAX {
            next
        } else {
            range.offset_in_table
        };
        next = first.checked_add(range.num)?;
        if (first..next).contains(&index) && range.range_type == kind {
            let reg = range.base_register.checked_add(index - first)?;
            let class = match kind {
                0 => 't',
                1 => 'u',
                2 => 'b',
                3 => 's',
                _ => return None,
            };
            return Some(format!("register({class}{reg}, space{})", range.space));
        }
    }
    None
}

fn raygen(state: &State<'_>) -> Option<String> {
    let isa = state.program;
    // Both load the dispatch X indexed 32-byte ray, preserve a six-dword
    // payload on the lane stack, and branch to the shared traversal code.
    let returns_intersection = match fingerprint(isa, &[0x250..0x254, 0x2bc..0x2bd]) {
        0xf717e7eb9e00c05b => false,
        0xfb49d7d417fda1fb => true,
        _ => return None,
    };
    let flags = immediate(isa, 0x2bc, 1)?;
    if flags > 0xff || flags & 3 == 3 {
        return None;
    }
    let packed = immediate(isa, 0x24c, 2)?;
    // Traversal compares the low byte with the instance mask. Bits 8..12
    // add to the instance contribution, bits 12..16 scale the geometry
    // index, and the high half selects a record from the miss table.
    let mask = packed & 255;
    let contribution = (packed >> 8) & 15;
    let multiplier = (packed >> 12) & 15;
    let miss = packed >> 16;

    let dxbc = Dxbc::parse(state.root).ok()?;
    let part = dxbc.chunk(b"RTS0")?;
    let sig = parse_rts0(state.root.get(part.offset..part.offset + part.size)?).ok()?;
    // Loads at s[6:7]+0/+12 select tables 0/3. A descriptor occupies 32 bytes.
    let scene = binding(&sig, 0, 0, 0)?;
    let rays = binding(&sig, 0, 1, 0)?;
    let output = binding(&sig, 3, u32::from(returns_intersection), 1)?;
    let (output_type, store) = if returns_intersection {
        ("Intersection", "Intersection hit = { p.t, p.primitive, p.instance, p.bary };\n    result[index] = hit;")
    } else {
        // v_cmp_ngt_f32 0, hitT is true on unordered input too.
        ("uint", "result[index] = !(0.0 > p.t) ? 0xffffffffu : 0u;")
    };
    Some(format!(
        r#"{PAYLOAD}struct InputRay {{ float3 origin; uint unused; float3 direction; float tmax; }};
struct Intersection {{ float t; uint primitive; uint instance; float2 bary; }};
RaytracingAccelerationStructure scene : {scene};
StructuredBuffer<InputRay> rays : {rays};
RWStructuredBuffer<{output_type}> result : {output};

[shader("raygeneration")]
void {name}() {{
    uint index = DispatchRaysIndex().x;
    InputRay input = rays[index];
    RayDesc ray = {{ input.origin, 0.0, input.direction, input.tmax }};
    Payload p = (Payload)0;
    TraceRay(scene, {flags}, {mask}, {contribution}, {multiplier}, {miss}, ray, p);
    {store}
}}
"#,
        name = state.name
    ))
}

fn identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn closest_hit(state: &State<'_>) -> Option<String> {
    let native = (state.program.len(), fingerprint(state.program, &[]));
    if native == (1640, 0x3552e36b712b52a3) {
        let any = state.any_hit.as_deref().filter(|name| identifier(name))?;
        // The candidate callback selects traversal result 1 in v28[15:8].
        // The traversal resumes its search. The closest-hit export only pops
        // the return continuation and leaves the incoming payload intact.
        return Some(format!(
            "struct Payload {{ uint word; }};\n[shader(\"closesthit\")]\nvoid {}(inout Payload p, BuiltInTriangleIntersectionAttributes a) {{}}\n[shader(\"anyhit\")]\nvoid {any}(inout Payload p, BuiltInTriangleIntersectionAttributes a) {{ IgnoreHit(); }}\n",
            state.name
        ));
    }
    if state.any_hit.is_some() != (native == (1896, 0x7d4db16d0c45fcf4)) {
        return None;
    }
    // Native hit continuations use v6 for hit T and v14/v15 for triangle
    // barycentrics. When requested, the node at v2 supplies the instance
    // index and its 24-bit ID, and the leaf at v3 supplies the primitive.
    let (fields, stores) = match native {
        (1640, 0x8e794ecbeef50098) | (1896, 0x7d4db16d0c45fcf4) => (
            "float t; uint primitive; uint instance; float2 bary; uint id;",
            "p.t = RayTCurrent(); p.primitive = PrimitiveIndex(); p.instance = InstanceIndex(); p.bary = a.barycentrics; p.id = InstanceID();"
        ),
        (1640, 0xa82fc0e37d6049df) => (
            "float t; uint primitive; uint instance; float2 bary;",
            "p.t = RayTCurrent(); p.primitive = PrimitiveIndex(); p.instance = InstanceIndex(); p.bary = a.barycentrics;"
        ),
        (1384, 0x0c1a50f836e3f357) => (
            "float t; uint2 preserved; float2 bary;",
            "p.t = RayTCurrent(); p.bary = a.barycentrics;"
        ),
        (1384, 0x49f4d0e87549927c) => ("float t;", "p.t = RayTCurrent();"),
        _ => return None,
    };
    let mut hlsl = format!("struct Payload {{ {fields} }};\n[shader(\"closesthit\")]\nvoid {}(inout Payload p, BuiltInTriangleIntersectionAttributes a) {{ {stores} }}\n", state.name);
    if native == (1896, 0x7d4db16d0c45fcf4) {
        let export = state.any_hit.as_deref().filter(|name| identifier(name))?;
        // This native any-hit export only reloads its return address from
        // the lane stack. Returning normally accepts the current candidate.
        writeln!(hlsl, "[shader(\"anyhit\")]\nvoid {export}(inout Payload p, BuiltInTriangleIntersectionAttributes a) {{}}").ok()?;
    }
    Some(hlsl)
}

/// Translate a raw serialized v2 library. Unrecognized state layouts, traversal
/// conventions or title continuations fail before producing a host library.
pub fn library(bytes: &[u8]) -> Result<(String, String), String> {
    let state = State::parse(bytes).ok_or("invalid or unsupported serialized Xbox ray state")?;
    let expected = if state.any_hit.is_some() {
        ANY_HIT_TRAVERSAL
    } else {
        TRIANGLE_TRAVERSAL
    };
    if !state
        .traversal
        .iter()
        .zip(expected)
        .all(|(program, (size, id))| program.len() == size && fingerprint(program, &[]) == id)
    {
        return Err(format!("unsupported traversal program for {}", state.name));
    }
    if !identifier(&state.name) {
        return Err("ray export cannot be represented as an HLSL identifier".into());
    }
    if state.kind == 0 {
        let family = state.name.rsplit_once('_').map(|(family, _)| family);
        let payload = match family {
            Some("OcclusionRGS") => Some(continuation::Payload::Fixed(16)),
            // Screen-probe gathers reserve five adjacent hit words at frame word
            // 7, 13, 18, 25 or 30. Some perform only the output pass.
            Some("LumenScreenProbeGatherHardwareRayTracingRGS") => Some(continuation::Payload::Fixed(5)),
            // These Lumen passes initialize five hit words. Direct lighting
            // starts at frame word 1, radiosity and volume at 7, and the
            // radiance cache at 7 or 13. The next word is live across the call.
            Some("LumenTranslucencyVolumeHardwareRayTracingRGS"
                | "LumenRadianceCacheHardwareRayTracingRGS"
                | "LumenSceneDirectLightingHardwareRayTracingRGS"
                | "LumenRadiosityHardwareRayTracingRGS") => Some(continuation::Payload::Fixed(5)),
            Some("LumenReflectionHardwareRayTracingRGS") if matches!(state.name.as_str(),
                "LumenReflectionHardwareRayTracingRGS_af06c2ee" |
                "LumenReflectionHardwareRayTracingRGS_8d5b0e96" |
                "LumenReflectionHardwareRayTracingRGS_78010894") => Some(continuation::Payload::Fixed(5)),
            Some("LumenReflectionHardwareRayTracingRGS") if matches!(state.name.as_str(),
                "LumenReflectionHardwareRayTracingRGS_7f9f89a2" |
                "LumenReflectionHardwareRayTracingRGS_74627350") => Some(continuation::Payload::Reflection),
            _ => None,
        };
        if let Some(payload) = payload {
            let hlsl = continuation::ray_generation(&state, payload)?;
            return Ok((state.name, hlsl));
        }
    }
    if lighting::matches(&state) {
        let hlsl = lighting::translated(&state)?;
        return Ok((state.name, hlsl));
    }
    if let Some(words) = material::payload_words(&state) {
        let hlsl = material::translated(&state, words)?;
        return Ok((state.name, hlsl));
    }
    let shader = match (state.kind, state.program.len()) {
        (0, 1896) => raygen(&state),
        (4, 1384) if fingerprint(state.program, &[0x128..0x129]) == 0x746c9acd16ec4a58 =>
            immediate(state.program, 0x128, 1).map(|value| format!(
                "struct Payload {{ float t; }};\n[shader(\"miss\")]\nvoid {}(inout Payload p) {{ p.t = asfloat(0x{value:08x}u); }}\n", state.name)),
        (2, _) => closest_hit(&state),
        _ => None,
    }.ok_or_else(|| format!("unsupported native ray continuation for {} (kind {}, {} bytes)", state.name, state.kind, state.program.len()))?;
    Ok((state.name, shader))
}

#[cfg(test)]
mod tests {
    use super::immediate;

    #[test]
    fn preserves_immediate_width_at_masked_operands() {
        let mut code = 0x7e22028d_u32.to_le_bytes().to_vec();
        code.extend(0x00002001_u32.to_le_bytes());
        assert_eq!(immediate(&code, 0, 1), Some(13));
        code[0] = 0xff;
        assert_eq!(immediate(&code, 0, 1), None);
        assert_eq!(immediate(&code, 0, 2), Some(0x2001));
        assert_eq!(immediate(&code[..4], 0, 2), None);
    }
}
