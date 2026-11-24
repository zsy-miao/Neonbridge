//! Synthetic native pixel shader with direct texture and sampler heaps.
use tuxbox_xshader::{ShaderExtract, Stage, hlsl};
mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/support/wave_guard_fixture.rs")); }

fn main() {
    let variant = std::env::args().nth(1).expect("variant: copy, inplace, scalar");
    let (mut ex, _, _) = fixture::shader(32);
    ex.stage = Stage::Pixel;
    ex.shader_model = (6,6);
    ex.required_features = Some(3 << 25);
    ex.psv_threads = None;
    ex.resources = vec![tuxbox_xshader::psv::PsvResource {res_type:2,space:0,lower:500,upper:500}];
    ex.has_output_sig = true;
    ex.output_sig = vec![tuxbox_xshader::meta::SigElement {
        name:"SV_Target".into(),index:0,sysval:64,comp_type:3,register:0,mask:15,rw_mask:15
    }];
    let user = [[0u32,0,2,500,0x30a],[5,0,1,0,0x0e],[6,0,1,0,0x10]];
    let used = [[2u32,0,500,0,0x30a04]];
    let mut fssm = vec![0u8;0x110];
    fssm[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
    for (rows,field,count) in [(&user[..],0xe8,0xf8),(&used[..],0xf0,0xfc)] {
        let off = (fssm.len()-16) as u64;
        fssm[field..field+8].copy_from_slice(&off.to_le_bytes());
        fssm[count..count+4].copy_from_slice(&(rows.len() as u32).to_le_bytes());
        fssm.extend(rows.iter().flatten().copied().flat_map(u32::to_le_bytes));
    }
    let size = (fssm.len()-16) as u32;
    fssm[8..12].copy_from_slice(&size.to_le_bytes());
    ex.fssm=fssm;
    let mut words=vec![0x8f00850au32]; // s0 = texture index << 5
    match variant.as_str() {
        "scalar" => words.push(0x8f01840b),
        "copy" => words.extend([0x7e0a020b,0x340c0a84,0x7e0e0306,0x7e020507]),
        "inplace" => words.extend([0x7e0a020b,0x340c0a84,0x7e0c0306,0x7e020506]),
        _ => panic!("unknown variant"),
    }
    // Fetch both descriptors, sample at normalized (0.375,0.375), export RGBA.
    words.extend([0xf40c0107,0x00000000,0xf4080508,0x02000000,
                  0x7e0802ff,0x3ec00000,0x7e0a02ff,0x3ec00000,0xf0800f08,0x00a10704,
                  0xf800080f,0x0a090807,0xbf810000]);
    ex.isa=words.iter().flat_map(|w|w.to_le_bytes()).collect();
    ex.isa_data=ex.isa.clone();
    let t=hlsl::translate(&ex).expect("native heap pixel translation");
    assert_eq!(t.profile,"ps_6_6");
    print!("{}",t.hlsl);
}
