//! A least amount of indirect light.
//!
//! Grand Theft Auto VI's indirect diffuse light comes from probes it fills
//! by tracing rays, which do not run here yet: its program that resolves
//! them (`CS_ScreenProbes_TemporalResolve_*`) leaves next to nothing, and
//! whatever the sun does not reach directly is black. Right after that
//! program, its output is replaced by a constant (it used to be a floor
//! under what the program wrote, but that is garbage: see the shader). The
//! constant is the same everywhere: the light in these images does not go with the title's exposure
//! (well-lit surfaces hold about the same in a shot at -5.5 EV and in one at
//! -9.75). The title's own lighting then applies it with the surfaces'
//! colors. Raised, not added: the result is the next frame's history.
//!
//! `VICTORY_AMBIENT=<level>` sets it (0.6 by default, 0 for none); `off`
//! keeps the title's own output.
//! shortcut: the same light everywhere, from everywhere; gone once rays run.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::s;

use crate::host::{HostList, host};
use crate::object::This;

/// The programs it follows, by the start of their name.
pub(crate) const PROGRAM: &[u8] = b"CS_ScreenProbes_TemporalResolve";
/// Where they write the indirect diffuse light (R9G9B9E5 as 32-bit
/// integers): root parameter and place in its table.
pub(crate) const LIGHT: (u8, u32) = (1, 176);

/// 0.6 looked balanced in run a2. `VICTORY_AMBIENT=off` leaves what the
/// title's resolve wrote; 0 is no indirect diffuse light at all.
const DEFAULT: f32 = 0.6;

/// The floor; `None` if off.
pub(crate) fn level() -> Option<f32> {
    static LEVEL: OnceLock<f32> = OnceLock::new();
    let level = *LEVEL.get_or_init(|| {
        let set = std::env::var("VICTORY_AMBIENT").unwrap_or_default();
        if set.trim() == "off" {
            return -1.0;
        }
        set.trim().parse().ok().filter(|l: &f32| l.is_finite() && *l >= 0.0).unwrap_or(DEFAULT)
    });
    (level >= 0.0).then_some(level)
}

/// The light's colour: warm white, or what `VICTORY_AMBIENT_TINT=r,g,b`
/// gives. With `VICTORY_AMBIENT_FILE=<file>` (diagnostics builds) the file's
/// `level r g b` is read again twice a second, to tune both by eye in a
/// running game.
fn tint() -> [f32; 3] {
    let live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, tint)) = *live {
        return tint;
    }
    static SET: OnceLock<[f32; 3]> = OnceLock::new();
    *SET.get_or_init(|| {
        let set: Vec<f32> = std::env::var("VICTORY_AMBIENT_TINT")
            .unwrap_or_default()
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        match set[..] {
            [r, g, b] => [r, g, b],
            _ => [1.0, 0.98, 0.94],
        }
    })
}

/// The level and colour last read from `VICTORY_AMBIENT_FILE`, if any.
static LIVE: std::sync::Mutex<Option<(f32, [f32; 3])>> = std::sync::Mutex::new(None);

/// The program that writes the title's reflections for the last time in a
/// frame, by the start of its name, and where it writes them (R9G9B9E5 as
/// 32-bit integers): root parameter and place in its table. The lighting
/// programs read that image next.
///
/// Without rays the reflections are the sky's, unoccluded, wherever a
/// surface is glossy: indoors a wooden floor, a counter and glass tops wore
/// a white glaze five to ten times brighter than anything in the room (run
/// ciel1, 2026-11-25; it did not move with the ambient level). They are held
/// to a part of the light around ([`keep`]).
pub(crate) const REFLECTED: &[u8] = b"CS_UpdateHistory";
pub(crate) const REFLECTIONS: (u8, u32) = (1, 40);

/// The most a reflection may hold, as a part of the ambient light's level:
/// `VICTORY_AMBIENT_REFLECT=<part>` (0.5 by default; 0 leaves the title's
/// reflections alone); a ninth number in the file sets it in a running game.
/// shortcut: stands in for what the title's traced reflections would see.
static KEEP: std::sync::Mutex<Option<f32>> = std::sync::Mutex::new(None);

