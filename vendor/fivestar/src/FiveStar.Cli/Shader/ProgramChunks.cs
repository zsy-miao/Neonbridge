// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;

namespace FiveStar.Cli.Shader;

/// <summary>The PSV0 chunk of a compiled program (a DXBC container as dxc writes it).</summary>
internal static class ProgramChunks
{
    public static ReadOnlyMemory<byte> Psv(byte[] program)
    {
        var chunk = DxbcContainer.Parse(program).Find(PsvSignatureParser.ChunkCode) ?? throw new InvalidDataException("no PSV0 chunk");

        return chunk.Data;
    }
}
