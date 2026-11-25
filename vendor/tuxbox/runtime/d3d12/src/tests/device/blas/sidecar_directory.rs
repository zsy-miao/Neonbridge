use super::*;
fn leaf() -> Vec<u8> {
    let mut bytes = vec![0; 128];
    bytes[48..52].copy_from_slice(&0x80u32.to_le_bytes());
    bytes[56..60].copy_from_slice(&1u32.to_le_bytes());
    bytes[60..64].copy_from_slice(&0x0209u32.to_le_bytes());
    bytes[124..128].copy_from_slice(&9u32.to_le_bytes());
    bytes
}

#[test]
fn encodes_instance_indices_and_shared_leaf_sides() {
    let instances = [
        InstanceRange { first: 0, count: 2 },
        InstanceRange { first: 2, count: 0 },
        InstanceRange { first: 0, count: 2 },
    ];
    let candidates = [
        Candidate {
            leaf_offset: 0,
            side: 0,
        },
        Candidate {
            leaf_offset: 0,
            side: 1,
        },
    ];
    let out = encode(&leaf(), &instances, &candidates).unwrap();
    let words: Vec<u32> = out
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    assert_eq!(
        words,
        [
            1, 3, 2, 128, 0, 2, 0, 0, 2, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0
        ]
    );
}

#[test]
fn rejects_invalid_ranges_and_unverified_leaf_forms() {
    let good = Candidate {
        leaf_offset: 0,
        side: 0,
    };
    assert_eq!(
        encode(
            &leaf(),
            &[InstanceRange {
                first: u32::MAX,
                count: 2
            }],
            &[good]
        ),
        Err(Error::InvalidInstanceRange)
    );
    assert_eq!(
        encode(
            &leaf(),
            &[],
            &[Candidate {
                leaf_offset: 1,
                side: 0
            }]
        ),
        Err(Error::InvalidLeafRange)
    );
    assert_eq!(
        encode(
            &leaf(),
            &[],
            &[Candidate {
                leaf_offset: 128,
                side: 0
            }]
        ),
        Err(Error::InvalidLeafRange)
    );
    assert_eq!(
        encode(
            &leaf(),
            &[],
            &[Candidate {
                leaf_offset: 0,
                side: 2
            }]
        ),
        Err(Error::UnsupportedLeaf)
    );
    let mut unpaired = leaf();
    unpaired[48..52].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(
        encode(
            &unpaired,
            &[],
            &[Candidate {
                leaf_offset: 0,
                side: 1
            }]
        ),
        Err(Error::UnsupportedLeaf)
    );
}
