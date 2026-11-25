use super::*;

mod fixture {
    include!("../../../examples/support/wave_guard_fixture.rs");
}

/// Build native masked reductions with optional pre-guard edges and an
/// independent tail EXECZ; width selects the verified 32/64-lane fixture.
fn guarded_shader(width: u32, prefix: &[u32]) -> ShaderExtract {
    let (mut shader, mut words, end) = fixture::shader(width);
    words.truncate(end);
    let skip = words.len() as u32;
    words.splice(..0, prefix.iter().copied().chain([0xbe87_0391, 0xbf88_0000 | skip]));
    words.extend([0xbf88_0001, 0xbf80_0000, 0xbf81_0000]);
    shader.isa = words.into_iter().flat_map(u32::to_le_bytes).collect();
    shader.isa_data = shader.isa.clone();
    shader
}

#[test]
fn reduction_execz_votes_only_at_proven_guards() {
    for width in [32, 64] {
        let translated = translate(&guarded_shader(width, &[])).expect("guarded reduction");
        assert_eq!(translated.required_wave_size, Some(width));
        assert_eq!(translated.hlsl.matches("!WaveActiveAnyTrue((exec_lo & 1u) != 0u)").count(), 1);
        assert!(translated.hlsl.contains("if ((exec_lo & 1u) == 0u)"));
    }
}

#[test]
fn reduction_execz_accepts_direct_rejoin_at_vote() {
    for width in [32, 64] {
        // Skip one local vector instruction and rejoin exactly at the guard.
        let translated = translate(&guarded_shader(width, &[0xbf88_0002, 0x4a00_0081]))
            .expect("direct rejoin before wave vote");
        assert_eq!(translated.hlsl.matches("!WaveActiveAnyTrue((exec_lo & 1u) != 0u)").count(), 1);
    }
}

#[test]
fn reduction_execz_behind_a_bypass_branches_with_the_whole_wave() {
    for width in [32, 64] {
        let (mut shader, mut words, end) = fixture::shader(width);
        words.truncate(end);
        let skip = words.len() as u32 + 1;
        // The first edge bypasses the vote but lands before the prologue.
        words.splice(..0, [0xbf88_0002, 0xbe87_0391, 0xbf88_0000 | skip, 0xbf80_0000]);
        words.push(0xbf81_0000);
        shader.isa = words.into_iter().flat_map(u32::to_le_bytes).collect();
        shader.isa_data = shader.isa.clone();
        // One targeted vote cannot cover this shape, so every branch votes.
        let translated = translate(&shader).expect("whole-wave branches");
        assert_eq!(translated.hlsl.matches("!WaveActiveAnyTrue((exec_lo & 1u) != 0u)").count(), 2);
        assert!(!translated.hlsl.contains("if ((exec_lo & 1u) == 0u)"));
    }
}

#[test]
fn dispatched_lanes_wait_for_their_wave_at_a_barrier() {
    let op = |offset, op, mnemonic: &str, imm| Insn {
        offset, dwords: 1, enc: Enc::Sopp, op, raw: vec![0xbf80_0000 | op << 16], mnemonic: mnemonic.into(),
        dst: Vec::new(), src: Vec::new(), aux: tuxbox_dxbc::rdna2::InsnAux { imm, ..Default::default() },
    };
    let emit = |barrier: bool| {
        let shader = guarded_shader(32, &[]);
        // A lane leaves the loop at its first EXECZ and may reach the barrier before the others.
        let mut emitter = Emitter::new(&shader, vec![
            op(0, 8, "s_cbranch_execz", 1),
            op(4, 2, "s_branch", -2),
            if barrier { op(8, 0x0a, "s_barrier", 0) } else { op(8, 0, "s_nop", 0) },
            op(12, 1, "s_endpgm", 0),
        ]);
        emitter.emit_body_dispatch(false, 0).unwrap();
        emitter.body
    };
    let body = emit(true);
    assert!(body.contains("bool tb_wait = tb_pc == 2u;"), "{body}");
    let block = body.find("if (tb_sel == 2u) {").expect("the barrier starts a block");
    let wait = body.find("WaveActiveAnyTrue(!tb_wait)").expect("waits for its wave");
    let sync = body.find("GroupMemoryBarrierWithGroupSync();").unwrap();
    assert!(wait < block && block < sync, "{body}");
    assert!(body.contains("uint tb_sel = tb_wait ? 0xfffffffeu : tb_pc;"), "{body}");
    let body = emit(false);
    assert!(!body.contains("tb_wait") && body.contains("uint tb_sel = tb_pc;"), "{body}");
}

#[test]
fn reduction_execz_refuses_block_dispatch() {
    let shader = guarded_shader(32, &[]);
    let mut emitter = Emitter::new(&shader, Vec::new());
    emitter.reduction_execz_guards.insert(0);
    assert!(emitter.emit_body_dispatch(false, 0).unwrap_err().contains("EXECZ vote"));
}
