// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>A tag 0x42 entry of a shader's metadata: a resource the program declares in a descriptor table.</summary>
/// <param name="Used">Whether the low byte of the u32 at +8 is 1 (else 2): the resource is one of the container's PSV0 bindings.</param>
/// <param name="Class">Its register class (byte 1 of +8, numbered 0 SRV, 1 UAV, 2 CBV, 3 sampler).</param>
/// <param name="ResourceKind">The high 16 bits of +8: the PSV0 resource kind (2 Texture2D, 7 Texture2DArray, 10 typed buffer, 11 raw buffer, 12 structured buffer, 13 constant buffer, 16 acceleration structure).</param>
/// <param name="Register">The shader register (u32 at +12).</param>
/// <param name="Space">The register space (u32 at +16).</param>
public readonly record struct ResourceEntry(bool Used, BindingClass Class, int ResourceKind, int Register, int Space);
