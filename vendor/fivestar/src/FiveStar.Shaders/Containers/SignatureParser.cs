// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Globalization;
using System.Text;

namespace FiveStar.Shaders.Containers;

/// <summary>Reads ISG1/OSG1/PSG1 signature chunks (u32 element count, u32 table offset, 32-byte elements).</summary>
public static class SignatureParser
{
    // Chunk header: u32 count @0, u32 offset of the element table @4.
    private const int TableOffsetField = 4;
    private const int ElementSize = 32;

    // Element: u32 stream @0, u32 name offset @4, u32 semantic index @8, u32 system value @12,
    // u32 component type @16, u32 register @20, u8 mask @24, u8 read/write mask @25, u32 min precision @28.
    private const int NameOffsetField = 4;
    private const int SemanticIndexField = 8;
    private const int SystemValueField = 12;
    private const int ComponentTypeField = 16;
    private const int RegisterField = 20;
    private const int MaskField = 24;
    private const int ReadWriteMaskField = 25;
    private const int MinPrecisionField = 28;
    private const int ComponentCount = 4;
    private const string ComponentNames = "xyzw";

    // D3D_NAME values (d3dcommon.h).
    private static readonly IReadOnlyDictionary<int, string> SystemValues = new Dictionary<int, string>
    {
        [0] = string.Empty,
        [1] = "SV_Position",
        [2] = "SV_ClipDistance",
        [3] = "SV_CullDistance",
        [4] = "SV_RenderTargetArrayIndex",
        [5] = "SV_ViewportArrayIndex",
        [6] = "SV_VertexID",
        [7] = "SV_PrimitiveID",
        [8] = "SV_InstanceID",
        [9] = "SV_IsFrontFace",
        [10] = "SV_SampleIndex",
        [64] = "SV_Target",
        [65] = "SV_Depth",
        [66] = "SV_Coverage",
    };

    // D3D_REGISTER_COMPONENT_TYPE values plus the min-precision types.
    private static readonly IReadOnlyDictionary<int, string> ComponentTypes = new Dictionary<int, string>
    {
        [0] = "unknown",
        [1] = "uint",
        [2] = "int",
        [3] = "float",
        [4] = "min16uint",
        [5] = "min16int",
        [6] = "min16float",
    };

    /// <summary>Every element of a signature chunk, in table order.</summary>
    public static IReadOnlyList<SignatureElement> Parse(ReadOnlySpan<byte> chunk)
    {
        var count = BinaryPrimitives.ReadInt32LittleEndian(chunk);
        var table = BinaryPrimitives.ReadInt32LittleEndian(chunk[TableOffsetField..]);
        var elements = new List<SignatureElement>(count);

        for (var index = 0; index < count; index++)
        {
            elements.Add(ReadElement(chunk, table + (index * ElementSize)));
        }

        return elements;
    }

    private static SignatureElement ReadElement(ReadOnlySpan<byte> chunk, int at)
    {
        var nameOffset = ReadInt(chunk, at + NameOffsetField);
        var nameEnd = chunk[nameOffset..].IndexOf((byte)0);

        if (nameEnd < 0)
        {
            throw new InvalidDataException("signature name is not terminated");
        }

        var name = Encoding.Latin1.GetString(chunk.Slice(nameOffset, nameEnd));
        var systemValue = Lookup(SystemValues, ReadInt(chunk, at + SystemValueField));
        var componentType = Lookup(ComponentTypes, ReadInt(chunk, at + ComponentTypeField));
        var mask = MaskText(chunk[at + MaskField]);

        return new SignatureElement(
            name,
            ReadInt(chunk, at + SemanticIndexField),
            systemValue,
            componentType,
            ReadInt(chunk, at + RegisterField),
            mask,
            chunk[at + ReadWriteMaskField],
            ReadInt(chunk, at + MinPrecisionField));
    }

    private static int ReadInt(ReadOnlySpan<byte> chunk, int at)
    {
        return BinaryPrimitives.ReadInt32LittleEndian(chunk[at..]);
    }

    private static string Lookup(IReadOnlyDictionary<int, string> names, int code)
    {
        if (names.TryGetValue(code, out var name))
        {
            return name;
        }

        return code.ToString(CultureInfo.InvariantCulture);
    }

    private static string MaskText(byte mask)
    {
        var text = new StringBuilder();

        for (var component = 0; component < ComponentCount; component++)
        {
            if (((mask >> component) & 1) != 0)
            {
                text.Append(ComponentNames[component]);
            }
        }

        return text.ToString();
    }
}
