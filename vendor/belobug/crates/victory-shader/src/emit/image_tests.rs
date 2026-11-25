//! Image sampling forms: hand-assembled GFX10 compute programs (our own
//! code) that read per-lane addresses from a buffer, run one image
//! instruction and store what it returns. On the host GPU the results are
//! checked against texels worked out on the CPU (skipped without an SM 6.6
//! device); every translation is checked by the validator when `dxil.dll`
//! is found.

use windows::Win32::Graphics::Direct3D12::*;

use super::host::{Dispatch, Host, Root, Stride, View};
use super::{HostShader, emit};
use crate::dxbc::{self, Container};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{ShaderError, io, ir, resource, structure};

/// Lanes in the group (one wave of 32: eight quads).
const LANES: usize = 32;
/// Address dwords each lane reads (v2..v17).
const ADDR: usize = 16;

// Encodings (GFX10 ISA reference, "Microcode Formats").
const S_ENDPGM: u32 = 0xBF81_0000;
const S_WAITCNT_0: u32 = 0xBF8C_0000;
const ZERO: u32 = 0x80;
const LITERAL: u32 = 0xFF;
const IMAGE_STORE: u32 = 0x08;

fn s_mov(d: u8, src: u32) -> u32 {
    0xBE80_0300 | u32::from(d) << 16 | src
}

/// `s_load_dwordx<n>` (op 2 x4, 3 x8) of s[d..] from s[base:base+1] + offset.
fn s_load(op: u32, d: u8, base: u8, offset: u32) -> [u32; 2] {
    [
        0xF400_0000 | op << 18 | u32::from(d) << 6 | u32::from(base / 2),
        0xFA00_0000 | offset,
    ]
}

fn v_mov(d: u8, src: u32) -> u32 {
    0x7E00_0000 | u32::from(d) << 17 | 1 << 9 | src
}

fn v_lshl(d: u8, by: u32, v: u8) -> u32 {
    0x1A << 25 | u32::from(d) << 17 | u32::from(v) << 9 | (0x80 + by)
}

/// MUBUF `op` with offen: v[data..] at s[rsrc..]'s base + v[addr] + offset.
fn mubuf(op: u32, data: u8, addr: u8, rsrc: u8, offset: u32) -> [u32; 2] {
    [
        0xE000_0000 | op << 18 | 1 << 12 | offset,
        u32::from(addr) | u32::from(data) << 8 | u32::from(rsrc / 4) << 16 | ZERO << 24,
    ]
}

/// A 2D image instruction: addresses from v2; data v20.. with the resource
/// in s[8:15] and the sampler in s[16:19], a store's v4.. to s[24:31].
#[derive(Clone, Copy)]
struct Mimg {
    op: u32,
    dmask: u8,
    a16: bool,
    d16: bool,
    /// A sample with implicit derivatives.
    implicit: bool,
}

impl Mimg {
    fn words(self) -> [u32; 2] {
        let (data, rsrc) = if self.op == IMAGE_STORE {
            (4, 24)
        } else {
            (20, 8)
        };
        [
            0xF000_0000
                | (self.op & 0x7F) << 18
                | self.op >> 7
                | 1 << 3
                | u32::from(self.dmask) << 8,
            2 | data << 8
                | (rsrc / 4) << 16
                | (16 / 4) << 21
                | u32::from(self.a16) << 30
                | u32::from(self.d16) << 31,
        ]
    }
}

/// The MIMG opcode the decoder names `name`.
fn op(name: &str) -> u32 {
    (0..256)
        .find(|&op| {
            let i = Mimg {
                op,
                dmask: 1,
                a16: false,
                d16: false,
                implicit: false,
            };
            let code: Vec<u8> = i
                .words()
                .iter()
                .chain(&[S_ENDPGM])
                .flat_map(|w| w.to_le_bytes())
                .collect();
            crate::gfx10::decode(&code).is_ok_and(|d| d[0].name() == Some(name))
        })
        .unwrap_or_else(|| panic!("{name}"))
}

fn mimg(name: &str, dmask: u8) -> Mimg {
    let explicit = ["l", "lz", "d", "cd"];
    Mimg {
        op: op(name),
        dmask,
        a16: false,
        d16: false,
        implicit: name.starts_with("image_sample")
            && !name.split('_').any(|t| explicit.contains(&t)),
    }
}

