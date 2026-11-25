use super::*;

fn psv_with_wave_range(runtime_len: usize, min: u32, max: u32) -> Vec<u8> {
    let mut psv = (runtime_len as u32).to_le_bytes().to_vec();
    let mut runtime = vec![0u8; runtime_len];
    if runtime_len >= 24 {
        runtime[16..20].copy_from_slice(&min.to_le_bytes());
        runtime[20..24].copy_from_slice(&max.to_le_bytes());
    }
    psv.extend_from_slice(&runtime);
    psv
}

#[test]
fn extracts_non_trivial_wave_lane_bounds_from_psv0() {
    assert_eq!(wave_lane_range(&psv_with_wave_range(52, 0, u32::MAX)), None);
    assert_eq!(wave_lane_range(&psv_with_wave_range(24, 0, 0)), None);
    assert_eq!(wave_lane_range(&psv_with_wave_range(24, 64, 64)), Some((64, 64)));
    assert_eq!(wave_lane_range(&psv_with_wave_range(52, 32, 32)), Some((32, 32)));
    assert_eq!(wave_lane_range(&psv_with_wave_range(52, 32, 64)), Some((32, 64)));
    assert_eq!(wave_lane_range(&psv_with_wave_range(52, 0, 64)), Some((0, 64)));
    assert_eq!(wave_lane_range(&psv_with_wave_range(52, 64, 32)), None);
    assert_eq!(wave_lane_range(&psv_with_wave_range(20, 64, 64)), None);
    assert_eq!(wave_lane_range(&psv_with_wave_range(24, 64, 64)[..20]), None);
}
use crate::container::tests::container_with;

fn dwords(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|w| w.to_le_bytes()).collect()
}

#[test]
fn extracts_compute_threads_from_psv0_and_shex() {
    let mut runtime = [0u8; 48];
    runtime[24] = 5;
    runtime[36..40].copy_from_slice(&8u32.to_le_bytes());
    runtime[40..44].copy_from_slice(&4u32.to_le_bytes());
    runtime[44..48].copy_from_slice(&2u32.to_le_bytes());
    let mut psv = 48u32.to_le_bytes().to_vec();
    psv.extend_from_slice(&runtime);
    let psv_dxbc = container_with(&[(b"PSV0", &psv)]);
    assert_eq!(shader_compute_threads(&psv_dxbc), Some([8, 4, 2]));

    // SM 5.0 compute shader with preceding customdata + dcl_thread_group(7, 3, 2).
    let shex = dwords(&[
        0x0005_0050,
        10,
        11,
        4,
        0xdead_beef,
        0xcafe_babe,
        (4 << 24) | 155,
        7,
        3,
        2,
    ]);
    let shex_dxbc = container_with(&[(b"SHEX", &shex)]);
    assert_eq!(shader_compute_threads(&shex_dxbc), Some([7, 3, 2]));

    // Non-compute stage or zero thread dimensions fail closed.
    let mut ps_shex = shex.clone();
    ps_shex[0..4].copy_from_slice(&0x0000_0050u32.to_le_bytes());
    assert_eq!(shader_compute_threads(&container_with(&[(b"SHEX", &ps_shex)])), None);
    let mut zero_shex = shex;
    zero_shex[28..32].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(shader_compute_threads(&container_with(&[(b"SHEX", &zero_shex)])), None);
    assert_eq!(shader_compute_threads(b"not dxbc"), None);
}

#[cfg(feature = "native-bindings")]
#[test]
fn falls_back_to_xdat_fssm_preamble_when_psv0_lacks_threads() {
    use miniz_oxide::deflate::compress_to_vec_zlib;

    let mut image = vec![0u8; 0x170];
    let write_u64 = |buf: &mut [u8], at: usize, v: u64| buf[at..at + 8].copy_from_slice(&v.to_le_bytes());
    write_u64(&mut image, 0, 6);
    for slot in 0..12 {
        write_u64(&mut image, 8 + 8 * slot, if slot == 11 { 0x80 } else { 0xdead_beef_dead_beef });
    }
    write_u64(&mut image, 0x80, 0x170);
    write_u64(&mut image, 0x88, 4);
    image[0x160..0x170].copy_from_slice(&dwords(&[0, 64, 2, 1]));
    image.extend_from_slice(&0xbf81_0000u32.to_le_bytes());
    let mut fssm = Vec::from(*b"FSSM");
    fssm.extend_from_slice(&0x101u32.to_le_bytes());
    fssm.extend_from_slice(&(image.len() as u64).to_le_bytes());
    fssm.extend_from_slice(&image);
    let compressed = compress_to_vec_zlib(&fssm, 6);
    let mut section = Vec::from(*b"XBCH");
    section.extend_from_slice(&3u32.to_le_bytes());
    section.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
    section.extend_from_slice(&(fssm.len() as u64).to_le_bytes());
    section.extend_from_slice(&compressed);

    let mut xdat = Vec::from(*b"XDAT");
    xdat.extend_from_slice(&dwords(&[2, 36, 13, 60, section.len() as u32, 7, 60 + section.len().next_multiple_of(4) as u32, 4]));
    xdat.extend_from_slice(&section);
    xdat.resize(xdat.len().next_multiple_of(4), 0);
    xdat.extend_from_slice(b"NOBC");

    let mut dxil = dwords(&[0x50060, ((24 + xdat.len()) / 4) as u32]);
    dxil.extend_from_slice(b"DXIL");
    dxil.extend_from_slice(&dwords(&[0x100, (16 + xdat.len() - 4) as u32, 4]));
    dxil.extend_from_slice(&xdat);

    let dxbc = container_with(&[(b"DXIL", &dxil)]);
    assert_eq!(shader_compute_threads(&dxbc), Some([64, 2, 1]));

    // Non-compute stage ignores FSSM preamble.
    dxil[0..4].copy_from_slice(&0x0000_0060u32.to_le_bytes());
    assert_eq!(shader_compute_threads(&container_with(&[(b"DXIL", &dxil)])), None);
}
