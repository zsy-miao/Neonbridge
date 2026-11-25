use super::*;
use crate::rdna2::disassemble;

fn native(a: u32, b: u32) -> Insn {
    let bytes: Vec<u8> = [a, b].iter().flat_map(|v| v.to_le_bytes()).collect();
    disassemble(&bytes).remove(0)
}

#[test]
fn ordinary_32_bit_forms_accept_vgprs_without_modifiers() {
    // LLVM gfx1030 words for `v7, v4, v5[, v6]`.
    for (first, second, name, want) in [
        (0xd77f_0007u32, 0x0202_0b04u32, "v_add_nc_i32", "(v4 + v5)"),
        (0xd776_0007, 0x0202_0b04, "v_sub_nc_i32", "(v4 - v5)"),
        (0xd773_0007, 0x041a_0b04, "v_mad_u32_u16", "((v4 & 0xffffu) * (v5 & 0xffffu) + v6)"),
        (0xd775_0007, 0x041a_0b04, "v_mad_i32_i16", "asint(v6)"),
    ] {
        let ins = native(first, second);
        assert_eq!(ins.mnemonic, name);
        assert!(expression(&ins).unwrap().unwrap().contains(want), "{name}");
        if name.starts_with("v_mad_") {
            assert!(expression(&native(first | 0x8000, second)).is_err(), "{name} clamp");
        }
        assert!(expression(&native(first | 0x100, second)).is_err(), "{name} abs");
        assert!(expression(&native(first, (second & !511) | 4)).is_err(), "{name} sgpr");
        assert!(expression(&native(first, second | 1 << 29)).is_err(), "{name} neg");
        // Destination or addend half selection has no meaning here.
        assert!(expression(&native(first | 0x4000, second)).is_err(), "{name} dst sel");
    }
    // op_sel:[1,1,0,0] selects the high multiplicand halves.
    let expr = expression(&native(0xd773_1807, 0x041a_0b04)).unwrap().unwrap();
    assert_eq!(expr, "((v4 >> 16) * (v5 >> 16) + v6)");
    assert!(expression(&native(0xd77f_0807, 0x0202_0b04)).is_err());
}

#[test]
fn unsigned_mad_accepts_a_full_width_inline_integer_accumulator() {
    // LLVM gfx1030: v_mad_u32_u16 v4, v1, v2, 7 (then -1).
    for (sources, addend) in [(0x021e_0501, "0x7u"), (0x0306_0501, "0xffffffffu")] {
        let expr = expression(&native(0xd773_0004, sources)).unwrap().unwrap();
        assert_eq!(expr, format!("((v1 & 0xffffu) * (v2 & 0xffffu) + {addend})"));
    }
    // v_mad_u32_u16 v4, 7, v2, v3 and v_mad_i32_i16 v4, v1, v2, 7
    assert!(expression(&native(0xd773_0004, 0x040e_0487)).is_err());
    assert!(expression(&native(0xd775_0004, 0x021e_0501)).is_err());
}

#[test]
fn integer_packs_clamp_each_word_before_combining_halves() {
    use crate::hlsl::float64::tests::translate;
    use crate::Stage;

    // LLVM gfx1030: v_cvt_pk_{u16_u32,i16_i32} v4, v1, v2
    for (opcode, low, high) in [
        (0xd76a_0004, "min(v1, 65535u)", "min(v2, 65535u)"),
        (0xd76b_0004, "(asuint(clamp(asint(v1), -32768, 32767)) & 0xffffu)",
                      "asuint(clamp(asint(v2), -32768, 32767))"),
    ] {
        let body = [opcode, 0x0002_0501];
        let hlsl = translate(Stage::Compute, &body).unwrap();
        assert!(hlsl.contains(&format!("({low} | ({high} << 16))")), "{hlsl}");
        assert!(translate(Stage::Compute, &[opcode | 0x0800, body[1]]).is_err());
        assert!(translate(Stage::Compute, &[opcode, body[1] | (1 << 29)]).is_err());
    }
}

