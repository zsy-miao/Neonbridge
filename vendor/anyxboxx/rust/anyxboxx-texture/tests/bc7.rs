//! Vérifications reprises de `tests/bc7_blocks_check.py` de Xbox2Windows (R0Y00, licence MIT) :
//! décalages des champs, blocs calculés à la main, réciprocité pack/unpack et empreintes du jeu
//! de 512 blocs. Les deux empreintes SHA-256 sont celles de leur dépôt : le portage Rust doit les
//! reproduire à l'identique.
use anyxboxx_texture::bc7::{self, Fields, MODES};
use anyxboxx_texture::{decode_surface, tiles};
use sha2::{Digest, Sha256};

const ROWS_SHA256: &str = "184fc0970dc8e1b91f79639d820bd0eb7df2659427bb8b17ce66f61fd7e969ba";
const PIXELS_SHA256: &str = "79fe13df3cf1a31914c7a08d0e02d78840347d5e2dcbaa51b35c44afc7210383";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn block(fields: &[(u128, u32)]) -> Vec<u8> {
    fields.iter().map(|&(value, offset)| value << offset).sum::<u128>().to_le_bytes().to_vec()
}

fn texels(data: &[u8]) -> Vec<[u8; 4]> {
    data.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect()
}

fn random_block(mode: usize, case: usize) -> Vec<u8> {
    let digest = Sha256::digest(format!("random {mode} {case}").as_bytes());
    let bits = u128::from_le_bytes(digest[..16].try_into().unwrap());
    (bits >> (mode + 1) << (mode + 1) | 1 << mode).to_le_bytes().to_vec()
}

fn fixture_fields(mode: usize, case: usize) -> Fields {
    let partition = case % (1 << MODES[mode].partition_bits);
    let mut fields = Fields::new();
    fields.insert("mode".into(), mode as u32);
    for (name, _, width) in bc7::layout(mode, partition).unwrap().into_iter().skip(1) {
        let digest = Sha256::digest(format!("bc7 fixture {mode} {case} {name}").as_bytes());
        fields.insert(name, digest[0] as u32 & ((1 << width) - 1));
    }
    for (name, value) in [("partition", partition as u32), ("rotation", case as u32 & 3),
                          ("index_selection", case as u32 >> 2 & 1)] {
        if let Some(slot) = fields.get_mut(name) {
            *slot = value;
        }
    }
    fields
}

#[test]
fn field_offsets_match_the_published_layouts() {
    let expected: [&[(&str, u32)]; 8] = [
        &[("partition", 1), ("r0", 5), ("g0", 29), ("b0", 53), ("p0", 77), ("index0", 83)],
        &[("partition", 2), ("r0", 8), ("g0", 32), ("b0", 56), ("p0", 80), ("index0", 82)],
        &[("partition", 3), ("r0", 9), ("g0", 39), ("b0", 69), ("index0", 99)],
        &[("partition", 4), ("r0", 10), ("g0", 38), ("b0", 66), ("p0", 94), ("index0", 98)],
        &[("rotation", 5), ("index_selection", 7), ("r0", 8), ("g0", 18), ("b0", 28), ("a0", 38), ("index0", 50), ("index2_0", 81)],
        &[("rotation", 6), ("r0", 8), ("g0", 22), ("b0", 36), ("a0", 50), ("index0", 66), ("index2_0", 97)],
        &[("r0", 7), ("g0", 21), ("b0", 35), ("a0", 49), ("p0", 63), ("index0", 65)],
        &[("partition", 8), ("r0", 14), ("g0", 34), ("b0", 54), ("a0", 74), ("p0", 94), ("index0", 98)],
    ];
    for (mode, offsets) in expected.iter().enumerate() {
        for partition in 0..1 << MODES[mode].partition_bits {
            let spans = bc7::layout(mode, partition).unwrap();
            assert_eq!(spans.iter().map(|(_, _, w)| w).sum::<u32>(), 128);
            for (name, offset) in offsets.iter() {
                let found = spans.iter().find(|(n, _, _)| n == name).unwrap();
                assert_eq!(found.1, *offset, "mode {mode} partition {partition} champ {name}");
            }
        }
    }
}

