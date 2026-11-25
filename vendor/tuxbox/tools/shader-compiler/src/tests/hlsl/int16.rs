use super::*;
use crate::rdna2::disassemble;

#[test]
fn signed_half_add_and_subtract_preserve_the_untouched_half_with_immediates() {
    use crate::hlsl::float64::tests::translate;
    use crate::Stage;

    // LLVM gfx1030: v_add_nc_i16 v7, v4, -1
    //              v_sub_nc_i16 v8, 0x1234, v5 op_sel:[0,1,1]
    let hlsl = translate(
        Stage::Compute,
        &[0xd70d_0007, 0x0001_8304, 0xd70e_5008, 0x0002_0aff, 0x1234],
    )
    .unwrap();
    assert!(
        hlsl.contains("(v7 & 0xffff0000u) | (((v4 & 0xffffu) + (0xffffffffu & 0xffffu)) & 0xffffu)"),
        "{hlsl}"
    );
    assert!(
        hlsl.contains("(v8 & 0xffffu) | (((0x1234u & 0xffffu) - (v5 >> 16)) << 16)"),
        "{hlsl}"
    );
    // v_add_nc_i16 v7, v4, -1 op_sel:[0,1,0]
    assert!(translate(Stage::Compute, &[0xd70d_1007, 0x0001_8304]).is_err());
}

#[test]
fn inline_half_multiply_preserves_the_high_half() {
    use crate::hlsl::float64::tests::translate;
    use crate::Stage;

    // LLVM gfx1030: v_mul_lo_u16 v4, v0, 7
    //               v_mul_lo_u16 v4, -1, v0
    for (sources, product) in [
        (0x0001_0f00, "(v0 & 0xffffu) * (0x7u & 0xffffu)"),
        (0x0002_00c1, "(0xffffffffu & 0xffffu) * (v0 & 0xffffu)"),
    ] {
        let hlsl = translate(Stage::Compute, &[0xd705_0004, sources]).unwrap();
        assert!(hlsl.contains(&format!("(v4 & 0xffff0000u) | (({product}) & 0xffffu)")), "{hlsl}");
    }

    // The inline case does not cover op_sel or a trailing literal word.
    assert!(translate(Stage::Compute, &[0xd705_1004, 0x0001_0f00]).is_err());
    assert!(translate(Stage::Compute, &[0xd705_0004, 0x0001_ff00, 0x1234]).is_err());
}

#[test]
fn ordinary_half_forms_preserve_the_other_half() {
    for (first, mnemonic) in [
        (0xd705_4804u32, "v_mul_lo_u16"),
        (0xd709_4804u32, "v_max_u16"),
        (0xd70b_4804u32, "v_min_u16"),
    ] {
        let decode = |a: u32, b: u32| {
            let bytes: Vec<u8> = [a,b].iter().flat_map(|v| v.to_le_bytes()).collect();
            disassemble(&bytes).remove(0)
        };
        let ins=decode(first,0x0202_0b04);
        assert_eq!(ins.mnemonic,mnemonic);
        assert!(new_half(&ins));
        let expr=expression(&ins).unwrap().unwrap();
        assert!(expr.contains("v4 & 0xffffu"));
        let expr=expression(&decode(first & !0x4000,0x0202_0b04)).unwrap().unwrap();
        assert!(expr.contains("v4 & 0xffff0000u"));
        assert!(expression(&decode(first | 0x8000,0x0202_0b04)).is_err());
        assert!(expression(&decode(first,0x0202_0a04)).is_err());
    }
}

