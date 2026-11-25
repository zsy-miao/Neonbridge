//! The console's shader compiler as a test generator for the recompiler: HLSL compiled by the
//! console's `dxcompiler_xs.dll` (DXIL + the native RDNA2 code in XDAT → XBCH → FSSM) goes
//! through XCL's RDNA2 → HLSL translation and runs on the PC GPU; the same HLSL compiled by the
//! PC compiler runs next to it, and the two must write the same memory.
//!
//! The compiler is the console's own file and stays outside the repo: the tests run only when
//! `XCL_SAMPLE_DXC` names `dxcompiler_xs.dll` (with its `xbsc_xs.dll` next to it), e.g. from
//! the system directory of a console OS image.

use std::ffi::c_void;
use windows::core::{Interface, GUID, HRESULT, PCWSTR};
use windows::Win32::Graphics::Direct3D::Dxc::{DxcBuffer, IDxcBlob, IDxcCompiler3, IDxcResult, CLSID_DxcCompiler, DXC_CP_UTF8};
use xcl::gpu::recomp::{dxc, pso, runner};
use xcl::hle::d3d12x::console_list;

type CreateInstance = unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;

/// The console compiler (None: `XCL_SAMPLE_DXC` not set).
fn console_compiler() -> Option<IDxcCompiler3> {
    let path = std::env::var("XCL_SAMPLE_DXC").ok()?;
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    unsafe {
        // its back end is loaded by name when it compiles: loaded first from the same directory
        // (LOAD_WITH_ALTERED_SEARCH_PATH)
        let dir = std::path::Path::new(&path).parent().map(|d| d.to_path_buf()).unwrap_or_default();
        for name in ["xbsc_xs.dll", "newbe_xs.dll"] {
            let w: Vec<u16> = dir.join(name).to_string_lossy().encode_utf16().chain([0]).collect();
            windows_sys::Win32::System::LibraryLoader::LoadLibraryExW(w.as_ptr(), std::ptr::null_mut(), 0x8);
        }
        let h = windows_sys::Win32::System::LibraryLoader::LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), 0x8);
        assert!(!h.is_null(), "LoadLibraryExW({path}) failed");
        let f = windows_sys::Win32::System::LibraryLoader::GetProcAddress(h, b"DxcCreateInstance\0".as_ptr()).expect("DxcCreateInstance");
        let create: CreateInstance = std::mem::transmute(f);
        let mut p = std::ptr::null_mut();
        let hr = create(&CLSID_DxcCompiler, &IDxcCompiler3::IID, &mut p);
        assert!(hr.is_ok() && !p.is_null(), "DxcCreateInstance: {hr:?}");
        Some(IDxcCompiler3::from_raw(p))
    }
}

/// HLSL → the console compiler's container
fn console_compile(c: &IDxcCompiler3, src: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let args: Vec<Vec<u16>> = args.iter().map(|a| a.encode_utf16().chain([0]).collect()).collect();
    let ptrs: Vec<PCWSTR> = args.iter().map(|a| PCWSTR(a.as_ptr())).collect();
    let buf = DxcBuffer { Ptr: src.as_ptr() as *const c_void, Size: src.len(), Encoding: DXC_CP_UTF8.0 };
    unsafe {
        let r: IDxcResult = c.Compile(&buf, Some(&ptrs), None).map_err(|e| format!("Compile: {e}"))?;
        let status = r.GetStatus().map_err(|e| format!("{e}"))?;
        let errors = r
            .GetErrorBuffer()
            .ok()
            .filter(|b| !b.GetBufferPointer().is_null() && b.GetBufferSize() > 0)
            .map(|b| String::from_utf8_lossy(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize())).trim_end_matches('\0').to_string())
            .unwrap_or_default();
        if status.is_err() {
            return Err(format!("console dxc: {errors}"));
        }
        let blob: IDxcBlob = r.GetResult().map_err(|e| format!("{e}"))?;
        Ok(std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()).to_vec())
    }
}

/// The parts of a DXBC container (fourcc, size)
fn parts(blob: &[u8]) -> Vec<(String, usize)> {
    let u = |o: usize| u32::from_le_bytes(blob[o..o + 4].try_into().unwrap()) as usize;
    if blob.len() < 32 || &blob[0..4] != b"DXBC" {
        return Vec::new();
    }
    (0..u(28))
        .map(|i| {
            let o = u(32 + 4 * i);
            (String::from_utf8_lossy(&blob[o..o + 4]).into_owned(), u(o + 4))
        })
        .collect()
}

