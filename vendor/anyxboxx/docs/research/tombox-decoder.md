> Français : [tombox-decodeur.md](../fr/recherche/tombox-decodeur.md)

# TomBox RDNA 2 decoder checked against the LLVM tests

Report produced by `tools/compare_tombox.py` (Rust bridge `rust/anyxboxx-rdna2`) on the unique encodings of LLVM's public tests (gfx10, gfx1030), with `tombox-xshader` at commit `fef9a6e` of omonkeyz/xbox-emulator. "Unknown to TomBox": opcode missing from their mnemonic table (shown as `family<0x…>`). The name differences `v_mad_legacy`/`v_mac_legacy` and `v_mac_f16` come from the gfx1010 names (RDNA 1); LLVM gfx1030 (RDNA 2) uses the `fma`/`fmac` names. The FLAT/GLOBAL/SCRATCH differences come from the segment (bits 15-14), which is not part of the name.

| Family | Encodings | Identical | Wrong length | Different name | Unknown to TomBox |
|---|---:|---:|---:|---:|---:|
| Ds | 1865 | 1118 | 0 | 0 | 747 |
| Exp | 24 | 24 | 0 | 0 | 0 |
| Flat | 153 | 14 | 0 | 26 | 113 |
| Mimg | 219 | 140 | 0 | 0 | 79 |
| Mubuf | 781 | 543 | 0 | 0 | 238 |
| Smem | 443 | 192 | 0 | 0 | 251 |
| Sop1 | 1057 | 751 | 0 | 0 | 306 |
| Sop2 | 1656 | 1656 | 0 | 0 | 0 |
| Sopc | 342 | 342 | 0 | 0 | 0 |
| Sopk | 206 | 185 | 0 | 0 | 21 |
| Sopp | 112 | 112 | 0 | 0 | 0 |
| Vop1 | 3932 | 3923 | 0 | 0 | 9 |
| Vop2 | 2716 | 2678 | 0 | 38 | 0 |
| Vop3 | 11604 | 9893 | 0 | 41 | 1670 |
| Vopc | 4412 | 3260 | 0 | 0 | 1152 |
| **Total** | 29522 | 24831 | 0 | 105 | 4586 |

Examples of differences:

