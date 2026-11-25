# Build-time patches for the vendored trees

The vendored trees stay unchanged copies; the launcher applies these before a
build and reverses them after (default on, `NEONBRIDGE_PATCHES=0` off).

One patch per tree: each applies only to its own tree.

| Patch | Tree | What |
|-------|------|------|
| `rt-victory.patch` | `vendor/victory` | Ray tracing, below |
| `rt-vitoriabr.patch` | `vendor/vitoriabr` | Ray tracing, below |
| `rt-ztrx-victory.patch` | `vendor/ztrx-victory` | Ray tracing, below |
| `rt-scarlettport.patch` | `vendor/scarlettport/runtime/victory` | Ray tracing, below |

Ray tracing (the same change in every tree, hunks fitted to each):
- victory-gpu device slot 0x330 (GetRaytracingAccelerationStructurePrebuildInfo)
  answered by the host ID3D12Device5. Ported from TuxBox runtime/d3d12
  device/ray_build.rs (GPL-3.0).
- victory-gpu list slot 117 / 0x3a8 (BuildRaytracingAccelerationStructure)
  recorded on the host list (new `raytrace.rs`): title addresses translated
  through the title-buffer host copies, destinations in host AS buffers keyed
  by the title address, own scratch, TLAS instance BLAS addresses remapped at
  submission. Ported from TuxBox runtime/d3d12 commands/build_raytracing.rs
  (GPL-3.0).

Apply from the repo root (paths in the patches are relative to the tree root):

    git apply --ignore-whitespace --directory=vendor/<tree> vendor/patches/rt-<name>.patch

reverse with `-R`; check with `--check -v` and confirm each file is listed.
Do not run `git apply` with the tree as cwd: the trees live inside the 6ixlaunch
repo, so git resolves paths from the repo root and silently skips every file
("Skipped patch"), returning success.
