// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Shaders.Cache;

/// <summary>
/// Reads a shader's metadata entry: a header (0x10 bytes for version 2, 0x18 for 0x403), 32-byte entries tagged 0x41
/// or 0x42, and a last entry tag 0x43 with a 16-byte hash.
/// </summary>
public static class ShaderMetaParser
{
    /// <summary>Header version of graphics shaders.</summary>
    public const uint GraphicsVersion = 2;

    /// <summary>Header version of compute shaders.</summary>
    public const uint ComputeVersion = 0x403;

    private const int GraphicsHeader = 0x10;
    private const int ComputeHeader = 0x18;
    private const int EntrySize = 0x20;
    private const uint UserDataTag = 0x41;
    private const uint ResourceTag = 0x42;
    private const uint HashTag = 0x43;
    private const int HashSize = 16;

    // Entry fields after the tag.
    private const int KindField = 4;
    private const int OffsetField = 8;
    private const int RegisterField = 12;
    private const int ValueField = 16;
    private const int LayoutField = 20;

    // Packed fields: 0x41 +4 = kind << 8 | class; 0x41 +20 = parameter << 16 | SGPR << 8 | size;
    // 0x42 +8 = resource kind << 16 | class << 8 | (1 used, 2 not used).
    private const int ByteBits = 8;
    private const int HalfBits = 16;
    private const uint ByteMask = 0xFF;
    private const uint UsedMarker = 1;

    // Register classes of the 0x42 entries by their code (0 SRV, 1 UAV, 2 CBV, 3 sampler).
    private static readonly BindingClass[] ResourceClasses =
        [BindingClass.ShaderResource, BindingClass.UnorderedAccess, BindingClass.ConstantBuffer, BindingClass.Sampler];

    /// <summary>The entries of the metadata, or null when it is not a shader's metadata.</summary>
    public static ShaderMeta? Parse(ReadOnlySpan<byte> meta)
    {
        if (meta.Length < ComputeHeader)
        {
            return null;
        }

        var version = BinaryPrimitives.ReadUInt32LittleEndian(meta);
        int position;

        if (version == GraphicsVersion)
        {
            position = GraphicsHeader;
        }
        else if (version == ComputeVersion)
        {
            position = ComputeHeader;
        }
        else
        {
            return null;
        }

        return ReadEntries(meta, version, position);
    }

    private static ShaderMeta? ReadEntries(ReadOnlySpan<byte> meta, uint version, int position)
    {
        var userData = new List<UserDataEntry>();
        var resources = new List<ResourceEntry>();

        while (position + sizeof(uint) <= meta.Length)
        {
            var entry = meta[position..];
            var tag = BinaryPrimitives.ReadUInt32LittleEndian(entry);

            if (tag == HashTag && entry.Length >= sizeof(uint) + HashSize)
            {
                var hash = Convert.ToHexStringLower(entry.Slice(sizeof(uint), HashSize));

                return new ShaderMeta(version, userData, resources, hash);
            }

            if (entry.Length < EntrySize || !ReadEntry(entry, tag, userData, resources))
            {
                return null;
            }

            position += EntrySize;
        }

        return null;
    }

    private static bool ReadEntry(ReadOnlySpan<byte> entry, uint tag, List<UserDataEntry> userData, List<ResourceEntry> resources)
    {
        if (tag == UserDataTag)
        {
            var user = ReadUserData(entry);

            if (user.HasValue)
            {
                userData.Add(user.Value);
            }

            return user.HasValue;
        }

        if (tag == ResourceTag)
        {
            var resource = ReadResource(entry);

            if (resource.HasValue)
            {
                resources.Add(resource.Value);
            }

            return resource.HasValue;
        }

        return false;
    }

    private static UserDataEntry? ReadUserData(ReadOnlySpan<byte> entry)
    {
        var kindWord = U32(entry, KindField);
        var kind = (UserDataKind)((kindWord >> ByteBits) & ByteMask);
        var bindingClass = (BindingClass)(kindWord & ByteMask);

        if (!Enum.IsDefined(kind) || !Enum.IsDefined(bindingClass))
        {
            return null;
        }

        var layout = U32(entry, LayoutField);

        return new UserDataEntry(
            kind,
            bindingClass,
            (int)U32(entry, OffsetField),
            (int)U32(entry, RegisterField),
            (int)U32(entry, ValueField),
            (int)(layout & ByteMask),
            (int)((layout >> ByteBits) & ByteMask),
            (int)((layout >> HalfBits) & ByteMask));
    }

    private static ResourceEntry? ReadResource(ReadOnlySpan<byte> entry)
    {
        var type = U32(entry, OffsetField);
        var classCode = (type >> ByteBits) & ByteMask;

        if (classCode >= ResourceClasses.Length)
        {
            return null;
        }

        return new ResourceEntry(
            (type & ByteMask) == UsedMarker,
            ResourceClasses[classCode],
            (int)(type >> HalfBits),
            (int)U32(entry, RegisterField),
            (int)U32(entry, ValueField));
    }

    private static uint U32(ReadOnlySpan<byte> entry, int field)
    {
        return BinaryPrimitives.ReadUInt32LittleEndian(entry[field..]);
    }
}
