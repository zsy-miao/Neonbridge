//! `VICTORY_DESC_DATA`: a descriptor the code reads only as numbers (no image or
//! buffer instruction uses it) is refused by default and synthesized under the flag.

use super::lower;
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{ShaderError, io, ir, resource, structure};

/// `s_load_dwordx8 s[24:31]` from table slot 0, the output buffer's descriptor from
/// 0x20, then dwords 0 to 3 of the loaded descriptor stored to the buffer.
const CODE: [u32; 16] = [
    0xBE85_0380, // s_mov_b32 s5, 0
    0xF40C_0602,
    0xFA00_0000, // s_load_dwordx8 s[24:31], s[4:5], 0x0
    0xF408_0302,
    0xFA00_0020, // s_load_dwordx4 s[12:15], s[4:5], 0x20
    0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
    0x7E02_0219, // v_mov_b32 v1, s25
    0x7E04_021A, // v_mov_b32 v2, s26
    0x7E06_021B, // v_mov_b32 v3, s27
    0x7E08_0218, // v_mov_b32 v4, s24
    0x7E0A_0280, // v_mov_b32 v5, 0
    0xE078_1000,
    0x8003_0105, // buffer_store_dwordx4 v[1:4], v5, s[12:15], 0 offen
    0xBF81_0000, // s_endpgm
    0,
    0,
];

fn translate() -> Result<(), ShaderError> {
    let table = |kind, at, dwords| UsedResource {
        kind,
        root: RootType::Table,
        table_offset_dwords: at,
        lower_bound: 0,
        stride: 4,
        dwords,
        user_data: 4,
        slot: 1,
    };
    let param = |root, sgprs, user_data, slot| RootParam {
        root,
        table_offset_dwords: 0,
        sgprs,
        base_register: 0,
        user_data,
        slot,
    };
    let native = NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 8,
        layout: vec![param(RootType::Constants, 4, 0, 0), param(RootType::Table, 1, 4, 1)],
        used: vec![table(ResourceType::Srv, 0, 8), table(ResourceType::Uav, 8, 4)],
        io: StageIo::Cs(ComputeIo { rsrc2: 5 << 1, threads: [64, 1, 1] }),
        code: CODE[..14].iter().flat_map(|w| w.to_le_bytes()).collect(),
    };
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let st = structure::structure(&p).expect("structure");
    let res = resource::track(&p, &native.layout, native.stage, Some(&st))?;
    let io = io::raise(&p, &native).expect("io::raise");
    lower::lower(&lower::Parts {
        native: &native,
        p: &p,
        st: &st,
        io: &io,
        res: &res,
        inputs: Vec::new(),
        outputs: Vec::new(),
        prims: Vec::new(),
        mesh_stride: 0,
    })
    .map(|_| ())
}

#[test]
fn a_descriptor_read_only_as_data_translates_under_the_flag() {
    // One test switches the process-wide flag, so the two halves cannot race.
    // SAFETY: no other test reads this variable.
    unsafe { std::env::remove_var("VICTORY_DESC_DATA") };
    assert!(
        matches!(translate(), Err(ShaderError::Unsupported { what, .. }) if what == "a descriptor read only as data"),
        "refused without the flag"
    );
    unsafe { std::env::set_var("VICTORY_DESC_DATA", "1") };
    let result = translate();
    unsafe { std::env::remove_var("VICTORY_DESC_DATA") };
    result.expect("translates with the flag");
}
