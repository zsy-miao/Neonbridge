# AcpHal provider

`AcpHal.dll` exposes the seven exports Frosty imports. The Rust provider backs the APU heap with host virtual memory, assigns 32-bit heap-relative APU addresses, and allocates the XMA context array used by this title. `AcpHalCreate` returns an inert 32-slot interface because the host has no Xbox SHAPE DSP. `AcpHalReleaseShapeContexts` is an HRESULT-returning, no-argument cleanup entry point in the reference DLL; this provider has no DSP device state and returns success.

The Frosty call site sets the XMA context count to `0x200`, then uses the array pointer at record offset `0x38` for `0x8000` bytes. This matches 512 contexts at 64 bytes each. Other SHAPE context kinds return `E_NOTIMPL`. The title imports seven of ten exports; ordinals 8–10 remain unimplemented. APU memory reclamation and DSP/XMA decoding remain incomplete.

Build with `python build.py acphal`. The export table preserves the package ordinals. Source compilation alone does not count as behavioral coverage; update the library map only after a guest or retail trace verifies the behavior.
