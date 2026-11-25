// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>What the game handed over as a shader's bytecode.</summary>
internal enum ShaderSourceKind
{
    /// <summary>A DXBC container from the SGD5 tables of shaders.rpf, its FSSM program inside the XDAT wrapper.</summary>
    Container,

    /// <summary>The start of a code block of the pipeline cache (at least the 24-byte hash prefix).</summary>
    CacheCode,
}