/// User data: s0 the SRV table (the image at heap index 0, the address
/// buffer at 1), s1 the sampler table, s2 the UAV table (index 2). Each
/// lane loads 16 address dwords into v2..v17, `literals` then set some to
/// constants, and the instruction's four result dwords are stored at
/// 16 × the lane (stores write the image instead).
fn program(inst: Mimg, literals: &[(u8, u32)]) -> Vec<u32> {
    let mut w = vec![
        s_mov(4, 0),
        s_mov(5, ZERO),
        s_mov(6, 1),
        s_mov(7, ZERO),
        s_mov(20, 2),
        s_mov(21, ZERO),
    ];
    w.extend(s_load(3, 8, 4, 0));
    w.extend(s_load(2, 32, 4, 0x20));
    w.extend(s_load(2, 16, 6, 0));
    w.extend(s_load(3, 24, 20, 0));
    w.push(S_WAITCNT_0);
    for r in 20..24 {
        w.push(v_mov(r, ZERO));
    }
    w.push(v_lshl(1, 6, 0));
    for k in 0..4 {
        w.extend(mubuf(0x0E, 2 + 4 * k, 1, 32, 16 * u32::from(k)));
    }
    w.push(S_WAITCNT_0);
    for &(r, x) in literals {
        w.extend([v_mov(r, LITERAL), x]);
    }
    w.extend(inst.words());
    w.push(S_WAITCNT_0);
    if inst.op != IMAGE_STORE {
        w.push(v_lshl(24, 4, 0));
        w.extend(mubuf(0x1E, 20, 24, 24, 0));
    }
    w.push(S_ENDPGM);
    w
}

fn native(words: &[u32]) -> NativeProgram {
    let table = |user_data, slot| RootParam {
        root: RootType::Table,
        table_offset_dwords: 0,
        sgprs: 1,
        base_register: 0,
        user_data,
        slot,
    };
    let used = |kind, user_data, slot, dwords| UsedResource {
        kind,
        root: RootType::Table,
        table_offset_dwords: 0,
        lower_bound: 0,
        stride: 0,
        dwords,
        user_data,
        slot,
    };
    NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 32,
        layout: vec![table(0, 1), table(1, 2), table(2, 3)],
        used: vec![
            used(ResourceType::Srv, 0, 1, 8),
            used(ResourceType::Sampler, 1, 2, 4),
            used(ResourceType::Uav, 2, 3, 8),
        ],
        io: StageIo::Cs(ComputeIo {
            rsrc2: 3 << 1,
            threads: [LANES as u32, 1, 1],
        }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    }
}

/// What validator 1.6, the version the translation declares, says of
/// implicit derivatives in a compute shader (1.8 takes them). It refuses
/// the plain `image_sample` there as well; the host's driver runs them.
const CS_DERIVATIVES: &str = "Function uses features incompatible with the shader stage (cs)";

/// `inst`'s program translated, and validated when the validator is found
/// (`CS_DERIVATIVES` let through for a sample with implicit derivatives).
fn translate(inst: Mimg, literals: &[(u8, u32)]) -> Result<HostShader, ShaderError> {
    let s = lower(&program(inst, literals))?;
    if let Ok(mut v) =
        super::validator::Validator::load(super::validator::Validator::default_path().as_deref())
    {
        match v.validate(&mut s.dxil.clone()) {
            Err(e) if !(inst.implicit && e.contains(CS_DERIVATIVES)) => panic!("validation: {e}"),
            _ => {}
        }
    }
    Ok(s)
}

fn lower(words: &[u32]) -> Result<HostShader, ShaderError> {
    let n = native(words);
    let p = ir::build(&n.code, n.wave64, n.vgprs)?;
    let res = resource::track(&p, &n.layout, n.stage)?;
    let io = io::raise(&p, &n)?;
    let st = structure::structure(&p)?;
    let bytes = dxbc::build(&[]);
    let c = Container::parse(&bytes)?;
    emit(&c, &n, &p, &st, &io, &res, crate::emit::Target::ANY)
}

fn host() -> Option<Host> {
    match Host::new(false) {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("no SM 6.6 device ({e}); skipped");
            None
        }
    }
}

/// Point sampling, clamped; LESS_EQUAL for comparisons (passes when the
/// reference is at most the texel).
fn point(compare: bool) -> D3D12_SAMPLER_DESC {
    D3D12_SAMPLER_DESC {
        Filter: if compare {
            D3D12_FILTER_COMPARISON_MIN_MAG_MIP_POINT
        } else {
            D3D12_FILTER_MIN_MAG_MIP_POINT
        },
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC_LESS_EQUAL,
        MaxLOD: f32::MAX,
        ..Default::default()
    }
}