- Ds / unknown : 00 00 54 d8 00 01 00 00 : LLVM ds_add_f32, TomBox 2 mot(s) ds<0x15>
- Ds / unknown : 00 00 56 d8 00 01 00 00 : LLVM ds_add_f32, TomBox 2 mot(s) ds<0x15>
- Ds / unknown : 34 12 54 d8 00 01 00 00 : LLVM ds_add_f32, TomBox 2 mot(s) ds<0x15>
- Flat / unknown : 00 80 d0 dc 00 02 02 00 : LLVM global_atomic_csub, TomBox 2 mot(s) flat<0x34>
- Flat / unknown : 00 80 d1 dc 00 02 02 02 : LLVM global_atomic_csub, TomBox 2 mot(s) flat<0x34>
- Flat / unknown : 64 80 d3 dc 00 02 02 02 : LLVM global_atomic_csub, TomBox 2 mot(s) flat<0x34>
- Flat / name : 10 80 70 dc fe 01 02 00 : LLVM global_store_dword, TomBox 2 mot(s) flat_store_dword
- Flat / name : 00 64 30 dc 00 00 7f 00 : LLVM scratch_load_dword, TomBox 2 mot(s) flat_load_dword
- Flat / name : 00 64 24 dc 00 00 7f 00 : LLVM scratch_load_sbyte, TomBox 2 mot(s) flat_load_sbyte
- Mimg / unknown : 3b 01 00 f0 cc 0e 0a 00 0b 0e 13 00 : LLVM image_msaa_load, TomBox 3 mot(s) mimg<0x80>
- Mimg / unknown : 31 01 00 f0 01 05 02 80 : LLVM image_msaa_load, TomBox 2 mot(s) mimg<0x80>
- Mimg / unknown : 31 0f 00 f0 05 01 02 00 : LLVM image_msaa_load, TomBox 2 mot(s) mimg<0x80>
- Mubuf / unknown : ff 4f d0 e0 00 05 02 c1 : LLVM buffer_atomic_csub, TomBox 2 mot(s) mubuf<0x34>
- Mubuf / unknown : 00 00 d0 e0 00 05 02 03 : LLVM buffer_atomic_csub, TomBox 2 mot(s) mubuf<0x34>
- Mubuf / unknown : 00 40 d0 e0 00 05 02 03 : LLVM buffer_atomic_csub, TomBox 2 mot(s) mubuf<0x34>
- Smem / unknown : 41 01 08 f6 64 00 00 fa : LLVM s_atomic_add, TomBox 2 mot(s) smem<0x82>
- Smem / unknown : 41 41 08 f6 64 00 00 fa : LLVM s_atomic_add, TomBox 2 mot(s) smem<0x82>
- Smem / unknown : 41 01 09 f6 64 00 00 fa : LLVM s_atomic_add, TomBox 2 mot(s) smem<0x82>
- Sop1 / unknown : 01 44 ff be : LLVM s_andn1_saveexec_b32, TomBox 1 mot(s) sop1<0x44>
- Sop1 / unknown : 01 44 fe be : LLVM s_andn1_saveexec_b32, TomBox 1 mot(s) sop1<0x44>
- Sop1 / unknown : 01 44 fc be : LLVM s_andn1_saveexec_b32, TomBox 1 mot(s) sop1<0x44>
- Sopk / unknown : 34 12 7e bb : LLVM s_call_b64, TomBox 1 mot(s) sopk<0x16>
- Sopk / unknown : 34 12 00 bb : LLVM s_call_b64, TomBox 1 mot(s) sopk<0x16>
- Sopk / unknown : d1 c1 00 bb : LLVM s_call_b64, TomBox 1 mot(s) sopk<0x16>
- Vop1 / unknown : 01 91 0a 7e : LLVM v_movrelsd_2_b32, TomBox 1 mot(s) vop1<0x48>
- Vop1 / unknown : 01 91 fe 7f : LLVM v_movrelsd_2_b32, TomBox 1 mot(s) vop1<0x48>
- Vop1 / unknown : ff 91 0a 7e : LLVM v_movrelsd_2_b32, TomBox 1 mot(s) vop1<0x48>
- Vop2 / name : 01 05 00 0c : LLVM v_fmac_legacy_f32, TomBox 1 mot(s) v_mac_legacy_f32
- Vop2 / name : 01 05 0a 6c : LLVM v_fmac_f16, TomBox 1 mot(s) v_mac_f16
- Vop2 / name : 01 05 fe 6d : LLVM v_fmac_f16, TomBox 1 mot(s) v_mac_f16
- Vop3 / unknown : 05 00 c8 d5 01 01 01 02 : LLVM v_movrelsd_2_b32, TomBox 2 mot(s) vop3<0x1c8>
- Vop3 / unknown : ff 00 c8 d5 01 01 01 02 : LLVM v_movrelsd_2_b32, TomBox 2 mot(s) vop3<0x1c8>
- Vop3 / unknown : 05 00 c8 d5 ff 01 01 02 : LLVM v_movrelsd_2_b32, TomBox 2 mot(s) vop3<0x1c8>
- Vop3 / name : 00 00 40 d5 01 e8 0d 84 : LLVM v_fma_legacy_f32, TomBox 2 mot(s) v_mad_legacy_f32
- Vop3 / name : 00 00 40 d5 01 05 0e 04 : LLVM v_fma_legacy_f32, TomBox 2 mot(s) v_mad_legacy_f32
- Vop3 / name : 00 02 40 d5 01 05 0e 84 : LLVM v_fma_legacy_f32, TomBox 2 mot(s) v_mad_legacy_f32
- Vopc / unknown : 01 05 12 7d : LLVM v_cmp_lt_i16, TomBox 1 mot(s) vopc<0x89>
- Vopc / unknown : ff 05 12 7d : LLVM v_cmp_lt_i16, TomBox 1 mot(s) vopc<0x89>
- Vopc / unknown : 01 04 12 7d : LLVM v_cmp_lt_i16, TomBox 1 mot(s) vopc<0x89>