/// The least a reflection holds, as a part of the light around: a tenth
/// number in the file (0, the default, leaves dark reflections dark).
/// shortcut: without rays, what should mirror the sky (still water) sees
/// nothing and is black; this gives it a part of the ambient light instead.
static LEAST: std::sync::Mutex<f32> = std::sync::Mutex::new(0.0);

fn keep() -> f32 {
    if let Some(live) = *KEEP.lock().unwrap_or_else(|e| e.into_inner()) {
        return live;
    }
    static SET: OnceLock<f32> = OnceLock::new();
    *SET.get_or_init(|| {
        std::env::var("VICTORY_AMBIENT_REFLECT")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .filter(|k: &f32| k.is_finite() && *k >= 0.0)
            .unwrap_or(0.5)
    })
}

/// The program that draws the sky as the title reflects it, by the start of
/// its name: a small image of the whole sky, in the light's own units.
pub(crate) const SKY: &[u8] = b"ps_sky_paraboloid_reflection";

/// The sky dome that program last drew into: the host copy, held, since
/// the title's object may go at any time.
///
/// The program draws twice a frame, on two lists recorded at once: the dome
/// (128x128, R16G16B16A16_FLOAT) and another image of the sky (R11G11B10).
/// Following whichever was recorded last made the light jump between the
/// two, three times apart, from frame to frame (run don1, 2026-11-25).
static SKY_IMAGE: std::sync::Mutex<Option<std::sync::Arc<crate::resource::HostTexture>>> =
    std::sync::Mutex::new(None);

pub(crate) fn sky_drawn(image: usize) {
    let Some(copy) = crate::resource::host_copy(image as This) else { return };
    // SAFETY: a live resource.
    let d = unsafe { copy.texture.GetDesc() };
    if d.Format != DXGI_FORMAT_R16G16B16A16_FLOAT || d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D {
        return;
    }
    let mut held = SKY_IMAGE.lock().unwrap_or_else(|e| e.into_inner());
    if !held.as_ref().is_some_and(|h| std::sync::Arc::ptr_eq(h, &copy)) {
        *held = Some(copy);
    }
}

/// The light follows the sky: its level and colour are the sky image's mean
/// times a gain, over a least bluish level. Hand-set levels that looked
/// right were 0.07 at night, 0.3 at dusk and 1.4 at noon (runs lum1, pr1,
/// av1, 2026-11-25), with sky means of 0.001 at night and 0.65 at dawn; one
/// level for all hours either drowned the title's own shadows or left what
/// the sun does not reach black. On by default; `VICTORY_AMBIENT_SKY=off`
/// keeps the constant, `gain,least,power` sets it; a file of two or three
/// numbers, or six or seven with the shading's, does so in a running game.
/// shortcut: the sky's mean stands in for the light the title's traced
/// probes would gather; indoors it is the sky's too.
static FOLLOW: std::sync::Mutex<Option<Option<[f32; 3]>>> = std::sync::Mutex::new(None);

/// Gain, least level and power: the level is `gain * mean^power + least`.
/// The power is under one because the title's exposure follows the light:
/// the sky's mean goes from 0.001 at night to several units by day, the
/// levels that look right only from 0.07 to 1.4.
fn follow() -> Option<[f32; 3]> {
    if let Some(live) = *FOLLOW.lock().unwrap_or_else(|e| e.into_inner()) {
        return live;
    }
    static SET: OnceLock<Option<[f32; 3]>> = OnceLock::new();
    *SET.get_or_init(|| {
        // On unless `off`: by day and by night it looked right where a
        // constant did not (runs ciel3, opt1b, 2026-11-25).
        let set = std::env::var("VICTORY_AMBIENT_SKY").unwrap_or_default();
        if set.trim() == "off" {
            return None;
        }
        let mut follow = [0.36, 0.05, 0.4];
        for (to, v) in follow.iter_mut().zip(set.split(',')) {
            if let Ok(v) = v.trim().parse() {
                *to = v;
            }
        }
        Some(follow)
    })
}

/// Where the resolve reads the scene's depth (reversed, D32 with stencil):
/// root parameter and place in its table.
pub(crate) const DEPTH: (u8, u32) = (0, 464);

