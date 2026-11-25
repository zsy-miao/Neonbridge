//! TB_ZTRX_SHADER: the root signature a ztrx victory-shader translation
//! (a `.cso` with a `.ztrx` sidecar) expects. Copied from ztrx
//! victory-shader emit/host.rs root_signature_for and emit/mod.rs
//! HostShader::user_data_dwords (GPL-3.0, credit to its authors): user data
//! of stage k is root parameter k (32-bit constants at b0, space 0), root
//! CBV/SRV/UAVs follow by slot then kind (space 1), immediate samplers are
//! static samplers s<n> (space 0), both heaps directly indexed.
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;

/// ztrx emit/shader.rs USER_DATA_SPACE, SAMPLER_SPACE, ROOT_CBV_SPACE.
pub(crate) const USER_DATA_SPACE: u32 = 0;
pub(crate) const SAMPLER_SPACE: u32 = 0;
pub(crate) const ROOT_CBV_SPACE: u32 = 1;

/// ztrx emit/sampler.rs StaticSampler.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StaticSampler {
    pub filter: u32,
    /// U, V, W.
    pub address: [u32; 3],
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison: u32,
    pub border: u32,
    pub min_lod: f32,
    pub max_lod: f32,
}

/// The parts of ztrx's HostShader the root signature reads (from the sidecar).
#[derive(Debug, Clone, Default)]
pub(crate) struct ZtrxStage {
    /// The user-data dwords the shader reads and the title root parameters that fill them.
    pub user_data: Vec<UserData>,
    pub static_samplers: Vec<StaticSampler>,
    pub root_cbvs: Vec<u8>,
    pub root_srvs: Vec<u8>,
    pub root_uavs: Vec<u8>,
    /// Heap descriptors selected from root state at bind time.
    pub root_indexed_views: Vec<RootIndexedView>,
    /// `user_data_dwords` as the sidecar states it.
    pub declared_dwords: Option<u32>,
}

impl ZtrxStage {
    /// Size of the user-data buffer the shader declares, in dwords (a
    /// multiple of 4; 0 when it reads none). Root constants must cover it.
    pub(crate) fn user_data_dwords(&self) -> u32 {
        let n = self.user_data.iter().map(|u| u.index + 1).max().unwrap_or(0);
        self.declared_dwords.unwrap_or(n.next_multiple_of(4))
    }
}

/// TUXBOX_ZTRX_ROOT_FX (default on, =0 the merged layout): the root of a
/// ztrx pipeline is laid out as VIctory-fxceless victory-gpu
/// pipeline/host.rs HostPipeline::new (lines 381-490; GPL-3.0, credit to
/// its authors): per stage in order, its user data (only when it reads
/// any) at the stage's visibility, then its root CBVs, SRVs and UAVs by
/// slot at the stage's visibility. Their bounded one-view tables
/// (bounded_root_srvs) are not ported; root descriptors are used, as
/// their VICTORY_ROOT_SRV_DIRECT. test192: 32 MS+PS pipelines with ztrx
/// stages were refused 0x80070057 on the merged root (user data of every
/// stage at max(1) dwords, root views deduplicated at VISIBILITY_ALL).
/// TUXBOX_ZTRX_ROOT_SRV_DIRECT=1 (default off): with TUXBOX_ZTRX_ROOT_FX,
/// a ztrx root SRV/UAV is a root descriptor. By default it is a one-view
/// descriptor table (one raw view from the bound address to the buffer's
/// end, in a private slot of the bound view heap), as VIctory-fxceless
/// victory-gpu pipeline/host.rs bounded_root_srvs (default on unless
/// VICTORY_ROOT_SRV_DIRECT) and draw.rs root_srv_table / root UAV tables
/// (GPL-3.0, credit to its authors): a root descriptor has no bounds, so a
/// read past the buffer's end is not the zero a bounded view gives.
pub(crate) fn root_srv_tables() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| root_fx() && crate::env_toggle!("TUXBOX_ZTRX_ROOT_SRV_DIRECT") != Some("1"))
}

pub(crate) fn root_fx() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_ZTRX_ROOT_FX") != Some("0"))
}