/// The buffer every kernel works on: inputs, then outputs (byte offsets).
const INTS: u32 = 0;
const FLOATS: u32 = 0x1000;
const OUT: u32 = 0x2000;
const BUF_BYTES: u32 = 0x4000;
/// thread groups of 64 threads
const GROUPS: u32 = 4;

/// The kernel source around `body`: one RWByteAddressBuffer `buf` – a root UAV on the console
/// (its address in the root argument array), u2 (the runner's flat memory) on the PC.
fn kernel(body: &str) -> String {
    format!(
        r#"#ifdef XCL_CONSOLE
RWByteAddressBuffer buf : register(u0);
#define ROOT [RootSignature("UAV(u0)")]
#else
RWByteAddressBuffer buf : register(u2);
#define ROOT
#endif
#define OUT_AT(i) ({OUT}u + 4u * (i))
groupshared uint lds[64];
uint in_u(uint i) {{ return buf.Load({INTS}u + (i & 1023u) * 4u); }}
float in_f(uint i) {{ return asfloat(buf.Load({FLOATS}u + (i & 1023u) * 4u)); }}
void out_u(uint i, uint v) {{ buf.Store(OUT_AT(i), v); }}
void out_f(uint i, float v) {{ out_u(i, asuint(v)); }}
ROOT [numthreads(64, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID, uint3 gtid : SV_GroupThreadID, uint3 gid : SV_GroupID) {{
{body}
}}
"#
    )
}

/// How the two runs' words must agree.
#[derive(Clone, Copy, PartialEq)]
enum Compare {
    Exact,
    /// up to the sign of zero (also of the halves of a word: packed f16): the console's
    /// compiler multiplies by constants with v_mul_legacy_f32 where the HLSL allows it (DXIL
    /// fast math: no signed zeros), and there -0 × c = +0 – on the PC -0
    SignOfZero,
    /// floats, relative error 2e-5 (the console's transcendental and division code)
    Float,
    /// half floats, relative error 2e-3: the console computes them in fp16, a PC driver may
    /// keep fp32 (D3D12 allows the higher precision)
    Half,
}
use Compare::*;

/// (name, body, how the outputs are compared)
const KERNELS: &[(&str, &str, Compare)] = &[
    (
        "integer arithmetic",
        "uint a = in_u(tid.x), b = in_u(tid.x + 256u), c = in_u(tid.x + 512u); uint o = tid.x * 8u;
         out_u(o, a * b + c);
         out_u(o + 1u, (a >> (b & 31u)) ^ (c << 3u));
         out_u(o + 2u, countbits(a) + firstbithigh(b) * 100u + firstbitlow(c) * 10000u);
         out_u(o + 3u, reversebits(a));
         out_u(o + 4u, (uint)((int)a / max(1, (int)(b & 0xFFFFu))));
         out_u(o + 5u, a % max(1u, b & 0xFFFu));
         out_u(o + 6u, min(a, b) + (uint)max((int)b, (int)c));
         out_u(o + 7u, (uint)(((uint64_t)a * b) >> 32));",
        Exact,
    ),
    (
        "conversions",
        "float x = in_f(tid.x) * 1000.0f; uint a = in_u(tid.x); uint o = tid.x * 8u;
         out_u(o, (uint)(int)x);
         out_f(o + 1u, (float)a);
         out_f(o + 2u, (float)(int)a);
         out_u(o + 3u, f32tof16(x) | f32tof16(-x * 0.001f) << 16);
         out_f(o + 4u, f16tof32(a));
         out_f(o + 5u, floor(x) + ceil(x * 0.5f) * 3.0f);
         out_f(o + 6u, round(x * 0.37f));
         out_f(o + 7u, saturate(x * 0.001f) + trunc(x));",
        SignOfZero,
    ),
    (
        "float math",
        "float x = in_f(tid.x), y = in_f(tid.x + 256u); uint o = tid.x * 8u;
         out_f(o, x * y + 1.0f);
         out_f(o + 1u, sqrt(abs(x)));
         out_f(o + 2u, rsqrt(abs(x) + 1.0f));
         out_f(o + 3u, exp2(x * 0.5f));
         out_f(o + 4u, log2(abs(y) + 1.0f));
         out_f(o + 5u, sin(x) + cos(y));
         out_f(o + 6u, x / (y + 20.0f));
         out_f(o + 7u, frac(x * 3.7f) + min(x, y) - max(x, y));",
        Float,
    ),
    (
        "loops and branches per lane",
        "uint a = in_u(tid.x); uint n = a & 63u; uint acc = tid.x;
         for (uint i = 0u; i < n; i++) {
             acc = acc * 31u + i;
             if ((acc & 7u) == 3u) continue;
             if (acc > 0xF0000000u) break;
             acc ^= in_u(i + tid.x);
         }
         if (a & 1u) acc += 5u; else acc = ~acc;
         out_u(tid.x * 8u, acc);
         out_u(tid.x * 8u + 1u, n);",
        Exact,
    ),
    (
        "groupshared memory and barriers",
        "lds[gtid.x] = in_u(tid.x);
         GroupMemoryBarrierWithGroupSync();
         uint s = 0u;
         for (uint i = 0u; i < 64u; i++) s += lds[(gtid.x + i) & 63u] * (i + 1u);
         GroupMemoryBarrierWithGroupSync();
         lds[63u - gtid.x] = s;
         GroupMemoryBarrierWithGroupSync();
         out_u(tid.x * 8u, lds[gtid.x]);
         out_u(tid.x * 8u + 1u, gid.x);",
        Exact,
    ),
    (
        "atomics",
        "uint a = in_u(tid.x); uint old;
         buf.InterlockedAdd(OUT_AT(a & 15u), 1u, old);
         buf.InterlockedMax(OUT_AT(16u + (a >> 28)), a, old);
         buf.InterlockedXor(OUT_AT(32u + (tid.x & 7u)), a);
         buf.InterlockedOr(OUT_AT(40u), 1u << (a & 31u));
         buf.InterlockedMax(OUT_AT(41u), a | 0x80000000u);",
        Exact,
    ),
    (
        "64-bit integers",
        "uint64_t a = ((uint64_t)in_u(tid.x) << 32) | in_u(tid.x + 256u);
         uint64_t b = ((uint64_t)in_u(tid.x + 512u) << 32) | in_u(tid.x + 768u);
         uint64_t r = (a * b + (a >> 7)) ^ (b << 13);
         int64_t d = (int64_t)a / (int64_t)((b & 0xFFFFFFull) | 1ull);
         uint o = tid.x * 8u;
         out_u(o, (uint)r); out_u(o + 1u, (uint)(r >> 32));
         out_u(o + 2u, (uint)d); out_u(o + 3u, (uint)((uint64_t)d >> 32));
         out_u(o + 4u, (uint)min(a, b)); out_u(o + 5u, (uint)(max(a, b) >> 32));",
        Exact,
    ),
    (
        "local arrays indexed per lane",
        "uint arr[32]; uint a = in_u(tid.x);
         for (uint i = 0u; i < 32u; i++) arr[i] = in_u(tid.x * 3u + i) ^ i;
         uint s = 0u;
         for (uint j = 0u; j < 32u; j++) { s = s * 3u + arr[(j * 7u + a) & 31u]; arr[(a >> j) & 31u] += j; }
         out_u(tid.x * 8u, s);
         out_u(tid.x * 8u + 1u, arr[a & 31u]);",
        Exact,
    ),
    (
        "switch and nested loops",
        "uint a = in_u(tid.x), r = 0u;
         for (uint i = 0u; i < (a & 7u); i++) {
             for (uint j = 0u; j < 8u; j++) {
                 switch ((a >> (i + j)) & 3u) {
                     case 0u: r += j; break;
                     case 1u: r ^= i << j; break;
                     case 2u: r = (r & 1u) ? r + 7u : r * 5u + 1u; break;
                     default: r = (r >> 1) | (r << 31); break;
                 }
                 if ((r & 0xFu) == 9u) break;
             }
         }
         out_u(tid.x * 8u, r);",
        Exact,
    ),
    (
        "16-bit floats",
        "float16_t h = (float16_t)in_f(tid.x), k = (float16_t)in_f(tid.x + 256u);
         uint o = tid.x * 8u;
         out_f(o, (float)(h * k + (float16_t)1.0));
         out_f(o + 1u, (float)max(h, k));
         out_f(o + 2u, (float)(h - k * (float16_t)0.5));",
        Half,
    ),
    (
        "16-bit integers",
        "uint16_t u = (uint16_t)in_u(tid.x), v = (uint16_t)(in_u(tid.x) >> 16);
         uint o = tid.x * 8u;
         out_u(o, (uint)(uint16_t)(u * v + (uint16_t)3u));
         out_u(o + 1u, (uint)(uint16_t)(u >> (v & (uint16_t)15u)));
         out_u(o + 2u, (uint)(int)(int16_t)((int16_t)u / (int16_t)((v & (uint16_t)0x7Fu) | (uint16_t)1u)));",
        Exact,
    ),
];

/// The inputs: 1024 integers (varied bit patterns), 1024 floats (-10 … 10 and a few special
/// values).
fn inputs() -> Vec<u32> {
    let mut v = vec![0u32; (BUF_BYTES / 4) as usize];
    let mut x = 0x2545_F491u32;
    for i in 0..1024 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        v[(INTS / 4) as usize + i] = match i % 16 {
            0 => 0,
            1 => u32::MAX,
            2 => 0x8000_0000,
            3 => i as u32,
            _ => x,
        };
        let f = match i % 32 {
            0 => 0.0f32,
            1 => -0.0,
            2 => 1.0,
            3 => 1e-30,
            _ => (x >> 8) as f32 / (1u32 << 24) as f32 * 20.0 - 10.0,
        };
        v[(FLOATS / 4) as usize + i] = f.to_bits();
    }
    v
}