/// Runs `inst` over the lanes' addresses with `image` at heap index 0 and
/// `out` at 2: what `out` holds afterwards.
fn run(
    host: &Host,
    inst: Mimg,
    literals: &[(u8, u32)],
    (image, out): (View, View),
    sampler: D3D12_SAMPLER_DESC,
    addr: &[[u32; ADDR]],
) -> Result<Vec<u32>, String> {
    let s = translate(inst, literals).map_err(|e| e.to_string())?;
    let (root, pso) = host.compute_pipeline(&s)?;
    let mut data = vec![0; s.user_data_dwords().max(1) as usize];
    for u in &s.user_data {
        data[u.index as usize] = match u.slot {
            1 | 2 => 0,
            3 => 2 * 32,
            n => panic!("user data of slot {n}"),
        };
    }
    let words: Vec<u32> = addr.iter().flatten().copied().collect();
    let views = [
        image,
        View::Buffer {
            data: &words,
            stride: Stride::Raw,
        },
        out,
    ];
    let out = host.dispatch(
        &root,
        &pso,
        &Dispatch {
            roots: vec![(0, Root::Constants(data))],
            views: &views,
            samplers: &[sampler],
            groups: [1, 1, 1],
        },
    )?;
    Ok(out.into_iter().next().unwrap_or_default())
}

fn f(x: f32) -> u32 {
    x.to_bits()
}

/// f16 bits of a power of two or 0 (the gradients below).
fn h(x: f32) -> u32 {
    let b = x.to_bits();
    if x == 0.0 {
        0
    } else {
        (b >> 31) << 15 | ((b >> 23 & 0xFF) + 15 - 127) << 10
    }
}

/// f16 bits of a value an f16 holds exactly (texel centres).
fn half(x: f32) -> u32 {
    let b = x.to_bits();
    assert_eq!(b & 0x1FFF, 0, "{x} is not an f16");
    h(x) | (b >> 13 & 0x3FF)
}

fn offset_word(dx: i32, dy: i32) -> u32 {
    (dx as u32 & 0x3F) | (dy as u32 & 0x3F) << 8
}

/// Lane l's texel offset when it varies per lane: past -8..7 on both sides.
fn lane_offset(l: usize) -> (i32, i32) {
    ((l * 5 % 25) as i32 - 12, (l * 3 % 17) as i32 - 8)
}

/// Level-0 size of the sampled textures, and their levels.
const W: u32 = 8;
const LEVELS: u32 = 4;

/// Texel (x, y) of level n: (x, y, x + 8y + 100n, n).
fn level_texels() -> Vec<[f32; 4]> {
    let mut t = Vec::new();
    for n in 0..LEVELS {
        let s = W >> n;
        for i in 0..s * s {
            t.push([
                (i % s) as f32,
                (i / s) as f32,
                (i % s + 8 * (i / s) + 100 * n) as f32,
                n as f32,
            ]);
        }
    }
    t
}

/// What a clamped point sample of level n at (u, v), offset by (dx, dy)
/// texels, reads.
fn texel(n: u32, (u, v): (f32, f32), (dx, dy): (i32, i32)) -> [u32; 4] {
    let s = (W >> n) as i32;
    let at = |c: f32, d: i32| ((c * s as f32).floor() as i32 + d).clamp(0, s - 1);
    let (x, y) = (at(u, dx), at(v, dy));
    [
        x as f32,
        y as f32,
        (x + 8 * y + 100 * n as i32) as f32,
        n as f32,
    ]
    .map(f32::to_bits)
}

/// Lane l's coordinate: a texel centre of level 0.
fn centre(l: usize) -> (f32, f32) {
    let (x, y) = (l % W as usize, l / W as usize * 2 + 1);
    ((x as f32 + 0.5) / W as f32, (y as f32 + 0.5) / W as f32)
}

/// A lane's coordinate and the level its quad's implicit derivatives pick:
/// quad q's lanes are 2^n texels apart in x (lanes 0, 1) and y (0, 2), n
/// = q mod 3.
fn quad_lane(l: usize) -> ((f32, f32), u32) {
    let (q, i) = (l / 4, l % 4);
    let n = (q % 3) as u32;
    let s = 1usize << n;
    let (x, y) = (
        q * 3 % (8 - s) + (i & 1) * s,
        q * 5 % (8 - s) + (i >> 1) * s,
    );
    (((x as f32 + 0.5) / 8.0, (y as f32 + 0.5) / 8.0), n)
}