/// Where each binding of a ztrx root goes: stage k's user data parameter
/// (None: no parameter), and each root view as (stage, slot, kind, param),
/// kind 0 CBV, 1 SRV, 2 UAV. The root's creation and the draw's binding
/// both use it.
pub(crate) struct Layout {
    pub user: Vec<Option<u32>>,
    pub views: Vec<(usize, u8, u8, u32)>,
}

pub(crate) fn layout(stages: &[&ZtrxStage]) -> Layout {
    let mut user = Vec::new();
    let mut views = Vec::new();
    if root_fx() {
        let mut next = 0u32;
        for (k, s) in stages.iter().enumerate() {
            if s.user_data_dwords() > 0 {
                user.push(Some(next));
                next += 1;
            } else {
                user.push(None);
            }
            for (slots, kind) in [(&s.root_cbvs, 0u8), (&s.root_srvs, 1), (&s.root_uavs, 2)] {
                for &slot in slots.iter() {
                    views.push((k, slot, kind, next));
                    next += 1;
                }
            }
        }
    } else {
        user = (0..stages.len() as u32).map(Some).collect();
        let mut merged: Vec<(u8, u8)> = stages
            .iter()
            .flat_map(|s| {
                let cbvs = s.root_cbvs.iter().map(|&r| (r, 0));
                let srvs = s.root_srvs.iter().map(|&r| (r, 1));
                cbvs.chain(srvs).chain(s.root_uavs.iter().map(|&r| (r, 2)))
            })
            .collect();
        merged.sort_unstable();
        merged.dedup();
        for (i, (slot, kind)) in merged.into_iter().enumerate() {
            views.push((usize::MAX, slot, kind, (stages.len() + i) as u32));
        }
    }
    Layout { user, views }
}

/// The root signature for these stages, laid out by [`layout`].
pub(crate) fn root_signature_for(
    device: &ID3D12Device,
    stages: &[(&ZtrxStage, D3D12_SHADER_VISIBILITY)],
) -> windows::core::Result<ID3D12RootSignature> {
    if root_fx() {
        return root_signature_fx(device, stages);
    }
    let mut params: Vec<D3D12_ROOT_PARAMETER1> = stages
        .iter()
        .map(|&(s, vis)| D3D12_ROOT_PARAMETER1 {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: USER_DATA_SPACE,
                    Num32BitValues: s.user_data_dwords().max(1),
                },
            },
            ShaderVisibility: vis,
        })
        .collect();
    // By slot, then kind: 0 CBV, 1 SRV, 2 UAV.
    let mut views: Vec<(u8, u8)> = stages
        .iter()
        .flat_map(|(s, _)| {
            let cbvs = s.root_cbvs.iter().map(|&r| (r, 0));
            let srvs = s.root_srvs.iter().map(|&r| (r, 1));
            cbvs.chain(srvs).chain(s.root_uavs.iter().map(|&r| (r, 2)))
        })
        .collect();
    views.sort_unstable();
    views.dedup();
    for (slot, kind) in views {
        params.push(D3D12_ROOT_PARAMETER1 {
            ParameterType: match kind {
                1 => D3D12_ROOT_PARAMETER_TYPE_SRV,
                2 => D3D12_ROOT_PARAMETER_TYPE_UAV,
                _ => D3D12_ROOT_PARAMETER_TYPE_CBV,
            },
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR1 {
                    ShaderRegister: u32::from(slot),
                    RegisterSpace: ROOT_CBV_SPACE,
                    Flags: D3D12_ROOT_DESCRIPTOR_FLAG_NONE,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        });
    }
    root_signature_from(device, stages, &params)
}

