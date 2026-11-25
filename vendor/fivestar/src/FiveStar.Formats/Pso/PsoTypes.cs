// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;

namespace FiveStar.Formats.Pso;

/// <summary>The catalog of known type codes with their names and sizes (sizes from member offsets and alignment).</summary>
public static class PsoTypes
{
    private static readonly PsoTypeInfo[] Catalog =
    [
        new(PsoTypeCode.Bool, "bool", 1),
        new(PsoTypeCode.Struct, "struct", null),
        new(PsoTypeCode.StructPointer, "ptr", 8),
        new(PsoTypeCode.Pointer08, "ptr8", 8),
        new(PsoTypeCode.Unknown0C, "t0c", 8),
        new(PsoTypeCode.Unknown0D, "t0d", 16),
        new(PsoTypeCode.S8, "s8", 1),
        new(PsoTypeCode.U8, "u8", 1),
        new(PsoTypeCode.S16, "s16", 2),
        new(PsoTypeCode.U16, "u16", 2),
        new(PsoTypeCode.S32, "s32", 4),
        new(PsoTypeCode.U32, "u32", 4),
        new(PsoTypeCode.U64, "u64", 8),
        new(PsoTypeCode.S64, "s64", 8),
        new(PsoTypeCode.F16, "f16", 2),
        new(PsoTypeCode.F32, "f32", 4),
        new(PsoTypeCode.F64, "f64", 8),
        new(PsoTypeCode.Vec2, "vec2", 8),
        new(PsoTypeCode.Vec3, "vec3", 16),
        new(PsoTypeCode.Vec3Packed, "vec3p", 12),
        new(PsoTypeCode.Vec4, "vec4", 16),
        new(PsoTypeCode.Vec3V, "vec3v", 16),
        new(PsoTypeCode.Mat34, "mat34", 48),
        new(PsoTypeCode.Mat44, "mat44", 64),
        new(PsoTypeCode.String40, "str40", 16),
        new(PsoTypeCode.String42, "str42", 8),
        new(PsoTypeCode.String44, "str44", 16),
        new(PsoTypeCode.Hash, "hash", 4),
        new(PsoTypeCode.Hash4B, "hash4b", 4),
        new(PsoTypeCode.NamespacedHash, "nshash", 4),
        new(PsoTypeCode.NamespacedHash64, "nshash64", 8),
        new(PsoTypeCode.FixedArray, "fixarr", null),
        new(PsoTypeCode.AtArray, "arr52", 16),
        new(PsoTypeCode.FixedArray53, "fixarr53", null),
        new(PsoTypeCode.FixedArray54, "fixarr54", null),
        new(PsoTypeCode.Array, "array", 16),
        new(PsoTypeCode.Enum8, "enum8", 1),
        new(PsoTypeCode.Enum16, "enum16", 2),
        new(PsoTypeCode.Enum32, "enum32", 4),
        new(PsoTypeCode.Bits8, "bits8", 1),
        new(PsoTypeCode.Bits16, "bits16", 2),
        new(PsoTypeCode.Bits32, "bits32", 4),
        new(PsoTypeCode.Bits64, "bits64", 8),
        new(PsoTypeCode.Map71, "map71", 24),
        new(PsoTypeCode.Map73, "map73", 16),
        new(PsoTypeCode.Map75, "map75", 16),
        new(PsoTypeCode.Map76, "t76", 16),
        new(PsoTypeCode.Map77, "t77", 16),
    ];

    private static readonly FrozenDictionary<PsoTypeCode, PsoTypeInfo> ByCode = Catalog.ToFrozenDictionary(info => info.Code);

    /// <summary>The catalog entry of a code, or null for an unknown code.</summary>
    public static PsoTypeInfo? Find(PsoTypeCode code)
    {
        ByCode.TryGetValue(code, out var info);

        return info;
    }

    /// <summary>True for the inline fixed arrays (<c>ref</c> = element count, <c>aux</c> = element member).</summary>
    public static bool IsFixedArray(PsoTypeCode code)
    {
        return code == PsoTypeCode.FixedArray || code == PsoTypeCode.FixedArray53 || code == PsoTypeCode.FixedArray54;
    }

    /// <summary>True for the map types (storage: count, then a pointer to a block of node structs).</summary>
    public static bool IsMap(PsoTypeCode code)
    {
        return code == PsoTypeCode.Map71 || code == PsoTypeCode.Map73 || code == PsoTypeCode.Map75
            || code == PsoTypeCode.Map76 || code == PsoTypeCode.Map77;
    }
}
