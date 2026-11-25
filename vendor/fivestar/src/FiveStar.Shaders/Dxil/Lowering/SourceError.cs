// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>Errors that name the source line.</summary>
public static class SourceError
{
    public static DxilBackendException At(SyntaxNode node, string message)
    {
        return new DxilBackendException($"line {node.Line}: {message}");
    }
}
