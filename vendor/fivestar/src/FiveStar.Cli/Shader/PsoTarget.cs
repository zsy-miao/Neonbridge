// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Shader;

/// <summary>
/// One pipeline state to create: the program, its kind (cs, cs_w32, vs, ps, ms, ms_w32, gs, msfb_cs, msfb_vs, or a
/// linked pair vs+ps, ms+ps, msfb_vs+ps) and its DXIL file; a pair also names the pixel shader's DXIL.
/// </summary>
internal sealed record PsoTarget(string Name, string Kind, string DxilPath)
{
    /// <summary>Gets the DXIL of the pixel shader of a linked pair.</summary>
    public string? ConsumerPath { get; init; }

    public string Key => $"{Name}|{Kind}";
}
