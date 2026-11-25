// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// One HLSL source to compile: the file it was written to (the DXIL goes next to it as <c>&lt;stem&gt;.dxil</c>, its folder
/// is the include path), its text, the profile prefix (ps, vs, cs, ms, gs) and whether optimizations are off (-Od).
/// </summary>
public sealed record CompileRequest(string HlslPath, string Hlsl, string Profile, bool DisableOptimizations)
{
    public string DxilPath => Path.ChangeExtension(HlslPath, ".dxil");
}
