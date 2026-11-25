//! Native gfx1030 DPP8 control with a real reflected UAV store.
use tuxbox_xshader::{ShaderExtract, Stage, hlsl};
mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/support/wave_guard_fixture.rs")); }
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let width: u32 = args[1].parse().unwrap();
    let op = match args[2].as_str() {
        "add" => 0x4a, "and" => 0x36, "or" => 0x38, "min" => 0x26, "max" => 0x28,
        "smin" => 0x22, "smax" => 0x24, "xor" => 0x3a,
        _ => panic!("operation"),
    };
    let selectors = u32::from_str_radix(&args[3],16).unwrap();
    assert!(selectors <= 0xffffff);
    let lane = args[4].parse::<i32>().unwrap();
    assert!(lane == -1 || width == 32 && (0..32).contains(&lane));
    let bias = u32::from_str_radix(&args[5],16).unwrap();
    let rhs = u32::from_str_radix(&args[6],16).unwrap();
    let fi = args.get(7).map(|s| s.parse::<u32>().unwrap()).unwrap_or(0);
    assert!(fi <= 1);
    let (mut ex, _, _) = fixture::shader(width);
    // s_load_dwordx4 s[16:19],s[4:5],0; XOR lane with bias;
    // materialize independent second operand; DPP8 v4,v2,v1.
    let mut words = vec![0xf4080402,0xfa000000,0xbf8cc07f,
                         0x3a0400ff,bias,0x7e0202ff,rhs,
                         (op<<24)|(0x000802e9+fi),(selectors<<8)|2];
    if lane >= 0 {
        // v_readlane s7,v4,lane; v_mov v4,s7.
        words.extend([0xd7600007,((128+lane as u32)<<9)|260,0x7e080207]);
    }
    // v_lshlrev v5,2,v0; buffer_store_dword v4,v5,s[16:19],0 offen.
    words.extend([0x340a0082,0xe0701000,0x80040405,0xbf810000]);
    ex.isa = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    ex.isa_data = ex.isa.clone();
    ex.has_root_sig = true;
    ex.resources.push(tuxbox_xshader::psv::PsvResource {res_type:7,space:0,lower:0,upper:0});
    ex.root_sig = Some(tuxbox_xshader::meta::RootSig {
        version:1, flags:0, num_static_samplers:0, static_samplers:Vec::new(),
        params:vec![tuxbox_xshader::meta::RootParam {visibility:0,
            kind:tuxbox_xshader::meta::RootParamKind::Table(vec![
                tuxbox_xshader::meta::RootRange {range_type:1,num:1,base_register:0,space:0,offset_in_table:0}
            ])}]
    });
    let t = hlsl::translate(&ex).expect("translate native DPP8 and store");
    assert_eq!(t.required_wave_size,None);
    assert!(t.hlsl.contains("tb_reduction_read(gtid.x"));
    print!("{}",t.hlsl);
}