#[test]
fn hand_derived_blocks() {
    // Mode 1, partition 13 : lignes 0-1 au sous-ensemble 0, lignes 2-3 au sous-ensemble 1.
    let data = block(&[(1, 1), (13, 2), (63, 8), (63, 26), (1, 80), (7, 105)]);
    let mut expected = vec![[255, 2, 2, 255]; 8];
    expected.push([253, 0, 0, 255]);
    expected.extend(vec![[0, 0, 0, 255]; 7]);
    assert_eq!(texels(&bc7::decode(&data).unwrap()), expected);
    // Mode 5 : rouge de 255 à 0, alpha de 64 à 192 ; la rotation 1 échange rouge et alpha.
    let fields = [(1, 5), (127, 8), (64, 50), (192, 58), (3, 67), (1, 100)];
    assert_eq!(texels(&bc7::decode(&block(&fields)).unwrap())[..4],
               [[255, 0, 0, 64], [0, 0, 0, 64], [255, 0, 0, 106], [255, 0, 0, 64]]);
    let rotated = [&fields[..], &[(1, 6)]].concat();
    assert_eq!(texels(&bc7::decode(&block(&rotated)).unwrap())[..4],
               [[64, 0, 0, 255], [64, 0, 0, 0], [106, 0, 0, 255], [64, 0, 0, 255]]);
    // Mode 4 : la sélection d'index échange les index de couleur et d'alpha.
    let fields = [(1, 4), (31, 8), (63, 44), (1, 51), (7, 83)];
    assert_eq!(texels(&bc7::decode(&block(&fields)).unwrap())[..2], [[255, 0, 0, 0], [171, 0, 0, 255]]);
    let selected = [&fields[..], &[(1, 7)]].concat();
    assert_eq!(texels(&bc7::decode(&block(&selected)).unwrap())[..2], [[255, 0, 0, 0], [0, 0, 0, 84]]);
    assert_eq!(bc7::decode(&[0; 16]).unwrap(), [0; 64]);
}

#[test]
fn pack_inverts_unpack() {
    for mode in 0..8 {
        for case in 0..512 {
            let data = random_block(mode, case);
            let fields = bc7::unpack(&data).unwrap();
            assert_eq!(fields["mode"] as usize, mode);
            assert_eq!(bc7::pack(&fields).unwrap().to_vec(), data);
        }
    }
    assert!(bc7::unpack(&[0; 16]).is_err() && bc7::unpack(&[1; 15]).is_err());
}

#[test]
fn fixture_reproduces_xbox2windows_digests() {
    let (columns, rows) = (16, 32);
    let blocks: Vec<u8> = (0..columns * rows)
        .flat_map(|index| bc7::pack(&fixture_fields(index % 8, index / 8)).unwrap())
        .collect();
    assert_eq!(hex(&Sha256::digest(&blocks)), ROWS_SHA256);
    let pixels = decode_surface(&blocks, columns * 4, rows * 4).unwrap();
    assert_eq!(hex(&Sha256::digest(&pixels)), PIXELS_SHA256);
}

#[test]
fn sw_4k_s_detiling_follows_the_bit_order() {
    // Une tuile dont le bloc n porte la valeur n : la position (x, y) lit le bloc y0 y1 x0 x1 y2 x2 y3 x3.
    let tile: Vec<u8> = (0..256u32).flat_map(|n| [n.to_le_bytes(), [0; 4], [0; 4], [0; 4]].concat()).collect();
    let linear = tiles::linearize_4k_s(&tile, 64, 64).unwrap();
    let at = |x: usize, y: usize| u32::from_le_bytes(linear[(y * 16 + x) * 16..][..4].try_into().unwrap());
    assert_eq!((at(0, 0), at(1, 0), at(0, 1), at(4, 0), at(0, 4), at(15, 15)), (0, 4, 1, 32, 16, 255));
    assert!(tiles::linearize_4k_s(&tile[..4000], 64, 64).is_err());
    let tail = tiles::linearize_4k_s_tail(&tile, 4, 4, 7).unwrap(); // niveau 7 en (0, 0)
    assert_eq!(u32::from_le_bytes(tail[..4].try_into().unwrap()), 0);
    assert!(tiles::linearize_4k_s_tail(&tile, 64, 64, 0).is_err());
}