/// The depth texture: (3x + 5y) mod 8, in eighths, plus 1/16.
fn depth(x: i32, y: i32) -> f32 {
    let (x, y) = (x.clamp(0, 7), y.clamp(0, 7));
    ((3 * x + 5 * y) % 8) as f32 / 8.0 + 1.0 / 16.0
}

fn depth_texels() -> Vec<f32> {
    (0..64).map(|i| depth(i % 8, i / 8)).collect()
}

/// Lane l's comparison reference, in eighths.
fn reference(l: usize) -> f32 {
    (l * 7 % 9) as f32 / 8.0
}

fn compare(r: f32, d: f32) -> u32 {
    f(if r <= d { 1.0 } else { 0.0 })
}

/// A gather footprint's texels, offset: (x0, y1), (x1, y1), (x1, y0),
/// (x0, y0) for the lane's coordinate at the corner of texels (x, y) and
/// (x + 1, y + 1).
fn footprint(l: usize, (dx, dy): (i32, i32)) -> [(i32, i32); 4] {
    let (x, y) = ((l % 8) as i32 + dx, (l / 8 * 2) as i32 + dy);
    [(x, y + 1), (x + 1, y + 1), (x + 1, y), (x, y)]
}

fn corner(l: usize) -> (f32, f32) {
    (((l % 8) + 1) as f32 / 8.0, ((l / 8 * 2) + 1) as f32 / 8.0)
}

/// One instruction over 32 lanes: its literals, then per lane the address
/// dwords and the four result dwords expected.
struct Case {
    what: &'static str,
    inst: Mimg,
    literals: Vec<(u8, u32)>,
    lanes: Vec<(Vec<u32>, [u32; 4])>,
}

fn case(
    what: &'static str,
    inst: Mimg,
    literals: &[(u8, u32)],
    lane: impl Fn(usize) -> (Vec<u32>, [u32; 4]),
) -> Case {
    Case {
        what,
        inst,
        literals: literals.to_vec(),
        lanes: (0..LANES).map(lane).collect(),
    }
}