fn root_signature_fx(
    device: &ID3D12Device,
    stages: &[(&ZtrxStage, D3D12_SHADER_VISIBILITY)],
) -> windows::core::Result<ID3D12RootSignature> {
    let only: Vec<&ZtrxStage> = stages.iter().map(|&(s, _)| s).collect();
    let layout = layout(&only);
    let mut params: Vec<(u32, D3D12_ROOT_PARAMETER1)> = Vec::new();
    for (k, &(s, vis)) in stages.iter().enumerate() {
        if let Some(param) = layout.user[k] {
            params.push((param, D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                Anonymous: D3D12_ROOT_PARAMETER1_0 {
                    Constants: D3D12_ROOT_CONSTANTS {
                        ShaderRegister: 0,
                        RegisterSpace: USER_DATA_SPACE,
                        Num32BitValues: s.user_data_dwords(),
                    },
                },
                ShaderVisibility: vis,
            }));
        }
    }
    let tables = root_srv_tables();
    // The one range of each tabled view (fx host.rs:428-470), kept alive
    // until the signature is serialized.
    let ranges: Vec<D3D12_DESCRIPTOR_RANGE1> = layout.views.iter().map(|&(_, slot, kind, _)| D3D12_DESCRIPTOR_RANGE1 {
        RangeType: if kind == 2 { D3D12_DESCRIPTOR_RANGE_TYPE_UAV } else { D3D12_DESCRIPTOR_RANGE_TYPE_SRV },
        NumDescriptors: 1,
        BaseShaderRegister: u32::from(slot),
        RegisterSpace: ROOT_CBV_SPACE,
        Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE | D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE,
        OffsetInDescriptorsFromTableStart: 0,
    }).collect();
    for (n, &(k, slot, kind, param)) in layout.views.iter().enumerate() {
        if tables && kind != 0 {
            params.push((param, D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER1_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE1 { NumDescriptorRanges: 1, pDescriptorRanges: &ranges[n] },
                },
                ShaderVisibility: stages[k].1,
            }));
            continue;
        }
        params.push((param, D3D12_ROOT_PARAMETER1 {
            ParameterType: match kind {
                1 => D3D12_ROOT_PARAMETER_TYPE_SRV,
                2 => D3D12_ROOT_PARAMETER_TYPE_UAV,
                _ => D3D12_ROOT_PARAMETER_TYPE_CBV,
            },
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR1 {
                    ShaderRegister: u32::from(slot),
                    RegisterSpace: ROOT_CBV_SPACE,
                    Flags: D3D12_ROOT_DESCRIPTOR_FLAG_DATA_VOLATILE,
                },
            },
            ShaderVisibility: stages[k].1,
        }));
    }
    params.sort_by_key(|p| p.0);
    let params: Vec<D3D12_ROOT_PARAMETER1> = params.into_iter().map(|p| p.1).collect();
    root_signature_from(device, stages, &params)
}

fn root_signature_from(
    device: &ID3D12Device,
    stages: &[(&ZtrxStage, D3D12_SHADER_VISIBILITY)],
    params: &[D3D12_ROOT_PARAMETER1],
) -> windows::core::Result<ID3D12RootSignature> {
    let mut samplers = Vec::new();
    for &(s, vis) in stages {
        for (n, d) in s.static_samplers.iter().enumerate() {
            let [u, v, w] = d.address.map(|a| D3D12_TEXTURE_ADDRESS_MODE(a as i32));
            samplers.push(D3D12_STATIC_SAMPLER_DESC {
                Filter: D3D12_FILTER(d.filter as i32),
                AddressU: u,
                AddressV: v,
                AddressW: w,
                MipLODBias: d.mip_lod_bias,
                MaxAnisotropy: d.max_anisotropy,
                ComparisonFunc: D3D12_COMPARISON_FUNC(d.comparison as i32),
                BorderColor: D3D12_STATIC_BORDER_COLOR(d.border as i32),
                MinLOD: d.min_lod,
                MaxLOD: d.max_lod,
                ShaderRegister: n as u32,
                RegisterSpace: SAMPLER_SPACE,
                ShaderVisibility: vis,
            });
        }
    }
    let desc = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
        Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
        Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 {
            Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
                NumParameters: params.len() as u32,
                pParameters: params.as_ptr(),
                NumStaticSamplers: samplers.len() as u32,
                pStaticSamplers: samplers.as_ptr(),
                Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT
                    | D3D12_ROOT_SIGNATURE_FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED
                    | D3D12_ROOT_SIGNATURE_FLAG_SAMPLER_HEAP_DIRECTLY_INDEXED,
            },
        },
    };
    // SAFETY: the description points at live arrays.
    unsafe {
        let mut blob: Option<ID3DBlob> = None;
        D3D12SerializeVersionedRootSignature(&desc, &mut blob, None)?;
        let blob = blob.ok_or(windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?;
        let bytes = std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
        device.CreateRootSignature(0, bytes)
    }
}

