// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Security.Cryptography;
using System.Text;

namespace FiveStar.Shaders.Compilation;

/// <summary>SHA-256 cache keys as lowercase hex. Each part is preceded by its length, so different part lists never collide.</summary>
public static class ContentHash
{
    public static string Of(params ReadOnlySpan<byte[]> parts)
    {
        using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);

        foreach (var part in parts)
        {
            hash.AppendData(BitConverter.GetBytes((long)part.Length));
            hash.AppendData(part);
        }

        return Convert.ToHexStringLower(hash.GetHashAndReset());
    }

    public static string OfText(params ReadOnlySpan<string> parts)
    {
        var bytes = new byte[parts.Length][];

        for (var index = 0; index < parts.Length; index++)
        {
            bytes[index] = Encoding.UTF8.GetBytes(parts[index]);
        }

        return Of(bytes);
    }

    /// <summary>The hash of the files' contents; a missing file counts as its path (a tool found on PATH).</summary>
    public static string OfFiles(IEnumerable<string> paths)
    {
        var parts = new List<byte[]>();

        foreach (var path in paths)
        {
            if (File.Exists(path))
            {
                parts.Add(File.ReadAllBytes(path));
            }
            else
            {
                parts.Add(Encoding.UTF8.GetBytes(path));
            }
        }

        return Of([.. parts]);
    }
}
