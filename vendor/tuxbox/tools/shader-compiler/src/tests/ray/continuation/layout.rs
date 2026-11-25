use super::*;

#[test]
fn partial_stack_pointer_add_is_not_a_full_frame_adjustment() {
    let delta = |ext: u32| {
        let bytes: Vec<_> = [0x4a0e_0ef9, ext].into_iter().flat_map(u32::to_le_bytes).collect();
        frame_adjustment(&instructions(&bytes).collect::<Vec<_>>(), 0)
    };
    // v7 += 32 with DWORD and WORD_0 destinations respectively.
    assert_eq!(delta(0x0686_16a0), Some(32));
    assert_eq!(delta(0x0686_14a0), None);
}

fn symbols(names: &[(&str, u32)]) -> Vec<u8> {
    let mut bytes = vec![0; 0x40 + names.len() * 24];
    bytes[0x38..0x3c].copy_from_slice(&(names.len() as u32).to_le_bytes());
    for (i, &(name, pc)) in names.iter().enumerate() {
        let record = 0x40 + i * 24;
        let at = (bytes.len() - 0x40) as u64;
        bytes[record..record + 8].copy_from_slice(&at.to_le_bytes());
        bytes[record + 8..record + 12].copy_from_slice(&pc.to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
    }
    bytes
}

#[test]
fn exported_substates_follow_symbol_order_and_require_the_known_scheduler() {
    let program = vec![0; 0x300 + 872];
    let names = [
        ("Shadow.ss_0", 0x200),
        ("Shadow.ss_1", 0x100),
        ("XDXR_TraceRay.ss_0", 0),
        ("main", 0x300),
    ];
    let good = symbols(&names);
    let mut state = State {
        traversal: [&[]; 3],
        program: &program,
        root: &[],
        symbols: &good,
        name: "Shadow".into(),
        kind: 0,
        hit_group: None,
        any_hit: None,
    };
    assert_eq!(symbol_offsets(&state), Some((vec![0x200, 0x100], 0x300)));
    assert!(matches!(Layout::analyze(&state), Err(e) if e.contains("scheduler")));

    let mut unaligned = names;
    unaligned[0].1 = 0x204;
    let unaligned = symbols(&unaligned);
    state.symbols = &unaligned;
    assert!(symbol_offsets(&state).is_none());
    state.symbols = &good;
    state.name = "Other".into();
    assert!(symbol_offsets(&state).is_none());

    // A second entry in a hit group must implement its declared any-hit
    // export. A similarly positioned continuation of the closest hit cannot
    // stand in for a candidate callback.
    let pair = symbols(&[
        ("Candidate.ss_0", 0x200),
        ("Material.ss_0", 0x100),
        ("XDXR_TraceRay.ss_0", 0),
        ("main", 0x300),
    ]);
    state.name = "Material".into();
    state.kind = 2;
    state.hit_group = Some("Group".into());
    state.any_hit = Some("Candidate".into());
    state.symbols = &pair;
    assert_eq!(symbol_offsets(&state), Some((vec![0x200, 0x100], 0x300)));
    state.any_hit = Some("WrongCandidate".into());
    assert!(symbol_offsets(&state).is_none());
}

fn put(program: &mut [u8], at: usize, words: &[u32]) {
    for (word, dst) in words.iter().zip(program[at..].chunks_exact_mut(4)) {
        dst.copy_from_slice(&word.to_le_bytes());
    }
}

#[test]
fn stack_frame_and_getpc_relocations_determine_the_resume_graph() {
    let mut program = 0xbf80_0000u32.to_le_bytes().repeat(0x300 / 4); // s_nop
    const RETURN: u32 = 0xbe80_201e; // s_setpc_b64 s[30:31]
    const PUSH: u32 = 0x4e42_0eb3; // v33 = v7 - 51
    const POP: u32 = 0x4a0e_0eb3; // v7 += 51

    // s_getpc s[54:55], then add the signed displacement to the PC pair.
    let to = |delta: i32| {
        [
            0xbeb6_1f00,
            0x8036_ff36,
            delta as u32,
            0x8237_ff37,
            u32::MAX,
        ]
    };
    put(&mut program, 0x200, &[PUSH]);
    put(&mut program, 0x204, &to(0x100 - 0x208));
    put(&mut program, 0x218, &[RETURN]);
    put(&mut program, 0x100, &[POP, RETURN]);
    let starts = &[0x200, 0x100];
    let analyzed = Layout::substates(&program, starts, 0x300).unwrap();
    assert_eq!(analyzed.entries, [0x200..0x21c, 0x100..0x108]);
    assert_eq!(analyzed.frame, 51);
    assert!(analyzed.forward);
    put(&mut program, 0x104, &to(0x100 - 0x108));
    put(&mut program, 0x118, &[RETURN]);
    assert!(!Layout::substates(&program, starts, 0x300).unwrap().forward);
    put(&mut program, 0x104, &to(0x104 - 0x108));
    assert!(matches!(Layout::substates(&program, starts, 0x300),
        Err(e) if e.contains("unknown ray continuation")));
    put(&mut program, 0x104, &to(0x100 - 0x108));
    put(&mut program, 0x100, &[POP - 1]);
    assert!(matches!(Layout::substates(&program, starts, 0x300),
        Err(e) if e.contains("stack frame")));
}

#[test]
fn stack_temporary_must_dominate_adjustment_and_cold_tail_must_rejoin_code() {
    let mut program = 0xbf80_0000u32.to_le_bytes().repeat(0x300 / 4);
    const RETURN: u32 = 0xbe80_201e;
    const POP: u32 = 0x4a0e_0ea6; // v7 += 38
    const COPY: u32 = 0x7e36_0307; // v27 = v7
    const PUSH: u32 = 0x4e0e_36a6; // v7 = v27 - 38
    put(&mut program, 0x200, &[COPY, PUSH, RETURN]);
    // The first conditional reaches the tail at 0x10c. It rejoins the
    // shared return via a backward branch.
    put(
        &mut program,
        0x100,
        &[0xbf84_0002, POP, RETURN, 0xbf82_fffd],
    );
    let starts = &[0x200, 0x100];
    let layout = Layout::substates(&program, starts, 0x300).unwrap();
    assert_eq!(layout.frame, 38);
    assert_eq!(layout.entries[1], 0x100..0x110);
    put(&mut program, 0x20c, &[0xbf82_fffd]); // bypass copy and enter at 0x204
    assert!(Layout::substates(&program, starts, 0x300).is_err());
    put(&mut program, 0x20c, &[0xbf80_0000]);
    put(&mut program, 0x10c, &[0xbf82_ffff]); // self-loop instead of rejoining
    assert!(Layout::substates(&program, starts, 0x300).is_err());
}

#[test]
fn a_frameless_export_may_read_only_the_callers_return_word() {
    let mut program = 0xbf80_0000u32.to_le_bytes().repeat(0x200 / 4);
    let words = [
        0xbf80_0000, // s_nop
        3611688968,
        73991431, // v8 = (v7 << 2) + v26
        3761246208,
        67110920,    // v8 = scratch[v8], byte offset 0
        0xbe80_201e, // s_setpc_b64 s[30:31]
    ];
    put(&mut program, 0x100, &words);
    let layout = Layout::substates(&program, &[0x100], 0x200).unwrap();
    assert_eq!(layout.frame, 0);
    assert!(layout.forward);
    put(&mut program, 0x100, &[0xbf82_0002]); // bypass the address calculation
    assert!(Layout::substates(&program, &[0x100], 0x200).is_err());
    put(&mut program, 0x100, &words);
    put(&mut program, 0x10c, &[words[3] | 4]); // scratch byte offset 4
    assert!(Layout::substates(&program, &[0x100], 0x200).is_err());
}