/// ztrx victory-shader native.rs RootType: what kind of title root
/// parameter fills a user-data dword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootType {
    Constants,
    /// A 64-bit buffer address.
    Descriptor,
    /// The 32-bit heap address of a descriptor table range.
    Table,
    /// A sampler the root signature fixes.
    ImmediateSampler,
    /// The 32-bit address of the CBV/SRV/UAV heap (SM 6.6 dynamic resources).
    ResourceHeap,
    /// The 32-bit address of the sampler heap.
    SamplerHeap,
    Other(u32),
}

/// ztrx victory-shader emit/mod.rs UserData: one dword of user data and the
/// root parameter that fills it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UserData {
    /// Dword in the user-data buffer (the console's user-data index).
    pub index: u32,
    /// Root parameter index in the title's root signature.
    pub slot: u8,
    /// `Constants` or `Table`.
    pub root: RootType,
    /// Dword within the parameter.
    pub dword: u32,
}

/// The root parameter as the title bound it (ztrx draw.rs Root).
pub(crate) enum TitleRoot<'a> {
    /// A descriptor table: its byte offset from the start of the bound heap
    /// (descriptors are 32 bytes, so index × 32).
    Table(u32),
    Constants(&'a [u32]),
}

/// The user-data root constants of one stage, as ztrx victory-gpu draw.rs
/// (the `stage.user_data` loop at :922 and user_value at :644, GPL-3.0,
/// credit to its authors) fills them: a table dword is the table's offset in
/// the title heap, a constants dword the title's value. `host_word(kind,
/// slot, dword)` answers the host-computed kinds (RootType::Other); a root
/// the title did not set reads 0.
pub(crate) fn user_values<'a>(
    dwords: u32,
    user_data: &[UserData],
    root: impl Fn(u8) -> Option<TitleRoot<'a>>,
    host_word: impl Fn(u32, u8, u32) -> u32,
) -> Vec<u32> {
    let mut values = vec![0u32; dwords as usize];
    for u in user_data {
        let Some(slot) = values.get_mut(u.index as usize) else { continue };
        if let RootType::Other(kind) = u.root {
            *slot = host_word(kind, u.slot, u.dword);
            continue;
        }
        *slot = match (root(u.slot), u.root) {
            (Some(TitleRoot::Table(offset)), RootType::Table) => offset,
            (Some(TitleRoot::Constants(c)), RootType::Constants) => c.get(u.dword as usize).copied().unwrap_or(0),
            _ => {
                static ONCE: std::sync::Once = std::sync::Once::new();
                crate::diagnostics::log_once(&ONCE, || format!(
                    "TB_ZTRX_SHADER: a shader reads root argument {} ({:?}) the title did not set", u.slot, u.root));
                0
            }
        };
    }
    values
}

/// ztrx victory-shader resource.rs RootIndexedView.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RootIndexedView {
    pub table_slot: u8,
    pub constant_slot: u8,
    pub constant_dword: u32,
    pub byte_offset: u32,
    pub shift: u8,
}

