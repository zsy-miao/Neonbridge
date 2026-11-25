// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf;

/// <summary>The resource kind in descriptor <c>d1</c> bits 20-23; these four values cover every shipped texture.</summary>
public enum TextureKind
{
    /// <summary>A 2D texture.</summary>
    Texture2D = 2,

    /// <summary>A cube map: six faces, stored as complete copies of the cube (docs/textures.md).</summary>
    Cube = 4,

    /// <summary>A 3D texture; the array-size field holds the depth.</summary>
    Volume = 6,

    /// <summary>A 2D texture array, stored level by level.</summary>
    Array = 10,
}
