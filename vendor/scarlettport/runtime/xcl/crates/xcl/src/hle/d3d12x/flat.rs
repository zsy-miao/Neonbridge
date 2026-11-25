//! `XCL_FLAT=1`: an unlit / flat rendering mode. Graphics pipelines keep their translated
//! vertex (mesh) shader but the pixel shader is a tiny built-in one that writes a flat grey
//! (shaded by SV_Position depth, so geometry stays readable) to every render target the
//! pipeline declares. It reads only SV_Position, which any VS / MS output signature satisfies,
//! and is built at once (no stand-in). Compute shaders are untouched.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Whether the flat mode is on (`XCL_FLAT=1`).
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_FLAT").is_ok_and(|v| v != "0" && !v.is_empty()))
}

/// Whether a console pixel shader of `len` bytes gets the flat stand-in: only big ones
/// (`XCL_FLAT_MIN_BYTES`, default 16384), so the 2D menus, loading screens and the final
/// composite keep their real (small) pixel shaders.
pub fn wants(len: usize) -> bool {
    static MIN: OnceLock<usize> = OnceLock::new();
    enabled() && len >= *MIN.get_or_init(|| std::env::var("XCL_FLAT_MIN_BYTES").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(16384))
}

fn note_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| crate::xlog_info!("XCL_FLAT: pixel shaders replaced by a flat stand-in"));
}

/// The HLSL type a render target of DXIL format `f` is written as: `None` for UNKNOWN.
fn target_type(f: u32) -> Option<&'static str> {
    Some(match f {
        0 => return None,
        // *_SINT
        4 | 8 | 14 | 18 | 32 | 38 | 43 | 52 | 59 | 64 => "int4",
        // *_UINT
        3 | 7 | 13 | 17 | 25 | 30 | 36 | 42 | 50 | 57 | 62 => "uint4",
        _ => "float4",
    })
}

/// The flat pixel shader for render targets `formats` (DXGI numbers; slot k = SV_Target k).
pub fn flat_ps_hlsl(formats: &[u32]) -> String {
    let mut s = String::from("struct O {\n");
    for (k, f) in formats.iter().enumerate() {
        if let Some(t) = target_type(*f) {
            s.push_str(&format!("    {t} t{k} : SV_Target{k};\n"));
        }
    }
    s.push_str("};\nO main(float4 pos : SV_Position) {\n    O o;\n    float g = 0.15 + 0.85 * sqrt(saturate(pos.z));\n");
    for (k, f) in formats.iter().enumerate() {
        match target_type(*f) {
            Some("float4") => s.push_str(&format!("    o.t{k} = float4(g, g, g, 1.0);\n")),
            Some(t) => s.push_str(&format!("    o.t{k} = {t}(0, 0, 0, 0);\n")),
            None => {}
        }
    }
    s.push_str("    return o;\n}\n");
    s
}

/// The compiled flat pixel shader for these formats, built once per signature.
pub fn flat_ps(formats: &[u32]) -> Result<Arc<Vec<u8>>, String> {
    static CACHE: Mutex<Option<HashMap<String, Arc<Vec<u8>>>>> = Mutex::new(None);
    let hlsl = flat_ps_hlsl(formats);
    if let Some(b) = CACHE.lock().unwrap().as_ref().and_then(|m| m.get(&hlsl)) {
        return Ok(b.clone());
    }
    let dxil = Arc::new(crate::gpu::recomp::dxc::compile(&hlsl, "main", "ps_6_0", &[]).map_err(|e| format!("flat PS: {e}"))?);
    note_once();
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(hlsl, dxil.clone());
    Ok(dxil)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hlsl_matches_formats() {
        // R8G8B8A8_UNORM, R32_UINT, UNKNOWN, R16G16_SINT
        let h = flat_ps_hlsl(&[28, 42, 0, 38]);
        assert!(h.contains("float4 t0 : SV_Target0;"));
        assert!(h.contains("uint4 t1 : SV_Target1;"));
        assert!(!h.contains("SV_Target2"));
        assert!(h.contains("int4 t3 : SV_Target3;"));
        assert!(h.contains("o.t0 = float4(g, g, g, 1.0);"));
        assert!(h.contains("o.t1 = uint4(0, 0, 0, 0);"));
        assert!(h.contains("float4 pos : SV_Position"));
        assert!(!h.contains("TEXCOORD"));
    }

    #[test]
    fn compiles_with_dxc() {
        if crate::gpu::recomp::dxc::location().is_err() {
            crate::gpu::recomp::runner::skip_test("no DXC");
            return;
        }
        let b = flat_ps(&[28, 42, 0, 38, 2]).unwrap();
        assert!(b.starts_with(b"DXBC"));
        assert!(Arc::ptr_eq(&b, &flat_ps(&[28, 42, 0, 38, 2]).unwrap()));
    }

    #[test]
    fn no_targets_is_valid() {
        assert!(flat_ps_hlsl(&[]).contains("O main("));
    }

    #[test]
    fn gpu_flat_ps_draws_with_a_vs_of_other_outputs() {
        if let Err(reason) = crate::gpu::recomp::runner::adapter() {
            println!("GPU unavailable - skipping ({reason})");
            crate::gpu::recomp::runner::skip_test("no GPU");
            return;
        }
        if crate::gpu::recomp::dxc::location().is_err() {
            crate::gpu::recomp::runner::skip_test("no DXC");
            return;
        }
        let vs = "struct O { float4 pos : SV_Position; float4 a : TEXCOORD3; float4 b : COLOR1; };\n\
                  O main(uint id : SV_VertexID) { O o; float2 uv = float2((id << 1) & 2, id & 2);\n\
                  o.pos = float4(uv * float2(2, -2) + float2(-1, 1), 0.5, 1); o.a = 1; o.b = 2; return o; }";
        let vsb = crate::gpu::recomp::dxc::compile(vs, "main", "vs_6_6", &[]).unwrap();
        let ps = flat_ps(&[2]).unwrap();
        let px = crate::gpu::recomp::runner::draw(&vsb, &ps, &[], &[0.0f32; 3], 4, &[], &[], 8, [0.0; 4]).unwrap();
        assert!(px.iter().any(|c| c[3] == 1.0 && c[0] > 0.1 && c[0] == c[1]), "{:?}", &px[..4]);
    }
}
