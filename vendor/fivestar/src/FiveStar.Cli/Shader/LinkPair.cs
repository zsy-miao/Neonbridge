// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Programs;

namespace FiveStar.Cli.Shader;

/// <summary>A producer (vertex or mesh program) and the pixel shader a console pipeline runs it with.</summary>
internal sealed record LinkPair(ProgramVariants Producer, ProgramVariants Consumer)
{
    public string Name => $"{Producer.Name}+{Consumer.Name}";
}