/// How much less of the light reaches creases and corners, worked out from
/// the depth image alone: strength (0 for none), the radius looked at in
/// metres, the near plane's distance, and the tangent of half the vertical
/// field of view (the last two are guesses the picture is not sensitive
/// to). `VICTORY_AMBIENT_SHADE=s,r,n,t` sets them; the file's values after
/// its first four replace them.
/// shortcut: stands in for the occlusion the title's traced probes carry.
static SHADE: std::sync::Mutex<Option<[f32; 4]>> = std::sync::Mutex::new(None);

fn shade() -> [f32; 4] {
    if let Some(live) = *SHADE.lock().unwrap_or_else(|e| e.into_inner()) {
        return live;
    }
    static SET: OnceLock<[f32; 4]> = OnceLock::new();
    *SET.get_or_init(|| {
        let mut shade = [1.0, 0.6, 0.1, 0.45];
        let set = std::env::var("VICTORY_AMBIENT_SHADE").unwrap_or_default();
        for (to, v) in shade.iter_mut().zip(set.split(',')) {
            if let Ok(v) = v.trim().parse() {
                *to = v;
            }
        }
        shade
    })
}

/// The level from `VICTORY_AMBIENT_FILE`, if it gave one; starts the thread
/// that reads the file.
fn live_level() -> Option<f32> {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        if !cfg!(feature = "diagnostics") {
            return;
        }
        let Some(file) = std::env::var_os("VICTORY_AMBIENT_FILE") else { return };
        let _ = std::thread::Builder::new().name("victory-ambient".into()).spawn(move || {
            loop {
                let read: Vec<f32> = std::fs::read_to_string(&file)
                    .unwrap_or_default()
                    .split_whitespace()
                    .filter_map(|v| v.parse().ok())
                    .collect();
                // Two, three, six or seven numbers: gain, least, a power if
                // their count is odd, then the shading's four.
                if let [gain, least, ref rest @ ..] = read[..]
                    && matches!(rest.len(), 0 | 1 | 4 | 5)
                {
                    let (power, rest) = match rest.len() % 2 {
                        1 => (rest[0], &rest[1..]),
                        _ => (0.4, rest),
                    };
                    if let [s, r, n, t] = rest[..] {
                        *SHADE.lock().unwrap_or_else(|e| e.into_inner()) = Some([s, r, n, t]);
                    }
                    let new = Some(Some([gain, least, power]));
                    let mut follow = FOLLOW.lock().unwrap_or_else(|e| e.into_inner());
                    if *follow != new {
                        tracing::info!(gain, least, power, "ambient light follows the sky, set from the file");
                        *follow = new;
                    }
                } else if let [level, r, g, b, ref rest @ ..] = read[..] {
                    *FOLLOW.lock().unwrap_or_else(|e| e.into_inner()) = Some(None);
                    if let [s, r, n, t, ref last @ ..] = rest[..] {
                        *SHADE.lock().unwrap_or_else(|e| e.into_inner()) = Some([s, r, n, t]);
                        if let [k, ref floor @ ..] = last[..] {
                            *KEEP.lock().unwrap_or_else(|e| e.into_inner()) = Some(k.max(0.0));
                            *LEAST.lock().unwrap_or_else(|e| e.into_inner()) =
                                floor.first().map_or(0.0, |f| f.max(0.0));
                        }
                    }
                    let new = Some((level.max(0.0), [r, g, b]));
                    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
                    if *live != new {
                        tracing::info!(level, r, g, b, "ambient light set from the file");
                        *live = new;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
    });
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).map(|(level, _)| level)
}

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=13, b0), DescriptorTable(UAV(u0), SRV(t0), SRV(t1))"
cbuffer Floor : register(b0) {
    float level; float3 tint;
    float strength; float radius; float near; float tan_y;
    float gain; float least;
    float held; float floor_part; float power;
};
Texture2DArray<float4> sky : register(t1);
// The light, from the sky's mean when it follows the sky (gain above 0).
float3 from_around() {
    if (gain <= 0.0) return tint * level;
    uint sw, sh, slices;
    sky.GetDimensions(sw, sh, slices);
    float3 sum = 0.0;
    const int N = 8;
    for (int y = 0; y < N; y++)
        for (int x = 0; x < N; x++) {
            // The sun's own texels are held down: it lights by itself.
            float3 there = sky.Load(int4((x * 2 + 1) * sw / (N * 2), (y * 2 + 1) * sh / (N * 2), 0, 0)).rgb;
            sum += clamp(there, 0.0, 8.0);
        }
    float3 mean = sum / (N * N);
    float m = dot(mean, float3(0.2126, 0.7152, 0.0722));
    // The sky's colour, half washed out (the ground lights too); a dark
    // sky has none worth following.
    const float3 NIGHT = float3(0.6, 0.75, 1.0);
    float3 hue = lerp(NIGHT, lerp(float3(1.0, 1.0, 1.0), mean / max(m, 1e-6), 0.5), saturate(m / 0.05));
    return hue * (gain * pow(max(m, 0.0), power) + least);
}
RWTexture2D<uint> light : register(u0);
Texture2D<float> depth : register(t0);
// The point seen at a texel, in metres from the eye (x right, y up, z away).
float3 seen(int2 p, float2 size) {
    float d = depth.Load(int3(clamp(p, int2(0, 0), int2(size) - 1), 0));
    float z = near / max(d, 1e-7);
    float2 ndc = (float2(p) + 0.5) / size * 2.0 - 1.0;
    return float3(ndc.x * tan_y * size.x / size.y * z, -ndc.y * tan_y * z, z);
}
// How much of the light from around reaches the texel: 1 in the open.
float open_to_light(int2 p, float2 size) {
    float3 c = seen(p, size);
    if (c.z > 400.0) return 1.0;
    float3 l = seen(p + int2(-1, 0), size), r = seen(p + int2(1, 0), size);
    float3 u = seen(p + int2(0, -1), size), d = seen(p + int2(0, 1), size);
    float3 dx = abs(l.z - c.z) < abs(r.z - c.z) ? c - l : r - c;
    float3 dy = abs(u.z - c.z) < abs(d.z - c.z) ? c - u : d - c;
    float3 n = normalize(cross(dy, dx));
    if (dot(n, c) > 0.0) n = -n;
    float px = radius * size.y / (2.0 * tan_y * c.z);
    if (px < 1.5) return 1.0;
    px = min(px, 128.0);
    float noise = frac(52.9829189 * frac(dot(float2(p), float2(0.06711056, 0.00583715))));
    float sum = 0.0;
    const int N = 16;
    for (int i = 0; i < N; i++) {
        float a = i * 2.399963 + noise * 6.2831853;
        float2 o = float2(cos(a), sin(a)) * (sqrt((i + 0.5) / N) * px);
        float3 v = seen(p + int2(round(o)), size) - c;
        float vv = dot(v, v);
        float over = dot(v, n) - 0.004 * c.z - 0.005;
        sum += max(over, 0.0) / sqrt(vv + 1e-6) * saturate(1.0 - vv / (radius * radius));
    }
    return saturate(1.0 - strength * 2.0 * sum / N);
}
float3 unpack(uint v) {
    return float3(v & 511, (v >> 9) & 511, (v >> 18) & 511) * exp2(float(v >> 27) - 24.0);
}
uint pack(float3 c) {
    c = clamp(c, 0.0, 65408.0);
    float most = max(c.r, max(c.g, c.b));
    int e = clamp(int(floor(log2(max(most, 1e-9)))) + 16, 0, 31);
    float scale = exp2(float(24 - e));
    if (most * scale >= 511.5) { e = min(e + 1, 31); scale = exp2(float(24 - e)); }
    uint3 m = min(uint3(c * scale + 0.5), 511u);
    return m.r | (m.g << 9) | (m.b << 18) | (uint(e) << 27);
}
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 at : SV_DispatchThreadID) {
    uint w, h;
    light.GetDimensions(w, h);
    if (at.x >= w || at.y >= h) return;
    // What the resolve left is not kept: without rays its inputs are never
    // written and it holds whatever their memory did (bright magenta on
    // translucent surfaces, blocks of colours indoors: run v6, 2026-11-25).
    if (held > 0.0) {
        // Reflections: no brighter than a part of the light around.
        const float3 LUMA = float3(0.2126, 0.7152, 0.0722);
        float3 seen_there = unpack(light[at.xy]);
        float most = dot(from_around(), LUMA) * held;
        float has = dot(seen_there, LUMA);
        float3 floor_light = from_around() * floor_part;
        if (has > most) light[at.xy] = pack(seen_there * (most / has));
        else if (has < dot(floor_light, LUMA)) light[at.xy] = pack(floor_light);
        return;
    }
    float open = strength > 0.0 ? open_to_light(int2(at.xy), float2(w, h)) : 1.0;
    light[at.xy] = pack(from_around() * open);
}
"#;

