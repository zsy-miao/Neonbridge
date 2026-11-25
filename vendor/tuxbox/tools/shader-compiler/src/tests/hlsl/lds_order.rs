use super::*;
use crate::rdna2::disassemble;

#[test]
fn publish_masked_writes_before_and_after_a_read_loop() {
    let words = [
        0xd8380100, 0x00020100, // ds_write2_b32
        0xbefe040a,             // s_mov_b64 exec, s[10:11]
        0xbf8cc07f,             // s_waitcnt lgkmcnt(0)
        0xd8dc0100, 0,          // loop: ds_read2_b32
        0xbefe040a, 0xbf8cc07f, // a drain inside the loop cannot rendezvous
        0xbf84fffb,             // s_cbranch_scc0 loop
        0xd8380100, 0x00020100,
        0xbefe040a, 0xbf8cc07f,
        0xd8dc0100, 0,
    ];
    let bytes = words.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
    let insns = disassemble(&bytes);
    let sites = restored_write_waits(&insns, &[(3, 6)]);
    assert_eq!(sites.into_iter().collect::<Vec<_>>(), [2, 9]);
}

#[test]
fn order_lds_phases_at_convergent_draining_waits() {
    let write = [0xd8380100, 0x00020100]; // ds_write2_b32
    let read = [0xd8dc0100, 0x00000000]; // ds_read2_b32
    let drain = [0xbf8cc07f]; // lgkmcnt(0)
    let busy = [0xbf8cc17f]; // lgkmcnt(1)
    let branch = [0xbf880001]; // s_cbranch_execz
    let barrier = [0xbf8a0000];
    let atomic_max = [0xd8200000, 0x00000100];
    let nop = [0xbf800000]; // s_nop
    let sites_in = |ops: &[&[u32]], loops: &[(usize, usize)]| {
        let isa = ops
            .iter()
            .flat_map(|op| op.iter().flat_map(|w| w.to_le_bytes()))
            .collect::<Vec<_>>();
        exchange_waits(&disassemble(&isa), loops)
            .into_iter()
            .collect::<Vec<_>>()
    };
    let sites = |ops: &[&[u32]]| sites_in(ops, &[]);
    assert_eq!(
        sites_in(
            &[&write, &drain, &read, &read, &drain, &write, &write, &drain, &read],
            &[(3, 5)]
        ),
        [1, 7]
    );
    assert_eq!(
        sites(&[&write, &write, &drain, &read, &drain, &write, &drain, &branch, &read]),
        [2, 4, 6]
    );
    assert_eq!(sites(&[&branch, &nop, &write, &drain, &read]), [3]);
    assert_eq!(
        sites(&[&write, &drain, &atomic_max, &atomic_max, &drain, &read]),
        [1, 4]
    );
    assert!(sites(&[&write, &busy, &read]).is_empty());
    assert!(sites(&[&write, &branch, &drain, &read]).is_empty());
    assert!(sites(&[&write, &drain, &barrier, &read]).is_empty());
}
