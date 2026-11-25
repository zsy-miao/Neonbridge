// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Shaders.Containers;

/// <summary>
/// Bare DXBC containers inside a module, without the archive's name records: XboxBvhBuilder.dll holds the 52 compute
/// programs its constructor makes its GPU builder from, each a game container whose XDAT section carries the native
/// FSSM program (docs/bvh-builder.md). A container is taken where the magic is followed by a total size that fits the
/// file and a header that parses.
/// </summary>
public static class EmbeddedContainers
{
    /// <summary>(offset, container bytes) of every container in <paramref name="module"/>, in file order.</summary>
    public static IReadOnlyList<(int Offset, ReadOnlyMemory<byte> Container)> Find(byte[] module)
    {
        var found = new List<(int, ReadOnlyMemory<byte>)>();
        var magic = "DXBC"u8;

        for (int at = module.AsSpan().IndexOf(magic); at >= 0;)
        {
            int next = at + magic.Length;

            if (at + DxbcContainer.HeaderSize <= module.Length)
            {
                uint size = BinaryPrimitives.ReadUInt32LittleEndian(module.AsSpan(at + DxbcContainer.TotalSizeOffset));

                if (size >= DxbcContainer.HeaderSize && size <= (uint)(module.Length - at) && Parses(module.AsMemory(at, (int)size)))
                {
                    found.Add((at, module.AsMemory(at, (int)size)));
                    next = at + (int)size;
                }
            }

            int rest = next < module.Length ? module.AsSpan(next).IndexOf(magic) : -1;
            at = rest < 0 ? -1 : next + rest;
        }

        return found;
    }

    private static bool Parses(ReadOnlyMemory<byte> container)
    {
        try
        {
            DxbcContainer.Parse(container);

            return true;
        }
        catch (InvalidDataException)
        {
            return false;
        }
    }
}