/// Views, used in turn: more than the passes of every list in flight.
const SLOTS: u32 = 1024;

struct Pass {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    views: ID3D12DescriptorHeap,
    step: u32,
    next: AtomicU32,
}
// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Pass {}
// SAFETY: as above.
unsafe impl Sync for Pass {}

fn make(device: &ID3D12Device) -> windows::core::Result<Pass> {
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the source and names live through the call; out pointers ours.
    let compiled = unsafe {
        D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            s!("ambient"),
            None,
            None,
            s!("main"),
            s!("cs_5_1"),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    let text = |b: &ID3DBlob| {
        // SAFETY: the blob's own bytes.
        unsafe { std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize()) }
            .to_vec()
    };
    if let Err(e) = compiled {
        if let Some(errors) = errors {
            tracing::warn!("ambient light: {}", String::from_utf8_lossy(&text(&errors)));
        }
        return Err(e);
    }
    let code = text(&code.ok_or_else(windows::core::Error::empty)?);
    // SAFETY: the root signature is inside the shader; plain creations.
    unsafe {
        let root: ID3D12RootSignature = device.CreateRootSignature(0, &code)?;
        let pso = device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: code.as_ptr().cast(),
                BytecodeLength: code.len(),
            },
            ..Default::default()
        })?;
        let kind = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
        let views = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: kind,
            NumDescriptors: SLOTS * 3,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
            NodeMask: 0,
        })?;
        Ok(Pass {
            root,
            pso,
            views,
            step: device.GetDescriptorHandleIncrementSize(kind),
            next: AtomicU32::new(0),
        })
    }
}