#[test]
fn new_16_bit_forms_select_halves_and_preserve_the_other_half() {
    let decode = |a: u32, b: u32| {
        let bytes: Vec<u8> = [a, b].iter().flat_map(|v| v.to_le_bytes()).collect();
        disassemble(&bytes).remove(0)
    };
    // LLVM gfx1030 words for `v7, v4, v5[, v6]` without op_sel.
    for (first, second, name) in [
        (0xd70d_0007u32, 0x0202_0b04u32, "v_add_nc_i16"),
        (0xd70e_0007, 0x0202_0b04, "v_sub_nc_i16"),
        (0xd740_0007, 0x041a_0b04, "v_mad_u16"),
        (0xd75e_0007, 0x041a_0b04, "v_mad_i16"),
        (0xd752_0007, 0x041a_0b04, "v_min3_i16"),
        (0xd753_0007, 0x041a_0b04, "v_min3_u16"),
        (0xd755_0007, 0x041a_0b04, "v_max3_i16"),
        (0xd756_0007, 0x041a_0b04, "v_max3_u16"),
        (0xd758_0007, 0x041a_0b04, "v_med3_i16"),
        (0xd759_0007, 0x041a_0b04, "v_med3_u16"),
    ] {
        let ins = decode(first, second);
        assert_eq!(ins.mnemonic, name);
        assert!(new_half(&ins), "{name}");
        let low = expression(&ins).unwrap().unwrap();
        assert!(low.contains("v7 & 0xffff0000u") && low.contains("(v4 & 0xffffu)"), "{name}: {low}");
        // op_sel source 0 high and destination high.
        let high = expression(&decode(first | 0x4800, second)).unwrap().unwrap();
        assert!(high.contains("v7 & 0xffffu") && high.contains("(v4 >> 16)"), "{name}: {high}");
        if name.ends_with("_i16") && !name.contains("_nc_") && !name.starts_with("v_mad") {
            assert!(low.contains("asint("), "{name} compares signed");
        }
        assert!(expression(&decode(first | 0x8000, second)).is_err(), "{name} clamp");
        assert!(expression(&decode(first, (second & !511) | 4)).is_err(), "{name} sgpr");
        assert!(expression(&decode(first | 0x100, second)).is_err(), "{name} abs");
    }
    // Source 2 selection for three-operand forms.
    let med = expression(&decode(0xd759_2007, 0x041a_0b04)).unwrap().unwrap();
    assert!(med.contains("(v6 >> 16)"));
}

#[test]
fn new_packed_integer_forms_accept_ordinary_sources_only() {
    // LLVM gfx1030 native words. The default high selectors include
    // source 2 via dword0 bit 14 for the three-operand forms.
    for (first, second, name) in [
        (0xcc02_4004, 0x1a02_0b04, "v_pk_add_i16"),
        (0xcc00_2804, 0x141a_0b04, "v_pk_mad_i16"),
        (0xcc09_4004, 0x1c1a_0b04, "v_pk_mad_u16"),
        (0xcc0c_4804, 0x1202_0b04, "v_pk_max_u16"),
        (0xcc0d_4004, 0x1a02_0b04, "v_pk_min_u16"),
    ] {
        let native = |a: u32, b: u32| {
            let bytes: Vec<u8> = [a,b].iter().flat_map(|v| v.to_le_bytes()).collect();
            disassemble(&bytes).remove(0)
        };
        let ins = native(first,second);
        assert_eq!(ins.mnemonic, name);
        assert!(expression(&ins).unwrap().is_some());
        if matches!(name, "v_pk_max_u16" | "v_pk_min_u16") {
            assert!(expression(&native(first | 0x8000, second)).is_err());
        } else {
            let clamped = expression(&native(first | 0x8000, second)).unwrap().unwrap();
            if name.ends_with("_i16") {
                assert!(clamped.contains("0x7fff8000u, 0x80007fffu"));
                assert!(clamped.contains("^ 0x80000000u"));
            }
        }
        assert!(expression(&native(first | 0x8000, (second & !511) | 4)).is_err());
        assert!(expression(&native(first, (second & !511) | 4)).is_err());
        assert!(expression(&native(first | 0x100, second)).is_err());
    }
    let bytes: Vec<u8> = [0xcc09_4004u32,0x1c1a_0b04].iter().flat_map(|v| v.to_le_bytes()).collect();
    let expr = expression(&disassemble(&bytes)[0]).unwrap().unwrap();
    assert!(expr.contains("(v6 & 0xffffu)"));
    assert!(expr.contains("(v6 >> 16)"));
}
