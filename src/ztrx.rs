//! TB_ZTRX_SHADER (default on, =0 only our translator): a plain shader request
//! goes to the leaders' translator first (vendor/ztrx/victory-shader, copied
//! unmodified from VItoriaBr a93b052, the ztrx line, GPL-3.0-or-later, credit to
//! its authors). It writes SM 6.6 DXIL itself from structured control flow
//! (per-lane booleans, proven EXEC guards, its own fold/reuse/dead passes), so
//! no HLSL, no dxc and none of our flag dispatch that made the 4-4.5 MiB -O0
//! programs. What it refuses goes on to our HLSL + dxc path.
//!
//! Its binding ABI (victory-shader emit/mod.rs, module notes) is not ours: the
//! runtime binds such a program by `<key>.ztrx` beside `<key>.cso`, in the format
//! d3d12's ztrx_root::parse_sidecar reads (UTF-8, one item per line, fields
//! split by whitespace, unknown lines skipped), straight from HostShader:
//!   ztrx 1
//!   user <index> <slot> <root> <dword>      root: c Constants, d Descriptor, t Table,
//!                                          i ImmediateSampler, r ResourceHeap, s SamplerHeap, o<kind> Other
//!   sampler <filter> <u> <v> <w> <mip_lod_bias> <max_anisotropy> <comparison> <border> <min_lod> <max_lod>
//!                                          one per static sampler, in s<n> order
//!   cbv <slot> / srv <slot> / uav <slot>
//!   view <table_slot> <constant_slot> <constant_dword> <byte_offset> <shift>

pub fn on() -> bool {
    std::env::var("TB_ZTRX_SHADER").map_or(true, |v| v.trim() != "0")
}

/// The DXIL container and the sidecar text, or the translator's refusal.
pub fn translate(container: &[u8]) -> Result<(Vec<u8>, String), String> {
    use victory_shader::RootType;
    let h = std::panic::catch_unwind(|| victory_shader::emit::translate(container))
        .map_err(|_| "panic in victory-shader".to_string())?
        .map_err(|e| e.to_string())?;
    // NEONBRIDGE_ZTRX_WAVE64_REFUSE (default on, =0 off): on a host whose waves
    // are at most 32 lanes, a program ztrx still leaves at WaveSize(64) (a
    // console wave64 compute program whose code crosses 32-lane halves, which
    // victory-shader emit/lower.rs keeps at 64 when its 32-lane retries fail)
    // is the driver's refusal waiting to happen: test190's 10 refused compute
    // pipelines were such DXIL (PSV0 lanes 64/64). It is ztrx's refusal here,
    // so our translator (the wave64 split) takes the program.
    if victory_shader::emit::host_max_wave() == 32 && psv0_min_lanes(&h.dxil) == Some(64)
        && std::env::var("NEONBRIDGE_ZTRX_WAVE64_REFUSE").as_deref() != Ok("0")
    {
        return Err("ztrx kept WaveSize(64) on a 32-lane host".into());
    }
    use std::fmt::Write;
    let mut s = String::from("ztrx 1
");
    for u in &h.user_data {
        let root = match u.root {
            RootType::Constants => "c".to_string(),
            RootType::Descriptor => "d".to_string(),
            RootType::Table => "t".to_string(),
            RootType::ImmediateSampler => "i".to_string(),
            RootType::ResourceHeap => "r".to_string(),
            RootType::SamplerHeap => "s".to_string(),
            RootType::Other(k) => format!("o{k}"),
        };
        let _ = writeln!(s, "user {} {} {root} {}", u.index, u.slot, u.dword);
    }
    for d in &h.static_samplers {
        let _ = writeln!(s, "sampler {} {} {} {} {} {} {} {} {} {}", d.filter, d.address[0], d.address[1], d.address[2],
            d.mip_lod_bias, d.max_anisotropy, d.comparison, d.border, d.min_lod, d.max_lod);
    }
    for (kind, list) in [("cbv", &h.root_cbvs), ("srv", &h.root_srvs), ("uav", &h.root_uavs)] {
        for p in list {
            let _ = writeln!(s, "{kind} {p}");
        }
    }
    for v in &h.root_indexed_views {
        let _ = writeln!(s, "view {} {} {} {} {}", v.table_slot, v.constant_slot, v.constant_dword, v.byte_offset, v.shift);
    }
    Ok((h.dxil, s))
}

/// PSV0's MinimumExpectedWaveLaneCount (PSVRuntimeInfo0 offset 16) of a DXIL container.
fn psv0_min_lanes(container: &[u8]) -> Option<u32> {
    let u32_at = |o: usize| container.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if container.get(..4)? != b"DXBC" {
        return None;
    }
    for i in 0..u32_at(28)? as usize {
        let o = u32_at(32 + i * 4)? as usize;
        if container.get(o..o + 4)? == b"PSV0" {
            let size = u32_at(o + 8)? as usize;
            return if size >= 24 { u32_at(o + 12 + 16) } else { None };
        }
    }
    None
}
