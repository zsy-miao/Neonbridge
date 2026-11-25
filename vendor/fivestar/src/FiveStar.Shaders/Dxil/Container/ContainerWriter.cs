// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>A DXBC container: header, part offsets, parts (fourcc, size, bytes), and the digest D3D12 checks.</summary>
public static class ContainerWriter
{
    private const int DigestOffset = 4;
    private const int HashedFrom = 20;
    private const int SizeOffset = 24;
    private const int PartCountOffset = 28;
    private const int HeaderBytes = 32;
    private const int PartHeaderBytes = 8;
    private const ushort MajorVersion = 1;

    public static byte[] Build(IReadOnlyList<(string FourCc, byte[] Data)> parts)
    {
        var table = HeaderBytes + (sizeof(uint) * parts.Count);
        var total = table + parts.Sum(part => PartHeaderBytes + part.Data.Length);
        var bytes = new byte[total];
        "DXBC"u8.CopyTo(bytes);
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(HashedFrom), MajorVersion);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(SizeOffset), (uint)total);

        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(PartCountOffset), (uint)parts.Count);
        var offset = table;

        for (var index = 0; index < parts.Count; index++)
        {
            var (fourCc, data) = parts[index];
            BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(HeaderBytes + (index * sizeof(uint))), (uint)offset);
            Encoding.ASCII.GetBytes(fourCc).CopyTo(bytes, offset);
            BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(offset + sizeof(uint)), (uint)data.Length);
            data.CopyTo(bytes, offset + PartHeaderBytes);
            offset += PartHeaderBytes + data.Length;
        }

        ContainerHash.Compute(bytes.AsSpan(HashedFrom)).CopyTo(bytes, DigestOffset);

        return bytes;
    }
}
