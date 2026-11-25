// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Cache;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Hardware;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The key a shader's translation is found by: the 24-byte hash prefix of its code block, which the FSSM program in a
/// DXBC container and the pipeline cache's code block share (docs/pipeline-cache.md, Shaders).
/// </summary>
internal static class ProgramKeys
{
    public static byte[] Prefix(ShaderSource source)
    {
        if (source.Kind == ShaderSourceKind.CacheCode)
        {
            return source.Bytes[..Math.Min(source.Bytes.Length, ProgramIndex.PrefixSize)];
        }

        var program = ShaderArchiveExtractor.ReadProgram(source.Bytes);
        var codeOffset = FssmHeader.Decode(program).CodeOffset;

        if (codeOffset is null || codeOffset.Value + ProgramIndex.PrefixSize > program.Length)
        {
            throw new InvalidDataException("the container's FSSM program has no code block");
        }

        return program.AsSpan((int)codeOffset.Value, ProgramIndex.PrefixSize).ToArray();
    }

    /// <summary>The key as lower-case hex, for messages.</summary>
    public static string Text(ReadOnlySpan<byte> prefix)
    {
        return Convert.ToHexStringLower(prefix);
    }
}