/// One kernel through the console compiler + XCL's translation and through the PC compiler:
/// the output part of the buffer after each.
fn run_both(c: &IDxcCompiler3, body: &str) -> Result<(Vec<u32>, Vec<u32>), String> {
    let src = kernel(body);
    let blob = console_compile(c, &src, &["-E", "main", "-T", "cs_6_6", "-enable-16bit-types", "-D", "XCL_CONSOLE=1"])?;
    let translated = pso::console_to_dxil_for(&blob, pso::Stage::Compute, 0, runner::split_wave64()).map_err(|e| format!("translation: {e}"))?;
    // the console run: the buffer at 0x1_0000, its address the one root argument – in user
    // SGPRs or in a table at 0x100, as XCL passes them on a console list's dispatch
    let (table_at, base) = (0x100usize, 0x1_0000usize);
    let layout = console_list::layout_from_blob(&blob).ok_or("no RTS0")?;
    let args = (base as u64).to_le_bytes();
    let data = inputs();
    let mut mem = vec![0u32; (base + BUF_BYTES as usize) / 4];
    for (k, b) in args[layout.inline as usize..].iter().enumerate() {
        mem[(table_at + k) / 4] |= (*b as u32) << (8 * (k % 4));
    }
    mem[base / 4..].copy_from_slice(&data);
    let sgprs = console_list::user_data(&layout, &args, table_at as u64);
    let console = runner::dispatch(&translated.dxil, [GROUPS, 1, 1], &sgprs, &mem)?;
    let pc_dxil = dxc::compile(&src, "main", "cs_6_6", &["-enable-16bit-types"])?;
    let pc = runner::dispatch(&pc_dxil, [GROUPS, 1, 1], &[], &data)?;
    let out = |m: &[u32], at: usize| m[(at + OUT as usize) / 4..(at + BUF_BYTES as usize) / 4].to_vec();
    Ok((out(&console, base), out(&pc, 0)))
}