impl StaticSampler {
    /// Decodes the four sampler words. Copied from ztrx victory-shader
    /// emit/sampler.rs StaticSampler::from_words (GPL-3.0, credit to its authors).
    pub(crate) fn from_words(w: [u32; 4]) -> Self {
        let bits = |word: u32, at: u32, n: u32| word >> at & ((1 << n) - 1);
        // wrap, mirror, clamp, mirror once; the half-border modes have no
        // D3D form and become their nearest.
        let address = |m: u32| [1, 2, 3, 5, 3, 5, 4, 5][m as usize];
        let (mag, min, mip) = (bits(w[2], 20, 2), bits(w[2], 22, 2), bits(w[2], 26, 2));
        let reduction = match bits(w[0], 29, 2) {
            1 => 2,
            2 => 3,
            _ => 0,
        };
        let filter = if mag >= 2 || min >= 2 {
            0x55 | reduction << 7
        } else {
            (min & 1) << 4 | (mag & 1) << 2 | u32::from(mip == 2) | reduction << 7
        };
        let lod = |v: u32| v as f32 / 256.0;
        // Without a mip filter the base level is sampled.
        let (min_lod, max_lod) = if mip == 0 {
            (0.0, 0.0)
        } else {
            (lod(bits(w[1], 0, 12)), lod(bits(w[1], 12, 12)))
        };
        StaticSampler {
            filter,
            address: [0, 3, 6].map(|at| address(bits(w[0], at, 3))),
            mip_lod_bias: ((bits(w[2], 0, 14) << 18) as i32 >> 18) as f32 / 256.0,
            max_anisotropy: 1 << bits(w[0], 9, 3).min(4),
            comparison: bits(w[0], 12, 3) + 1,
            // shortcut: a custom border colour (type 3, a table entry) becomes
            // transparent black; upgrade when a title uses one.
            border: bits(w[3], 30, 2) % 3,
            min_lod,
            max_lod,
        }
    }
}

/// The `<key>.ztrx` sidecar the launcher (src/ztrx.rs, TB_ZTRX_SHADER)
/// publishes before `<key>.cso`: HostShader's binding fields, one per line:
/// `stage`, `user_data_dwords n`, `user_data <index> <slot> <RootType:?>
/// <dword>`, `root_cbv|root_srv|root_uav <param>`, `sampler w0 w1 w2 w3`
/// (s<n> in order), `root_indexed_view table_slot constant_slot
/// constant_dword byte_offset shift`. None when a known line is malformed.
pub(crate) fn parse_sidecar(text: &str) -> Option<ZtrxStage> {
    let mut stage = ZtrxStage::default();
    let mut decoded = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let w: Vec<&str> = line.split_whitespace().collect();
        let n = |k: usize| w.get(k).and_then(|v| v.parse::<u32>().ok());
        let b = |k: usize| n(k).and_then(|v| u8::try_from(v).ok());
        let f = |k: usize| w.get(k).and_then(|v| v.parse::<f32>().ok());
        match w[0] {
            // D3D12 values as the launcher decoded them; preferred over the words.
            "static_sampler" => decoded.push(StaticSampler {
                filter: n(1)?,
                address: [n(2)?, n(3)?, n(4)?],
                mip_lod_bias: f(5)?,
                max_anisotropy: n(6)?,
                comparison: n(7)?,
                border: n(8)?,
                min_lod: f(9)?,
                max_lod: f(10)?,
            }),
            "user_data" => {
                let root = match *w.get(3)? {
                    "Constants" => RootType::Constants,
                    "Descriptor" => RootType::Descriptor,
                    "Table" => RootType::Table,
                    "ImmediateSampler" => RootType::ImmediateSampler,
                    "ResourceHeap" => RootType::ResourceHeap,
                    "SamplerHeap" => RootType::SamplerHeap,
                    o => RootType::Other(o.strip_prefix("Other(")?.strip_suffix(')')?.parse().ok()?),
                };
                stage.user_data.push(UserData { index: n(1)?, slot: b(2)?, root, dword: n(4)? });
            }
            "user_data_dwords" => stage.declared_dwords = n(1),
            // The launcher's shipped form (src/ztrx.rs "ztrx 1"): one-letter
            // root kinds and D3D12-valued samplers. prewarm188 read these as
            // sampler words with no user data, and the driver refused 887
            // pipelines whose root did not match their DXIL.
            "user" => {
                let r = *w.get(3)?;
                let root = match r {
                    "c" => RootType::Constants,
                    "d" => RootType::Descriptor,
                    "t" => RootType::Table,
                    "i" => RootType::ImmediateSampler,
                    "r" => RootType::ResourceHeap,
                    "s" => RootType::SamplerHeap,
                    o => RootType::Other(o.strip_prefix('o')?.parse().ok()?),
                };
                stage.user_data.push(UserData { index: n(1)?, slot: b(2)?, root, dword: n(4)? });
            }
            "sampler" if w.len() == 11 => decoded.push(StaticSampler {
                filter: n(1)?,
                address: [n(2)?, n(3)?, n(4)?],
                mip_lod_bias: f(5)?,
                max_anisotropy: n(6)?,
                comparison: n(7)?,
                border: n(8)?,
                min_lod: f(9)?,
                max_lod: f(10)?,
            }),
            "sampler" if w.len() == 5 => stage.static_samplers.push(StaticSampler::from_words([n(1)?, n(2)?, n(3)?, n(4)?])),
            "sampler" => return None,
            "root_cbv" | "cbv" => stage.root_cbvs.push(b(1)?),
            "root_srv" | "srv" => stage.root_srvs.push(b(1)?),
            "root_uav" | "uav" => stage.root_uavs.push(b(1)?),
            "root_indexed_view" | "view" => stage.root_indexed_views.push(RootIndexedView {
                table_slot: b(1)?,
                constant_slot: b(2)?,
                constant_dword: n(3)?,
                byte_offset: n(4)?,
                shift: b(5)?,
            }),
            _ => {}
        }
    }
    if !decoded.is_empty() {
        stage.static_samplers = decoded;
    }
    Some(stage)
}

