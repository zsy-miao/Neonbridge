# Finite dynamic CBV descriptor selection

The existing static descriptor and directly indexed heap paths are unchanged.
This bounded subset permits a compute s_load_dwordx4 from an unchanged
first root-table pair s[4:5] whose dynamic offset has at most four
possibly set bits, no low five bits and no encoded-selector overflow.
Enumerate every subset of the proven bitmask (at most 16 candidates).
Each possible slot must be in a finite, explicitly declared CBV root range
and have exactly one matching PSV resource. Only immediate scalar buffer
loads may consume the descriptor. Straight-line control, ordinary SGPRs,
no aliasing/escape or clobber of the descriptor words, no heap/packed/mesh
path. The conservative source-alias check may reject otherwise valid code.

Use the existing descriptor selection and guarded CBV loads rather than
turning guest descriptor words into host addresses. The runtime selector
is computed from the actual offset, not modulo or an assumed constant.

The native synthetic unit test checks the generated selector and rejects
missing reflection, missing root, misalignment, base high clobber,
descriptor escape, and a control-flow boundary.

Extract the HLSL between BEGIN_HLSL and END_HLSL using:

```sh
cargo test --target x86_64-unknown-linux-gnu --manifest-path tools/shader-compiler/Cargo.toml --lib bounded_dynamic_cbv -- --nocapture
```

Pass that HLSL and the normal tool/provider arguments to
`tools/check_dynamic_cbv.py --help`. The GPU fixture binds two distinct
real CBVs, injects a group index at the input and stores the final scalar.
It checks 128 selections and every word in a guarded 384-word buffer.
This is a translated HLSL control on desktop D3D12, not full guest native
pipeline preparation, arbitrary dynamic descriptor access or retail
coverage. Actual recovered refusal counts are unknown.

RTX 4080 / Proton Experimental: DXC/DXV and readback passed.

```json
{
  "schema": 1,
  "scope": "Synthetic dynamic HLSL selector, real CBV binding on GPU, injected gid input/output only",
  "passed": true,
  "adapter": "10de:2704",
  "guest_exit": 0,
  "result_words": 128,
  "compared_words": 384,
  "hlsl_sha256": "d84a4d15f0c1831630722648f45231f8e6978f5212b0951e9c59f3115e78b345",
  "dxil_sha256": "2ce2b05ebbcccc8f2df8cb0a8f9ed236a502a56bac6fdee06b31aa415ae545dd",
  "provider_sha256": "d5630a2fb1a0698a41cddeddea0d88dcb3de1c7db68be5124dd1d182090e9eaa",
  "guest_sha256": "7d414c923036059f24a9e1e61014dae8ecd1c5f558f3f145fcd67a5b4cb0520f",
  "dxc_sha256": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
  "dxv_sha256": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3"
}
```
