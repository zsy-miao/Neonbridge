// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil;

/// <summary>Source the DXIL backend cannot compile: a syntax error or an HLSL feature it does not lower.</summary>
public sealed class DxilBackendException : Exception
{
    public DxilBackendException(string message)
        : base(message)
    {
    }
}