fn check(
    host: &Host,
    cases: &[Case],
    image: impl Fn() -> View<'static>,
    sampler: D3D12_SAMPLER_DESC,
) {
    let mut failures = Vec::new();
    for c in cases {
        let addr: Vec<[u32; ADDR]> = c
            .lanes
            .iter()
            .map(|(a, _)| {
                let mut w = [0; ADDR];
                w[..a.len()].copy_from_slice(a);
                w
            })
            .collect();
        let out = View::RawUav(4 * LANES as u32);
        let got = match run(host, c.inst, &c.literals, (image(), out), sampler, &addr) {
            Ok(g) => g,
            Err(e) => {
                failures.push(format!("{}: {e}", c.what));
                continue;
            }
        };
        for (l, (_, want)) in c.lanes.iter().enumerate() {
            let got = &got[4 * l..4 * l + 4];
            if got != want {
                failures.push(format!(
                    "{}: lane {l}: {:?}, expected {:?}",
                    c.what,
                    got.iter().map(|&x| f32::from_bits(x)).collect::<Vec<_>>(),
                    want.map(f32::from_bits)
                ));
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

fn leak<T>(v: Vec<T>) -> &'static [T] {
    Box::leak(v.into_boxed_slice())
}

#[test]
fn texel_offsets_and_levels_sample_the_texels_they_name() {
    let Some(host) = host() else { return };
    let c = |u: f32, v: f32| [f(u), f(v)];
    let ow = offset_word;
    let cases = [
        // An offset known here, in -8..7: DXIL's immediate offsets.
        case(
            "lz_o known",
            mimg("image_sample_lz_o", 0xF),
            &[(2, ow(2, -3))],
            |l| {
                let (u, v) = centre(l);
                (
                    [0].into_iter().chain(c(u, v)).collect(),
                    texel(0, (u, v), (2, -3)),
                )
            },
        ),
        // Outside -8..7 or per lane: the coordinates move instead.
        case(
            "lz_o past -8..7",
            mimg("image_sample_lz_o", 0xF),
            &[(2, ow(-12, 9))],
            |l| {
                let (u, v) = centre(l);
                (
                    [0].into_iter().chain(c(u, v)).collect(),
                    texel(0, (u, v), (-12, 9)),
                )
            },
        ),
        case("lz_o per lane", mimg("image_sample_lz_o", 0xF), &[], |l| {
            let (u, v) = centre(l);
            let o = lane_offset(l);
            (
                [ow(o.0, o.1)].into_iter().chain(c(u, v)).collect(),
                texel(0, (u, v), o),
            )
        }),
        case(
            "l_o per lane at level 1",
            mimg("image_sample_l_o", 0xF),
            &[(5, f(1.0))],
            |l| {
                let (u, v) = centre(l);
                let o = lane_offset(l);
                (
                    [ow(o.0, o.1)].into_iter().chain(c(u, v)).collect(),
                    texel(1, (u, v), o),
                )
            },
        ),
        case(
            "l_o at per-lane levels",
            mimg("image_sample_l_o", 0xF),
            &[(2, ow(1, 2))],
            |l| {
                let (u, v) = centre(l);
                let n = (l % 4) as u32;
                (
                    [0, f(u), f(v), f(n as f32)].to_vec(),
                    texel(n, (u, v), (1, 2)),
                )
            },
        ),
        // Gradients of 2^n texels pick level n.
        case(
            "d_o",
            mimg("image_sample_d_o", 0xF),
            &[(2, ow(-1, 1))],
            |l| {
                let (u, v) = centre(l);
                let n = (l % 3) as u32;
                let g = (1 << n) as f32 / 8.0;
                (
                    [0, f(g), 0, 0, f(g), f(u), f(v)].to_vec(),
                    texel(n, (u, v), (-1, 1)),
                )
            },
        ),
        case(
            "cd_cl_o",
            mimg("image_sample_cd_cl_o", 0xF),
            &[(2, ow(3, 0))],
            |l| {
                let (u, v) = centre(l);
                let (n, clamp) = ((l % 3) as u32, (l / 3 % 4) as u32);
                let g = (1 << n) as f32 / 8.0;
                let a = [0, f(g), 0, 0, f(g), f(u), f(v), f(clamp as f32)];
                (a.to_vec(), texel(n.max(clamp), (u, v), (3, 0)))
            },
        ),
        case("d_g16", mimg("image_sample_d_g16", 0xF), &[], |l| {
            let (u, v) = centre(l);
            let n = (l % 3) as u32;
            let g = h((1 << n) as f32 / 8.0);
            ([g, g << 16, f(u), f(v)].to_vec(), texel(n, (u, v), (0, 0)))
        }),
        // a16 packs the coordinates, not the derivatives.
        case(
            "d (a16)",
            Mimg {
                a16: true,
                ..mimg("image_sample_d", 0xF)
            },
            &[],
            |l| {
                let (u, v) = centre(l);
                let n = (l % 3) as u32;
                let g = f((1 << n) as f32 / 8.0);
                (
                    [g, 0, 0, g, half(u) | half(v) << 16].to_vec(),
                    texel(n, (u, v), (0, 0)),
                )
            },
        ),
        case(
            "d_cl_o_g16",
            mimg("image_sample_d_cl_o_g16", 0xF),
            &[(2, ow(-2, -1))],
            |l| {
                let (u, v) = centre(l);
                let (n, clamp) = ((l % 3) as u32, (l / 3 % 4) as u32);
                let g = h((1 << n) as f32 / 8.0);
                let a = [0, g, g << 16, f(u), f(v), f(clamp as f32)];
                (a.to_vec(), texel(n.max(clamp), (u, v), (-2, -1)))
            },
        ),
        // Implicit derivatives (compute quads): bias and clamp move the level.
        case("o", mimg("image_sample_o", 0xF), &[(2, ow(1, -1))], |l| {
            let ((u, v), n) = quad_lane(l);
            ([0, f(u), f(v)].to_vec(), texel(n, (u, v), (1, -1)))
        }),
        case(
            "b_o",
            mimg("image_sample_b_o", 0xF),
            &[(2, ow(0, 2))],
            |l| {
                let ((u, v), n) = quad_lane(l);
                let bias = (l / 4 % 2) as u32;
                (
                    [0, f(bias as f32), f(u), f(v)].to_vec(),
                    texel((n + bias).min(3), (u, v), (0, 2)),
                )
            },
        ),
        case("cl", mimg("image_sample_cl", 0xF), &[], |l| {
            let ((u, v), n) = quad_lane(l);
            let clamp = if l / 4 % 2 == 0 { 2 } else { 0 };
            (
                [f(u), f(v), f(clamp as f32)].to_vec(),
                texel(n.max(clamp), (u, v), (0, 0)),
            )
        }),
        case(
            "b_cl_o",
            mimg("image_sample_b_cl_o", 0xF),
            &[(2, ow(-1, 0))],
            |l| {
                let ((u, v), n) = quad_lane(l);
                let bias = (l / 8 % 2) as u32;
                let clamp = (l / 4 % 2) as u32 * 3;
                let a = [0, f(bias as f32), f(u), f(v), f(clamp as f32)];
                (
                    a.to_vec(),
                    texel((n + bias).min(3).max(clamp), (u, v), (-1, 0)),
                )
            },
        ),
        // A level known to be 0 makes an explicit-level gather DXIL's.
        case(
            "gather4_l_o green per lane",
            mimg("image_gather4_l_o", 2),
            &[(5, 0)],
            |l| {
                let (u, v) = corner(l);
                let o = lane_offset(l);
                let ys = footprint(l, o).map(|(_, y)| f(y.clamp(0, 7) as f32));
                ([ow(o.0, o.1), f(u), f(v)].to_vec(), ys)
            },
        ),
    ];
    let t = leak(level_texels());
    check(
        &host,
        &cases,
        || View::Levels(W, W, LEVELS, t),
        point(false),
    );
}

#[test]
fn comparisons_and_comparison_gathers_match_per_texel_comparisons() {
    let Some(host) = host() else { return };
    let ow = offset_word;
    let one = |r: f32, (u, v): (f32, f32), (dx, dy): (i32, i32)| {
        let x = (u * 8.0).floor() as i32 + dx;
        let y = (v * 8.0).floor() as i32 + dy;
        [compare(r, depth(x, y)), 0, 0, 0]
    };
    let gathered = |l: usize, o| footprint(l, o).map(|(x, y)| compare(reference(l), depth(x, y)));
    let cases = [
        case(
            "c_lz_o known",
            mimg("image_sample_c_lz_o", 1),
            &[(2, ow(1, -1))],
            |l| {
                let (u, v) = centre(l);
                let r = reference(l);
                ([0, f(r), f(u), f(v)].to_vec(), one(r, (u, v), (1, -1)))
            },
        ),
        case(
            "c_lz_o per lane",
            mimg("image_sample_c_lz_o", 1),
            &[],
            |l| {
                let (u, v) = centre(l);
                let (r, o) = (reference(l), lane_offset(l));
                ([ow(o.0, o.1), f(r), f(u), f(v)].to_vec(), one(r, (u, v), o))
            },
        ),
        // Level known to be 0: SampleCmpLevelZero.
        case(
            "c_l_o at level 0",
            mimg("image_sample_c_l_o", 1),
            &[(6, 0)],
            |l| {
                let (u, v) = centre(l);
                let (r, o) = (reference(l), lane_offset(l));
                ([ow(o.0, o.1), f(r), f(u), f(v)].to_vec(), one(r, (u, v), o))
            },
        ),
        // Implicit derivatives in compute; one level.
        case("c_o", mimg("image_sample_c_o", 1), &[(2, ow(-3, 2))], |l| {
            let ((u, v), _) = quad_lane(l);
            let r = reference(l);
            ([0, f(r), f(u), f(v)].to_vec(), one(r, (u, v), (-3, 2)))
        }),
        // A bias known to be 0: SampleCmp.
        case(
            "c_b_cl with bias 0",
            mimg("image_sample_c_b_cl", 1),
            &[(2, 0)],
            |l| {
                let ((u, v), _) = quad_lane(l);
                let r = reference(l);
                ([0, f(r), f(u), f(v), 0].to_vec(), one(r, (u, v), (0, 0)))
            },
        ),
        case("gather4_c_lz", mimg("image_gather4_c_lz", 1), &[], |l| {
            let (u, v) = corner(l);
            ([f(reference(l)), f(u), f(v)].to_vec(), gathered(l, (0, 0)))
        }),
        case(
            "gather4_c_lz_o per lane",
            mimg("image_gather4_c_lz_o", 1),
            &[],
            |l| {
                let (u, v) = corner(l);
                let o = lane_offset(l);
                (
                    [ow(o.0, o.1), f(reference(l)), f(u), f(v)].to_vec(),
                    gathered(l, o),
                )
            },
        ),
        case(
            "gather4_c_l_o at level 0",
            mimg("image_gather4_c_l_o", 1),
            &[(2, ow(2, 1)), (6, 0)],
            |l| {
                let (u, v) = corner(l);
                (
                    [0, f(reference(l)), f(u), f(v)].to_vec(),
                    gathered(l, (2, 1)),
                )
            },
        ),
    ];
    let t = leak(depth_texels());
    check(&host, &cases, || View::Depth(W, W, t), point(true));
}

#[test]
fn stores_of_some_channels_write_zero_to_the_others() {
    let Some(host) = host() else { return };
    let mut failures = Vec::new();
    for (dmask, d16) in [(0x5u8, false), (0x6, true), (0x8, false)] {
        let inst = Mimg {
            op: IMAGE_STORE,
            dmask,
            a16: false,
            d16,
            implicit: false,
        };
        // Lane l writes texel (l mod 8, l / 8) with channel k = l + k / 4.
        let value = |l: usize, k: usize| l as f32 + k as f32 / 4.0;
        let addr: Vec<[u32; ADDR]> = (0..LANES)
            .map(|l| {
                let mut a = [0; ADDR];
                a[0] = (l % 8) as u32;
                a[1] = (l / 8) as u32;
                let data: Vec<f32> = (0..4)
                    .filter(|ch| dmask >> ch & 1 != 0)
                    .map(|k| value(l, k))
                    .collect();
                for (k, x) in data.iter().enumerate() {
                    if d16 {
                        a[2 + k / 2] |= h16(*x) << (16 * (k % 2));
                    } else {
                        a[2 + k] = f(*x);
                    }
                }
                a
            })
            .collect();
        let dummy: &'static [[f32; 4]] = &[[0.0; 4]];
        let got = run(
            &host,
            inst,
            &[],
            (View::Texture(1, 1, dummy), View::TextureUav(8, 4)),
            point(false),
            &addr,
        )
        .unwrap();
        for l in 0..LANES {
            let want: Vec<u32> = (0..4)
                .map(|ch| {
                    if dmask >> ch & 1 != 0 {
                        f(value(l, ch))
                    } else {
                        0
                    }
                })
                .collect();
            if got[4 * l..4 * l + 4] != want[..] {
                failures.push(format!(
                    "dmask {dmask:#x}: lane {l}: {:?}",
                    &got[4 * l..4 * l + 4]
                ));
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// f16 bits of a small multiple of 1/4 (the store data above).
fn h16(x: f32) -> u32 {
    if x == 0.0 {
        return 0;
    }
    let b = x.to_bits();
    let e = (b >> 23 & 0xFF) + 15 - 127;
    e << 10 | (b & 0x7F_FFFF) >> 13
}

/// Where a form keeps its offset word, bias and level (2D, 32-bit
/// addresses): VGPRs from v2.
fn layout(name: &str) -> (Option<u8>, Option<u8>, u8) {
    let has = |t: &str| name.split('_').skip(2).any(|x| x == t);
    let (o, b, c) = (has("o"), has("b"), has("c"));
    let d = has("d") || has("cd");
    let whole = u8::from(o) + u8::from(b) + u8::from(c);
    let grads = if !d {
        0
    } else if has("g16") {
        2
    } else {
        4
    };
    (
        o.then_some(2),
        b.then_some(2 + u8::from(o)),
        2 + whole + grads + 2,
    )
}

/// Literals for a form: a texel offset in -8..7 and, where given, a bias
/// and level of 0.
fn literals(name: &str, zero_bias: bool, zero_level: bool) -> Vec<(u8, u32)> {
    let (o, b, level) = layout(name);
    let mut l = Vec::new();
    l.extend(o.map(|r| (r, offset_word(3, -4))));
    l.extend(b.filter(|_| zero_bias).map(|r| (r, 0)));
    if zero_level {
        l.push((level, 0));
    }
    l
}

#[test]
fn sample_and_gather_forms_lower_to_valid_dxil() {
    let forms = [
        "image_sample",
        "image_sample_cl",
        "image_sample_o",
        "image_sample_cl_o",
        "image_sample_b",
        "image_sample_b_cl",
        "image_sample_b_o",
        "image_sample_b_cl_o",
        "image_sample_d",
        "image_sample_d_cl",
        "image_sample_d_o",
        "image_sample_d_cl_o",
        "image_sample_cd",
        "image_sample_cd_cl",
        "image_sample_cd_o",
        "image_sample_cd_cl_o",
        "image_sample_d_g16",
        "image_sample_d_cl_g16",
        "image_sample_d_o_g16",
        "image_sample_d_cl_o_g16",
        "image_sample_l",
        "image_sample_l_o",
        "image_sample_lz",
        "image_sample_lz_o",
        "image_sample_c",
        "image_sample_c_cl",
        "image_sample_c_o",
        "image_sample_c_cl_o",
        "image_sample_c_lz",
        "image_sample_c_lz_o",
        "image_sample_c_l",
        "image_sample_c_l_o",
        "image_sample_c_b",
        "image_sample_c_b_cl",
        "image_sample_c_b_o",
        "image_sample_c_b_cl_o",
        "image_gather4_lz",
        "image_gather4_lz_o",
        "image_gather4_l",
        "image_gather4_l_o",
        "image_gather4_c_lz",
        "image_gather4_c_lz_o",
        "image_gather4_c_l",
        "image_gather4_c_l_o",
    ];
    let mut failures = Vec::new();
    for name in forms {
        let c = name.contains("_c_") || name.ends_with("_c");
        let gather = name.starts_with("image_gather4");
        let zero_level = name.contains("_l_") || name.ends_with("_l");
        let lits = literals(name, c, zero_level && (c || gather));
        let dmask = if c || gather { 1 } else { 0xF };
        if let Err(e) = translate(mimg(name, dmask), &lits) {
            failures.push(format!("{name}: {e}"));
        }
    }
    // 16-bit addresses: offset, bias and reference stay dwords.
    for name in [
        "image_sample_b",
        "image_sample_b_cl_o",
        "image_sample_c_lz_o",
        "image_sample_d_cl_o",
        "image_gather4_c_lz_o",
    ] {
        let mut inst = mimg(name, if name.contains("_c_") { 1 } else { 0xF });
        inst.a16 = true;
        let lits = literals(name, false, false);
        if let Err(e) = translate(inst, &lits[..usize::from(name.ends_with("_o"))]) {
            failures.push(format!("{name} (a16): {e}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn forms_without_an_sm_6_6_equivalent_name_why() {
    let cases: &[(&str, bool, bool, &str)] = &[
        // (name, bias 0, level 0, reason)
        (
            "image_gather4",
            false,
            false,
            "a gather with implicit level",
        ),
        (
            "image_gather4_o",
            false,
            false,
            "a gather with implicit level",
        ),
        (
            "image_gather4_cl",
            false,
            false,
            "a gather with implicit level",
        ),
        (
            "image_gather4_c",
            false,
            false,
            "a gather with implicit level",
        ),
        (
            "image_gather4_c_o",
            false,
            false,
            "a gather with implicit level",
        ),
        (
            "image_gather4_b",
            false,
            false,
            "a gather with a level bias",
        ),
        (
            "image_gather4_b_o",
            false,
            false,
            "a gather with a level bias",
        ),
        (
            "image_gather4_c_b_cl_o",
            true,
            false,
            "a gather with a level bias",
        ),
        // image_gather4_l of a 2D texture is emulated with four loads.
        (
            "image_gather4_c_l_o",
            false,
            false,
            "a gather at a level not known to be 0",
        ),
        (
            "image_sample_c_l",
            false,
            false,
            "a comparison sample at a level not known to be 0",
        ),
        (
            "image_sample_c_b",
            false,
            false,
            "a comparison sample with a level bias",
        ),
        (
            "image_sample_c_d",
            false,
            false,
            "a comparison sample with explicit derivatives",
        ),
        (
            "image_sample_c_cd_cl_o",
            false,
            false,
            "a comparison sample with explicit derivatives",
        ),
        (
            "image_sample_c_d_o_g16",
            false,
            false,
            "a comparison sample with explicit derivatives",
        ),
    ];
    for &(name, zero_bias, zero_level, why) in cases {
        let dmask = if name.contains("_c") || name.starts_with("image_gather4") {
            1
        } else {
            0xF
        };
        let lits = literals(name, zero_bias, zero_level);
        match translate(mimg(name, dmask), &lits) {
            Err(ShaderError::Unsupported { what, .. }) => assert_eq!(what, why, "{name}"),
            other => panic!("{name}: {other:?}"),
        }
    }
    // A texel offset per lane needs a level known here.
    let why = "a texel offset past -8..7 or per lane at a level not known here";
    for name in [
        "image_sample_o",
        "image_sample_d_o",
        "image_sample_l_o",
        "image_sample_c_o",
    ] {
        let dmask = if name.contains("_c_") { 1 } else { 0xF };
        match translate(mimg(name, dmask), &[]) {
            Err(ShaderError::Unsupported { what, .. }) => assert_eq!(what, why, "{name}"),
            other => panic!("{name}: {other:?}"),
        }
    }
}
