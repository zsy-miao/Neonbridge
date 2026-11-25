// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// Reads the parameters of a serialized root signature (the blob D3D12SerializeVersionedRootSignature writes and
/// CreateRootSignature takes): a DXBC container with an RTS0 part. Each parameter is packed into user SGPRs in order,
/// as the console driver does (docs/pipeline-cache.md, Bindings): a table one SGPR, a root descriptor two, root
/// constants one per value.
/// </summary>
internal static class RootSignatureParser
{
    /// <summary>DXBC container: magic, 16-byte hash, version, total size, part count, then the part offsets.</summary>
    private const uint ContainerMagic = 0x43425844;
    private const int PartCountOffset = 0x1C;
    private const int PartOffsets = 0x20;

    /// <summary>Part header: fourCC and size, then the data.</summary>
    private const uint RootSignaturePart = 0x30535452;
    private const int PartHeader = 8;

    /// <summary>RTS0 header: version, parameter count, parameter offset (then samplers and flags).</summary>
    private const int ParameterCountField = 4;
    private const int ParameterOffsetField = 8;
    private const int RootHeader = 24;

    /// <summary>Parameter header: type, visibility, payload offset; root constants payload: register, space, count.</summary>
    private const int ParameterHeader = 12;
    private const int PayloadField = 8;
    private const int ConstantCountField = 8;
    private const int DescriptorSgprs = 2;

    /// <summary>Table payload: range count, range offset. Ranges: D3D12_DESCRIPTOR_RANGE (version 1.0, 20 bytes) or _RANGE1 (1.1, 24 bytes, flags before the offset).</summary>
    private const int RangeOffsetField = 4;
    private const int RangeCountField = 4;
    private const uint Version10 = 1;
    private const int Range10Bytes = 20;
    private const int Range11Bytes = 24;

    /// <summary>D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND and an unbounded range's NumDescriptors (UINT_MAX).</summary>
    private const uint Append = uint.MaxValue;
    private const uint Unbounded = uint.MaxValue;
    private const int MaxSgprs = byte.MaxValue;

    public static bool TryParse(ReadOnlySpan<byte> blob, out RootLayout layout)
    {
        layout = default;
        var part = FindRootPart(blob);

        if (part.Length < RootHeader)
        {
            return false;
        }

        int count = (int)U32(part, ParameterCountField);
        int parameters = (int)U32(part, ParameterOffsetField);

        if (count > RootLayout.MaxParameters || parameters < 0 || parameters + ((long)count * ParameterHeader) > part.Length)
        {
            return false;
        }

        return ReadParameters(part, parameters, count, ref layout) && ReadExtents(part, parameters, ref layout);
    }

    private static unsafe bool ReadParameters(ReadOnlySpan<byte> part, int parameters, int count, ref RootLayout layout)
    {
        int sgpr = 0;

        for (int index = 0; index < count; index++)
        {
            var header = part[(parameters + (index * ParameterHeader))..];
            var kind = (RootParameterKind)U32(header, 0);
            int size = Size(part, kind, (int)U32(header, PayloadField));

            if (size < 0 || sgpr + size > MaxSgprs)
            {
                return false;
            }

            layout.Kinds[index] = (byte)kind;
            layout.Offsets[index] = (byte)sgpr;
            layout.Sizes[index] = (byte)size;
            sgpr += size;
        }

        layout.Count = count;

        return true;
    }

    /// <summary>The descriptors each table parameter spans; false when a range lies outside the part.</summary>
    private static unsafe bool ReadExtents(ReadOnlySpan<byte> part, int parameters, ref RootLayout layout)
    {
        int rangeBytes = U32(part, 0) == Version10 ? Range10Bytes : Range11Bytes;

        for (int index = 0; index < layout.Count; index++)
        {
            if (layout.Kinds[index] != (byte)RootParameterKind.Table)
            {
                continue;
            }

            int payload = (int)U32(part[(parameters + (index * ParameterHeader))..], PayloadField);
            uint extent = TableExtent(part, payload, rangeBytes);

            if (extent == 0)
            {
                return false;
            }

            layout.Extents[index] = (ushort)extent;
        }

        return true;
    }

    /// <summary>The end of a table's last range, capped at <see cref="DescriptorWindow.MaxTableRecords"/> (also for unbounded ranges); 0 when the table lies outside the part.</summary>
    private static uint TableExtent(ReadOnlySpan<byte> part, int payload, int rangeBytes)
    {
        if (payload < 0 || payload + RangeOffsetField + sizeof(uint) > part.Length)
        {
            return 0;
        }

        uint ranges = U32(part, payload);
        long first = U32(part, payload + RangeOffsetField);

        if (first + ((long)ranges * rangeBytes) > part.Length)
        {
            return 0;
        }

        ulong end = 0;

        for (int range = 0; range < ranges; range++)
        {
            var entry = part[(int)(first + ((long)range * rangeBytes))..];
            uint count = U32(entry, RangeCountField);
            uint offset = U32(entry, rangeBytes - sizeof(uint));
            ulong start = offset == Append ? end : offset;
            end = count == Unbounded ? DescriptorWindow.MaxTableRecords : Math.Max(end, start + count);
        }

        return (uint)Math.Clamp(end, 1, DescriptorWindow.MaxTableRecords);
    }

    /// <summary>The SGPRs a parameter takes; -1 for an unknown kind or a payload outside the part.</summary>
    private static int Size(ReadOnlySpan<byte> part, RootParameterKind kind, int payload)
    {
        switch (kind)
        {
            case RootParameterKind.Table:
                return 1;
            case RootParameterKind.Constants:
                if (payload < 0 || payload + ConstantCountField + sizeof(uint) > part.Length)
                {
                    return -1;
                }

                return (int)Math.Min(U32(part, payload + ConstantCountField), MaxSgprs);
            case RootParameterKind.ConstantBuffer:
            case RootParameterKind.ShaderResource:
            case RootParameterKind.UnorderedAccess:
                return DescriptorSgprs;
            default:
                return -1;
        }
    }

    private static ReadOnlySpan<byte> FindRootPart(ReadOnlySpan<byte> blob)
    {
        if (blob.Length < PartOffsets || U32(blob, 0) != ContainerMagic)
        {
            return [];
        }

        uint parts = U32(blob, PartCountOffset);

        for (int index = 0; index < parts && PartOffsets + ((index + 1) * sizeof(uint)) <= blob.Length; index++)
        {
            int offset = (int)U32(blob, PartOffsets + (index * sizeof(uint)));

            if (offset < 0 || offset + PartHeader > blob.Length || U32(blob, offset) != RootSignaturePart)
            {
                continue;
            }

            int size = (int)Math.Min(U32(blob, offset + sizeof(uint)), (uint)(blob.Length - offset - PartHeader));

            return blob.Slice(offset + PartHeader, size);
        }

        return [];
    }

    private static uint U32(ReadOnlySpan<byte> data, int offset)
    {
        return BinaryPrimitives.ReadUInt32LittleEndian(data[offset..]);
    }
}