#[test]
fn signed_add_and_subtract_accept_clamp() {
    use crate::hlsl::float64::tests::translate;
    use crate::Stage;

    // LLVM gfx1030: v_add_nc_i32 / v_sub_nc_i32 v4, v1, v2 clamp
    for (opcode, result) in [(0xd77f_8004, "(v1 + v2)"), (0xd776_8004, "(v1 - v2)")] {
        let hlsl = translate(Stage::Compute, &[opcode, 0x0002_0501]).unwrap();
        assert!(hlsl.contains(&format!("? (0x7fffffffu + (v1 >> 31)) : {result}")), "{hlsl}");
        assert!(translate(Stage::Compute, &[opcode | 0x0100, 0x0002_0501]).is_err());
    }
}

#[test]
fn signed_add_and_subtract_accept_immediate_operands() {
    use crate::hlsl::float64::tests::translate;
    use crate::Stage;

    // LLVM gfx1030: v_add_nc_i32 v7, v4, -1
    //              v_sub_nc_i32 v8, 0x80000000, v7
    let hlsl = translate(
        Stage::Compute,
        &[0xd77f_0007, 0x0001_8304, 0xd776_0008, 0x0002_0eff, 0x8000_0000],
    )
    .unwrap();
    assert!(hlsl.contains("v4 + 0xffffffffu"), "{hlsl}");
    assert!(hlsl.contains("0x80000000u - v7"), "{hlsl}");
}

#[test]
fn byte_sad_accepts_inline_zero_only_as_the_accumulator() {
    // LLVM gfx1030: v_sad_u8 v4, v1, v2, 0
    let expr = expression(&native(0xd55a_0004, 0x0202_0501)).unwrap().unwrap();
    assert_eq!(expr.matches("max(").count(), 4);
    assert!(expr.ends_with("+ 0x0u)"), "{expr}");

    for (opcode, sources) in [
        (0xd55a_0004, 0x0206_0501), // v_sad_u8 v4, v1, v2, 1
        (0xd55a_0004, 0x040e_0480), // v_sad_u8 v4, 0, v2, v3
        (0xd55b_0004, 0x0202_0501), // v_sad_hi_u8 v4, v1, v2, 0
    ] {
        assert!(expression(&native(opcode, sources)).is_err());
    }
}

#[test]
fn sums_of_absolute_differences_accumulate_into_source_2() {
    // LLVM gfx1030 words for `v7, v4, v5, v6`.
    for (first, name, fields) in [
        (0xd55a_0007u32, "v_sad_u8", 4), (0xd55b_0007, "v_sad_hi_u8", 4),
        (0xd55c_0007, "v_sad_u16", 2), (0xd55d_0007, "v_sad_u32", 1),
    ] {
        let ins = native(first, 0x041a_0b04);
        assert_eq!(ins.mnemonic, name);
        let expr = expression(&ins).unwrap().unwrap();
        assert_eq!(expr.matches("max(").count(), fields, "{name}: {expr}");
        assert!(expr.ends_with("+ v6)"), "{name}: {expr}");
        assert_eq!(name == "v_sad_hi_u8", expr.contains("<< 16"), "{name}");
        assert!(expression(&native(first | 0x8000, 0x041a_0b04)).is_err(), "{name} clamp");
        assert!(expression(&native(first | 0x0800, 0x041a_0b04)).is_err(), "{name} op_sel");
        assert!(expression(&native(first, 0x041a_0b04 & !511 | 4)).is_err(), "{name} sgpr");
    }
    // Masked and quad SADs remain refused.
    assert!(!handles(&native(0xd571_0007, 0x041a_0b04)));
}

#[test]
fn signed_64_bit_multiply_add_needs_a_null_carry() {
    use crate::hlsl::float64::tests::translate;
    use crate::Stage;
    // v_mad_i64_i32 v[10:11], null, v4, v5, v[8:9]
    let hlsl = translate(Stage::Compute, &[0xd577_7d0a, 0x0422_0b04]).unwrap();
    assert!(hlsl.contains("tb_mulhi_i32(v4, v5) + v9"), "{hlsl}");
    assert!(hlsl.contains("v10 = t") && hlsl.contains("v11 = t"), "{hlsl}");
    // A real carry destination (vcc_lo) is not modelled for the signed form.
    assert!(translate(Stage::Compute, &[0xd577_6a0a, 0x0422_0b04]).is_err());
    // The unsigned form now also accepts the null carry LLVM emits.
    let hlsl = translate(Stage::Compute, &[0xd576_7d0a, 0x0422_0b04]).unwrap();
    assert!(hlsl.contains("tb_mulhi_u32(v4, v5) + v9"), "{hlsl}");
}