fn sidecars() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<ZtrxStage>>> {
    static MAP: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<ZtrxStage>>>> = std::sync::OnceLock::new();
    MAP.get_or_init(Default::default)
}

/// TUXBOX_ZTRX_BIND (default on, =0 binds ztrx translations as ours): records
/// the sidecar of the `.cso` at `path`, keyed by the translation's digest.
pub(crate) fn note(path: &std::path::Path, code: &[u8]) {
    if crate::env_toggle!("TUXBOX_ZTRX_BIND") == Some("0") {
        return;
    }
    let Some(digest) = super::shader_cache::source_digest(code) else { return };
    // A session answer (`run-*/<key>.cso`) has no sidecar beside it: the
    // launcher writes `<key>.ztrx` into the compiler cache only, so prewarm189
    // bound those ztrx programs on the runtime tables and the driver refused
    // them. The cache's sidecar is taken when its `<key>.cso` is these bytes
    // (its .sha256 names the same digest). TUXBOX_ZTRX_CACHE_SIDECAR=0 off.
    let stage = sidecar_for(path).or_else(|| {
        if crate::env_toggle!("TUXBOX_ZTRX_CACHE_SIDECAR") == Some("0") {
            return None;
        }
        let twin = super::shader_cache::runtime_dir()?.join(path.file_name()?);
        if twin == path {
            return None;
        }
        let named = std::fs::read_to_string(twin.with_extension("sha256")).ok()?;
        (named.trim() == digest).then(|| sidecar_for(&twin)).flatten()
    });
    {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n <= 8 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!("ztrx note #{n}: {} {}, runtime dir {:?}",
                path.display(), if stage.is_some() { "has a sidecar" } else { "no sidecar here or in the cache" },
                super::shader_cache::runtime_dir()));
        }
    }
    let Some(stage) = stage else { return };
    sidecars().lock().unwrap_or_else(|e| e.into_inner()).insert(digest, std::sync::Arc::new(stage));
}

/// The sidecar of translated DXIL `code`, when it is a ztrx translation.
pub(crate) fn for_code(code: &[u8]) -> Option<std::sync::Arc<ZtrxStage>> {
    let map = sidecars().lock().unwrap_or_else(|e| e.into_inner());
    let digest = super::shader_cache::source_digest(code)?;
    let found = map.get(&digest).cloned();
    if found.is_none() {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n <= 8 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!("ztrx for_code miss #{n}: {digest} ({} bytes) not among {} noted sidecars", code.len(), map.len()));
        }
    }
    found
}

/// The sidecar of a translated shader cached at `cso` (`<digest>.ztrx`
/// beside it), when there is one: such a pipeline is a ztrx one.
pub(crate) fn sidecar_for(cso: &std::path::Path) -> Option<ZtrxStage> {
    parse_sidecar(&std::fs::read_to_string(cso.with_extension("ztrx")).ok()?)
}