/// Every kernel of `KERNELS`: the console's code as XCL runs it writes what the PC compile of
/// the same source writes.
#[test]
fn console_compiled_kernels_match_the_pc_compile() {
    let Some(c) = console_compiler() else { return };
    if runner::adapter().is_err() {
        runner::skip_test("no GPU");
        return;
    }
    let mut failed = Vec::new();
    let only = std::env::var("XCL_KERNEL").ok().and_then(|k| k.parse::<usize>().ok());
    for (k, &(name, body, how)) in KERNELS.iter().enumerate() {
        if only.is_some_and(|o| o != k) {
            continue;
        }
        match run_both(&c, body) {
            Err(e) => failed.push(format!("{name}: {e}")),
            Ok((console, pc)) => {
                let zero = |w: u32| {
                    let w = if w == 0x8000_0000 { 0 } else { w };
                    let half = |h: u32| if h == 0x8000 { 0 } else { h };
                    half(w & 0xFFFF) | half(w >> 16) << 16
                };
                let same = |a: u32, b: u32| match how {
                    _ if a == b => true,
                    Exact => false,
                    SignOfZero => zero(a) == zero(b),
                    Float | Half => {
                        let tolerance = if how == Half { 2e-3 } else { 2e-5 };
                        let (x, y) = (f32::from_bits(a), f32::from_bits(b));
                        (x.is_nan() && y.is_nan()) || (x - y).abs() <= tolerance * y.abs().max(1.0)
                    }
                };
                let bad: Vec<usize> = (0..pc.len()).filter(|&i| !same(console[i], pc[i])).collect();
                println!("{name}: {} of {} words differ", bad.len(), pc.len());
                if let Some(&i) = bad.first() {
                    let mut outputs: Vec<usize> = bad.iter().map(|i| i % 8).collect();
                    outputs.dedup();
                    outputs.sort();
                    outputs.dedup();
                    failed.push(format!("{name}: {} words differ (outputs {outputs:?}), e.g. word {i} (thread {}, output {}): console {:#x}, PC {:#x}", bad.len(), i / 8, i % 8, console[i], pc[i]));
                }
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// Kernel `XCL_KERNEL` (index in `KERNELS`) through the console compiler: its parts, its
/// hardware interface, its disassembly; with `XCL_KERNEL_HLSL` set the translated HLSL.
#[test]
#[ignore]
fn print_a_console_compile() {
    let Some(c) = console_compiler() else { return };
    let which = std::env::var("XCL_KERNEL").ok().and_then(|k| k.parse::<usize>().ok()).unwrap_or(0);
    let src = kernel(KERNELS[which].1);
    let blob = console_compile(&c, &src, &["-E", "main", "-T", "cs_6_6", "-enable-16bit-types", "-D", "XCL_CONSOLE=1"]).unwrap_or_else(|e| panic!("{e}"));
    println!("{}: {} bytes, parts {:?}", KERNELS[which].0, blob.len(), parts(&blob));
    match xcl::gpu::xbox::parse(&blob) {
        Some(sh) => {
            for f in &sh.fssm {
                println!("FSSM: stages {:?}, programs {}", f.stages.iter().map(|s| xcl::gpu::xbox::hw_stage_name(s.hw)).collect::<Vec<_>>(), f.programs.len());
                if let Some(st) = f.stages.first() {
                    println!("  {:?}", xcl::gpu::recomp::CsInterface::from_descriptor(&f.data, st.offset));
                }
                if let Some(p) = f.programs.first() {
                    for l in xcl::gpu::rdna2::disassemble(&f.data[p.offset..p.offset + p.len], 0, true) {
                        println!("  {:4x}: {}", l.pc, l.text);
                    }
                }
            }
        }
        None => println!("not a console container"),
    }
    match pso::console_to_hlsl(&blob, pso::Stage::Compute) {
        Ok((h, _)) => {
            println!("translated: {} bytes of HLSL", h.len());
            if std::env::var("XCL_KERNEL_HLSL").is_ok() {
                for (i, l) in h.lines().enumerate() {
                    println!("{:4} {l}", i + 1);
                }
            }
        }
        Err(e) => println!("translation: {e}"),
    }
}

/// Where the console puts root arguments: a few root signatures, the first instructions of
/// each compiled kernel and its user SGPR count.
#[test]
#[ignore]
fn print_root_argument_placement() {
    let Some(c) = console_compiler() else { return };
    let cases = [
        ("UAV only", "UAV(u0)", "RWByteAddressBuffer o : register(u0);", "o.Store(tid.x * 4u, tid.x);"),
        ("2 constants, UAV", "RootConstants(num32BitConstants=2, b0), UAV(u0)", "RWByteAddressBuffer o : register(u0); cbuffer k : register(b0) { uint k0; uint k1; };", "o.Store(tid.x * 4u, k0 + k1 * tid.x);"),
        ("UAV, 2 constants", "UAV(u0), RootConstants(num32BitConstants=2, b0)", "RWByteAddressBuffer o : register(u0); cbuffer k : register(b0) { uint k0; uint k1; };", "o.Store(tid.x * 4u, k0 + k1 * tid.x);"),
        ("CBV, UAV", "CBV(b0), UAV(u0)", "RWByteAddressBuffer o : register(u0); cbuffer k : register(b0) { uint k0; uint k1; };", "o.Store(tid.x * 4u, k0 + k1 * tid.x);"),
        ("SRV, UAV", "SRV(t0), UAV(u0)", "RWByteAddressBuffer o : register(u0); ByteAddressBuffer i : register(t0);", "o.Store(tid.x * 4u, i.Load(tid.x * 4u));"),
        ("table UAV", "DescriptorTable(UAV(u0))", "RWByteAddressBuffer o : register(u0);", "o.Store(tid.x * 4u, tid.x);"),
        ("20 constants, UAV", "RootConstants(num32BitConstants=20, b0), UAV(u0)", "RWByteAddressBuffer o : register(u0); cbuffer k : register(b0) { uint4 k[5]; };", "o.Store(tid.x * 4u, k[0].x + k[4].w * tid.x);"),
        ("10 constants, UAV", "RootConstants(num32BitConstants=10, b0), UAV(u0)", "RWByteAddressBuffer o : register(u0); cbuffer k : register(b0) { uint4 k[2]; uint2 k2; };", "o.Store(tid.x * 4u, k[0].x + k2.y * tid.x);"),
        ("11 constants, UAV", "RootConstants(num32BitConstants=11, b0), UAV(u0)", "RWByteAddressBuffer o : register(u0); cbuffer k : register(b0) { uint4 k[2]; uint3 k2; };", "o.Store(tid.x * 4u, k[0].x + k2.z * tid.x);"),
        ("UAV, 11 constants", "UAV(u0), RootConstants(num32BitConstants=11, b0)", "RWByteAddressBuffer o : register(u0); cbuffer k : register(b0) { uint4 k[2]; uint3 k2; };", "o.Store(tid.x * 4u, k[0].x + k2.z * tid.x);"),
        ("6 UAVs", "UAV(u0), UAV(u1), UAV(u2), UAV(u3), UAV(u4), UAV(u5)", "RWByteAddressBuffer o0 : register(u0); RWByteAddressBuffer o1 : register(u1); RWByteAddressBuffer o2 : register(u2); RWByteAddressBuffer o3 : register(u3); RWByteAddressBuffer o4 : register(u4); RWByteAddressBuffer o5 : register(u5);", "o0.Store(tid.x * 4u, 1u); o5.Store(tid.x * 4u, 2u); o3.Store(tid.x * 4u, 3u);"),
    ];
    for (name, rs, decls, body) in cases {
        let src = format!("{decls}\n[RootSignature(\"{rs}\")]\n[numthreads(64, 1, 1)]\nvoid main(uint3 tid : SV_DispatchThreadID) {{ {body} }}\n");
        let blob = match console_compile(&c, &src, &["-E", "main", "-T", "cs_6_6"]) {
            Ok(b) => b,
            Err(e) => {
                println!("{name}: {e}");
                continue;
            }
        };
        let Some(sh) = xcl::gpu::xbox::parse(&blob) else { continue };
        for f in &sh.fssm {
            let cs = f.stages.first().and_then(|st| xcl::gpu::recomp::CsInterface::from_descriptor(&f.data, st.offset));
            println!("{name} [{rs}]: user SGPRs {:?}", cs.map(|c| c.user_sgprs));
            if let Some(p) = f.programs.first() {
                for l in xcl::gpu::rdna2::disassemble(&f.data[p.offset..p.offset + p.len], 0, true).iter().skip(2).take(14) {
                    println!("    {}", l.text);
                }
            }
        }
    }
}

/// Where the console puts root arguments for a vertex and a pixel shader: the first
/// instructions of each and the user SGPR counts of their hardware stages.
#[test]
#[ignore]
fn print_graphics_root_argument_placement() {
    let Some(c) = console_compiler() else { return };
    let rs = "RootConstants(num32BitConstants=2, b0), CBV(b1), DescriptorTable(SRV(t0)), StaticSampler(s0)";
    let common = format!("cbuffer k : register(b0) {{ uint k0; uint k1; }};\ncbuffer m : register(b1) {{ float4 m0; }};\nTexture2D<float4> t : register(t0);\nSamplerState s : register(s0);\n#define ROOT [RootSignature(\"{rs}\")]\n");
    let shaders = [
        ("VS", "vs_6_6", "struct O { float4 p : SV_Position; float2 uv : TEXCOORD0; };\nROOT O main(float3 pos : POSITION, float2 uv : TEXCOORD0) { O o; o.p = float4(pos * m0.xyz, (float)k0); o.uv = uv + (float)k1; return o; }\n"),
        ("PS", "ps_6_6", "ROOT float4 main(float4 p : SV_Position, float2 uv : TEXCOORD0) : SV_Target { return t.Sample(s, uv) * m0 + (float)(k0 ^ k1); }\n"),
    ];
    for (name, profile, body) in shaders {
        let src = format!("{common}{body}");
        let blob = match console_compile(&c, &src, &["-E", "main", "-T", profile]) {
            Ok(b) => b,
            Err(e) => {
                println!("{name}: {e}");
                continue;
            }
        };
        let Some(sh) = xcl::gpu::xbox::parse(&blob) else { continue };
        for f in &sh.fssm {
            println!("{name}: FSSM stages {:?}", f.stages.iter().map(|s| xcl::gpu::xbox::hw_stage_name(s.hw)).collect::<Vec<_>>());
            for st in &f.stages {
                // SPI_SHADER_PGM_RSRC2 user SGPR count: bits 5:1 (graphics stages too)
                let rd = |o: usize| f.data.get(st.offset + o..st.offset + o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
                println!("  stage {} descriptor words: {:x?}", xcl::gpu::xbox::hw_stage_name(st.hw), (0..24).map(|k| rd(4 * k).unwrap_or(0)).collect::<Vec<_>>());
            }
            if let Some(p) = f.programs.first() {
                for l in xcl::gpu::rdna2::disassemble(&f.data[p.offset..p.offset + p.len], 0, true).iter().skip(2).take(40) {
                    println!("    {}", l.text);
                }
            }
        }
    }
}

/// The inline / table split of graphics root arguments: n constants + a CBV, for a few n.
#[test]
#[ignore]
fn print_graphics_root_argument_capacity() {
    let Some(c) = console_compiler() else { return };
    for n in [20u32, 22, 23, 24, 25, 40] {
        let rs = format!("RootConstants(num32BitConstants={n}, b0), CBV(b1)");
        let src = format!(
            "cbuffer k : register(b0) {{ uint k[{n}]; }};\ncbuffer m : register(b1) {{ float4 m0; }};\n[RootSignature(\"{rs}\")]\nfloat4 main(float4 p : SV_Position) : SV_Target {{ return m0 + (float)(k[0] + k[{}]); }}\n",
            n - 1
        );
        let blob = match console_compile(&c, &src, &["-E", "main", "-T", "ps_6_6"]) {
            Ok(b) => b,
            Err(e) => {
                println!("{n}: {e}");
                continue;
            }
        };
        let Some(sh) = xcl::gpu::xbox::parse(&blob) else { continue };
        for f in &sh.fssm {
            if let Some(p) = f.programs.first() {
                let lines: Vec<String> = xcl::gpu::rdna2::disassemble(&f.data[p.offset..p.offset + p.len], 0, true).iter().skip(2).take(14).map(|l| l.text.clone()).collect();
                println!("{n} constants + CBV: {}", lines.join(" | "));
            }
        }
    }
}

/// A vertex and a pixel shader of the console's compiler that read root constants and a root
/// CBV – translated, drawn with the arguments passed as on a console list's draw
/// (`console_list::user_data_gfx`: from s8, the rest in the table at s[0:1]): the pixels have
/// the values the arguments give.
#[test]
fn console_compiled_graphics_reads_its_root_arguments() {
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32G32B32_FLOAT;
    let Some(c) = console_compiler() else { return };
    if runner::adapter().is_err() {
        runner::skip_test("no GPU");
        return;
    }
    let common = "cbuffer k : register(b0) { float k0; float k1; float k2; float k3; };\ncbuffer m : register(b1) { float4 m0; float4 m1; };\n#define ROOT [RootSignature(\"RootConstants(num32BitConstants=4, b0), CBV(b1)\")]\n";
    let vs = format!("{common}struct O {{ float4 p : SV_Position; float4 c : COLOR0; }};\nROOT O main(float3 pos : POSITION) {{ O o; o.p = float4(pos.xy * k2, 0.5, 1.0); o.c = m1 * k3; return o; }}\n");
    let ps = format!("{common}ROOT float4 main(float4 p : SV_Position, float4 c : COLOR0) : SV_Target {{ return float4(k0, k1, m0.x + c.x, m0.y + c.y); }}\n");
    let vs_blob = console_compile(&c, &vs, &["-E", "main", "-T", "vs_6_6"]).unwrap_or_else(|e| panic!("VS: {e}"));
    let ps_blob = console_compile(&c, &ps, &["-E", "main", "-T", "ps_6_6"]).unwrap_or_else(|e| panic!("PS: {e}"));
    let vs_dxil = pso::console_to_dxil(&vs_blob, pso::Stage::Vertex, 0).unwrap_or_else(|e| panic!("VS translation: {e}")).dxil;
    let ps_dxil = pso::console_to_dxil(&ps_blob, pso::Stage::Pixel, 0).unwrap_or_else(|e| panic!("PS translation: {e}")).dxil;
    // root arguments: 4 constants (0.25, 0.5, 2.0 – the position scale, 0.5), the CBV's address
    let (cbv, table_at) = (0x1000usize, 0x100usize);
    let layout = console_list::layout_from_blob(&vs_blob).expect("RTS0");
    let mut args: Vec<u8> = [0.25f32, 0.5, 2.0, 0.5].iter().flat_map(|f| f.to_le_bytes()).collect();
    args.extend((cbv as u64).to_le_bytes());
    let mut mem = vec![0u32; 0x2000 / 4];
    for (k, f) in [0.125f32, 0.0625, 0.0, 0.0, 0.5, 0.25, 0.0, 0.0].iter().enumerate() {
        mem[cbv / 4 + k] = f.to_bits();
    }
    for (k, b) in args[layout.inline_gfx as usize..].iter().enumerate() {
        mem[(table_at + k) / 4] |= (*b as u32) << (8 * (k % 4));
    }
    let sgprs = console_list::user_data_gfx(&layout, &args, table_at as u64);
    // a triangle over the whole target (positions doubled by k2 = 2)
    let tri = [-0.5f32, -0.5, 0.0, 1.5, -0.5, 0.0, -0.5, 1.5, 0.0];
    let el = [runner::VertexElement { semantic: "POSITION", index: 0, format: DXGI_FORMAT_R32G32B32_FLOAT, offset: 0 }];
    let px = runner::draw(&vs_dxil, &ps_dxil, &el, &tri, 12, &sgprs, &mem, 8, [9.0; 4]).unwrap_or_else(|e| panic!("draw: {e}"));
    // k0, k1, m0.x + m1.x · k3, m0.y + m1.y · k3
    let want = [0.25f32, 0.5, 0.125 + 0.25, 0.0625 + 0.125];
    let p = px[4 * 8 + 4];
    println!("pixel {p:?}, want {want:?}");
    assert!(p.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-5), "pixel {p:?}, want {want:?}");
}

/// Derivatives across a quad where a primitive's edge splits it: a quad of two triangles,
/// the pixel shader returns ddx_fine / ddy_fine of a linear attribute – the same everywhere,
/// on the shared diagonal too. The console takes them across the quad's lanes (DPP), the
/// helper lanes of the other triangle included; a translation whose lane reads miss helper
/// lanes gets them wrong along every edge (seams on large quads).
#[test]
fn console_compiled_derivatives_hold_on_triangle_edges() {
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32G32B32_FLOAT;
    let Some(c) = console_compiler() else { return };
    if runner::adapter().is_err() {
        runner::skip_test("no GPU");
        return;
    }
    let root = "#define ROOT [RootSignature(\"RootFlags(ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT)\")]\n";
    let vs = format!("{root}struct O {{ float4 p : SV_Position; float2 uv : TEXCOORD0; }};\nROOT O main(float3 pos : POSITION) {{ O o; o.p = float4(pos.xy, 0.5, 1.0); o.uv = pos.xy * 16.0; return o; }}\n");
    let ps = format!("{root}ROOT float4 main(float4 p : SV_Position, float2 uv : TEXCOORD0) : SV_Target {{ return float4(ddx_fine(uv.x), ddy_fine(uv.y), ddx_fine(uv.y), 1.0); }}\n");
    let vs_blob = console_compile(&c, &vs, &["-E", "main", "-T", "vs_6_6"]).unwrap_or_else(|e| panic!("VS: {e}"));
    let ps_blob = console_compile(&c, &ps, &["-E", "main", "-T", "ps_6_6"]).unwrap_or_else(|e| panic!("PS: {e}"));
    let vs_dxil = pso::console_to_dxil(&vs_blob, pso::Stage::Vertex, 0).unwrap_or_else(|e| panic!("VS translation: {e}")).dxil;
    let ps_dxil = pso::console_to_dxil(&ps_blob, pso::Stage::Pixel, 0).unwrap_or_else(|e| panic!("PS translation: {e}")).dxil;
    // the whole 32x32 target as two triangles: uv steps by 1 per pixel in x, by −1 in y
    let quad = [-1.0f32, -1.0, 0.0, 1.0, -1.0, 0.0, -1.0, 1.0, 0.0, 1.0, -1.0, 0.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0];
    let el = [runner::VertexElement { semantic: "POSITION", index: 0, format: DXGI_FORMAT_R32G32B32_FLOAT, offset: 0 }];
    let size = 32usize;
    let px = runner::draw(&vs_dxil, &ps_dxil, &el, &quad, 12, &[], &[0u32; 64], size as u32, [9.0; 4]).unwrap_or_else(|e| panic!("draw: {e}"));
    let want = [1.0f32, -1.0, 0.0, 1.0];
    let bad: Vec<(usize, usize, [f32; 4])> = (0..size * size).filter(|&i| px[i].iter().zip(want).any(|(a, b)| (a - b).abs() > 1e-4)).map(|i| (i % size, i / size, px[i])).collect();
    println!("{} of {} pixels wrong, first: {:?}", bad.len(), size * size, bad.iter().take(6).collect::<Vec<_>>());
    assert!(bad.is_empty(), "{} pixels with wrong derivatives, first {:?}", bad.len(), &bad[..bad.len().min(6)]);
}