fn uav_barrier(list: &ID3D12GraphicsCommandList) {
    let barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        ..Default::default()
    };
    // SAFETY: an open list; the barrier names no resource.
    unsafe { list.ResourceBarrier(&[barrier]) };
}

/// Raises what `light` holds to the floor, in `list`, after the program that
/// wrote it as an image. Whether it recorded anything (the list's bindings
/// are then ours).
pub(crate) fn raise(list: &HostList, light: This, depth: Option<This>) -> bool {
    record(list, light, depth, 0.0)
}

/// Holds the reflections in `image` to a part of the ambient light, in
/// `list`, after the program that wrote them last. Whether it recorded
/// anything (the list's bindings are then ours).
pub(crate) fn hold(list: &HostList, image: This) -> bool {
    let held = keep();
    if held <= 0.0 || !record(list, image, None, held) {
        return false;
    }
    static HELD: std::sync::Once = std::sync::Once::new();
    HELD.call_once(|| tracing::info!(held, "reflections held to the ambient light"));
    true
}

fn record(list: &HostList, light: This, depth: Option<This>, held: f32) -> bool {
    let (Some(level), Some(host)) = (level(), host()) else {
        return false;
    };
    let Some(light) = crate::resource::host_resource(light) else {
        return false;
    };
    static PASS: OnceLock<Option<Pass>> = OnceLock::new();
    let Some(pass) = PASS
        .get_or_init(|| {
            make(&host.device)
                .inspect_err(|e| tracing::warn!(error = %e, "no ambient light"))
                .ok()
        })
        .as_ref()
    else {
        return false;
    };
    // SAFETY: a live resource.
    let d = unsafe { light.GetDesc() };
    static ONCE: std::sync::Once = std::sync::Once::new();
    if d.Format != DXGI_FORMAT_R32_UINT {
        ONCE.call_once(|| tracing::warn!(format = d.Format.0, "no ambient light: another format"));
        return false;
    }
    ONCE.call_once(|| {
        tracing::info!(level, width = d.Width, height = d.Height, "ambient light");
    });
    let slot = pass.next.fetch_add(1, Ordering::Relaxed) % SLOTS * 3;
    // The sky image, if the light follows it and the title has drawn it.
    let sky_image = SKY_IMAGE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let sky = follow().and_then(|f| {
        let t = &sky_image.as_ref()?.texture;
        // SAFETY: a live resource.
        let d = unsafe { t.GetDesc() };
        static SEEN: std::sync::Once = std::sync::Once::new();
        SEEN.call_once(|| {
            tracing::info!(
                format = d.Format.0,
                width = d.Width,
                height = d.Height,
                slices = d.DepthOrArraySize,
                "the sky image the ambient light would follow"
            );
        });
        Some((t, f, d.Format))
    });
    static FOLLOWS: std::sync::Once = std::sync::Once::new();
    if let Some((t, f, _)) = sky {
        FOLLOWS.call_once(|| tracing::info!(gain = f[0], least = f[1], power = f[2], "ambient light follows the sky"));
        list.keep(t);
        let read = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE.0 | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE.0;
        if let Some(copy) = &sky_image
            && let Some(before) = list.transition_copy(copy, read as u32)
        {
            let barrier = crate::host::transition(
                t,
                D3D12_RESOURCE_STATES(before as i32),
                D3D12_RESOURCE_STATES(read),
            );
            // SAFETY: an open list and a live texture the list keeps.
            unsafe { list.list.ResourceBarrier(&[barrier]) };
        }
    }
    list.keep(light);
    // The depth the resolve just read, in the state it read it in; of the
    // light's size, or the shading is left out.
    let depth = depth.and_then(crate::resource::host_resource).filter(|t| {
        // SAFETY: a live resource.
        let t = unsafe { t.GetDesc() };
        t.Format == DXGI_FORMAT_R32G8X24_TYPELESS && (t.Width, t.Height) == (d.Width, d.Height)
    });
    if let Some(depth) = depth {
        list.keep(depth);
    }
    let l = &list.list;
    uav_barrier(l);
    // SAFETY: an open list; a live texture the list keeps, in the
    // unordered-access state as the program before left it; a view in a slot
    // no list in flight reads.
    unsafe {
        let cpu = pass.views.GetCPUDescriptorHandleForHeapStart();
        let gpu = pass.views.GetGPUDescriptorHandleForHeapStart();
        let at = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: cpu.ptr + (slot * pass.step) as usize,
        };
        let view = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_UINT,
            ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
            ..Default::default()
        };
        host.device
            .CreateUnorderedAccessView(light, None::<&ID3D12Resource>, Some(&view), at);
        let seen = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV { MipLevels: 1, ..Default::default() },
            },
        };
        let beside = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: at.ptr + pass.step as usize };
        host.device.CreateShaderResourceView(depth, Some(&seen), beside);
        let around = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: sky.map_or(DXGI_FORMAT_R16G16B16A16_FLOAT, |s| s.2),
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2DARRAY,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2DArray: D3D12_TEX2D_ARRAY_SRV { MipLevels: 1, ArraySize: 1, ..Default::default() },
            },
        };
        let third = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: at.ptr + 2 * pass.step as usize };
        host.device.CreateShaderResourceView(sky.map(|s| s.0), Some(&around), third);
        l.SetDescriptorHeaps(&[Some(pass.views.clone())]);
        l.SetComputeRootSignature(&pass.root);
        l.SetPipelineState(&pass.pso);
        let [r, g, b] = tint();
        let level = live_level().unwrap_or(level);
        let [strength, radius, near, tan_y] = shade();
        let strength = if depth.is_some() { strength } else { 0.0 };
        let [gain, least, power] = sky.map_or([0.0, 0.0, 1.0], |s| s.1);
        let floor = *LEAST.lock().unwrap_or_else(|e| e.into_inner());
        let constants = [level, r, g, b, strength, radius, near, tan_y, gain, least, held, floor, power];
        l.SetComputeRoot32BitConstants(0, 13, constants.as_ptr().cast(), 0);
        l.SetComputeRootDescriptorTable(
            1,
            D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: gpu.ptr + u64::from(slot * pass.step),
            },
        );
        l.Dispatch((d.Width as u32).div_ceil(8), d.Height.div_ceil(8), 1);
    }
    uav_barrier(l);
    true
}

#[cfg(test)]
mod tests {
    /// The pass compiles with the system's compiler and the host takes it.
    #[test]
    fn ambient_pass_builds() {
        let Some(host) = crate::host::host() else {
            return;
        };
        super::make(&host.device).expect("the ambient pass");
    }
}
