// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>The register class of a binding in a shader's metadata, numbered as in the tag 0x41 entries.</summary>
public enum BindingClass
{
    /// <summary>tN.</summary>
    ShaderResource = 0,

    /// <summary>sN.</summary>
    Sampler = 1,

    /// <summary>bN.</summary>
    ConstantBuffer = 2,

    /// <summary>uN.</summary>
    UnorderedAccess = 3,
}
