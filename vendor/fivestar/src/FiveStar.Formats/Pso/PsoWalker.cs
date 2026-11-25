// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Globalization;
using FiveStar.Formats.Common;

namespace FiveStar.Formats.Pso;

/// <summary>
/// Walks a PSO's root object into a value tree (see <see cref="JsonText"/>): every struct becomes an ordered map
/// with <c>_type</c> and one entry per member (keys are 8-digit hex name hashes), arrays become lists, pointers to
/// struct blocks are followed. Hashes print as <c>h:xxxxxxxx</c>, matched enum values as <c>enum:xxxxxxxx</c>,
/// unknown types as <c>rawNN:hex</c>. Floats in vectors and matrices are rounded to 6 decimals.
/// </summary>
public sealed class PsoWalker
{
    /// <summary>A budget large enough for any file.</summary>
    public const long UnlimitedBudget = 1L << 30;

    private const int MaxDepth = 64;
    private const int VectorDecimals = 6;
    private const int DefaultRawSize = 8;
    private const int PointerSize = sizeof(ulong);
    private const int ArrayPointerOffset = 8;
    private const int AtArrayCountOffset = 8;
    private const int MapSlotStep = 8;
    private const int ArrayCountsSize = 8;
    private const int AtArrayCountsSize = 4;

    private readonly PsoDocument _document;
    private readonly byte[] _data;
    private readonly Dictionary<uint, string> _names = [];
    private long _budget;

    public PsoWalker(PsoDocument document)
    {
        _document = document;
        _data = document.Bytes;
    }

    /// <summary>The root object's tree; at most <paramref name="budget"/> member values are read, then "..." stands in.</summary>
    public object? WalkRoot(long budget)
    {
        var root = _document.Root;

        if (!root.HasValue || root.Value == 0)
        {
            throw new PsoException("no root", false);
        }

        var block = _document.BlockAt(root.Value);
        var schema = _document.StructNamed(block.Type);

        if (!block.Offset.HasValue)
        {
            throw new PsoException("root in a physical block", false);
        }

        _budget = budget;

        return ReadObject(block.Offset.Value, schema, 0);
    }

    private object ReadObject(long at, PsoStruct schema, int depth)
    {
        if (depth > MaxDepth)
        {
            return "<deep>";
        }

        var result = new OrderedMap<string, object?>();
        result.Set("_type", Name(schema.Name));

        foreach (var member in schema.Members)
        {
            if (member.Name == PsoMember.AnonymousElement)
            {
                continue;
            }

            result.Set(Name(member.Name), ReadLocated(at, schema, member, depth));
        }

        return result;
    }

    // A failure deep inside is reported once, with the innermost member, type and struct.
    private object? ReadLocated(long at, PsoStruct schema, PsoMember member, int depth)
    {
        try
        {
            return ReadMember(at + member.Offset, schema, member, depth);
        }
        catch (PsoException error) when (!error.Located)
        {
            var where = $" [member {Hex8(member.Name)} type {DecimalText.Hex(member.Type)} in struct {Hex8(schema.Name)} @{DecimalText.Hex(at + member.Offset)}]";

            throw new PsoException(error.Message + where, true);
        }
    }

    private object? ReadMember(long at, PsoStruct schema, PsoMember member, int depth)
    {
        if (_budget <= 0)
        {
            return "...";
        }

        _budget--;
        var code = member.Code;

        switch (code)
        {
            case PsoTypeCode.Struct:
                return ReadObject(at, _document.StructNamed(member.Ref), depth + 1);
            case PsoTypeCode.Array:
            case PsoTypeCode.AtArray:
                return ReadArray(at, schema, member, depth);
            case PsoTypeCode.StructPointer:
                return ReadPointer(at, depth);
            case PsoTypeCode.Enum8:
            case PsoTypeCode.Enum16:
            case PsoTypeCode.Enum32:
                return ReadEnum(at, member);
            case PsoTypeCode.Hash:
            case PsoTypeCode.Hash4B:
            case PsoTypeCode.NamespacedHash:
                return "h:" + Hex8(PsoBytes.U32(_data, at));
            case PsoTypeCode.NamespacedHash64:
                return "h64:" + PsoBytes.U64(_data, at).ToString("x16", CultureInfo.InvariantCulture);
            default:
                return ReadOther(at, schema, member, depth);
        }
    }

    private object? ReadOther(long at, PsoStruct schema, PsoMember member, int depth)
    {
        var code = member.Code;

        if (PsoTypes.IsFixedArray(code))
        {
            return ReadFixedArray(at, schema, member, depth);
        }

        if (PsoTypes.IsMap(code))
        {
            return ReadMap(at, code, depth);
        }

        var scalar = PsoScalars.TryRead(_data, at, code, VectorDecimals);

        if (scalar.Found)
        {
            return scalar.Value;
        }

        var size = MemberSize(schema, member);

        if (!size.HasValue || size.Value == 0)
        {
            size = DefaultRawSize;
        }

        var prefix = "raw" + ((byte)code).ToString("x2", CultureInfo.InvariantCulture) + ":";

        return prefix + Convert.ToHexStringLower(PsoBytes.Slice(_data, at, size.Value));
    }

    private object? ReadArray(long at, PsoStruct schema, PsoMember member, int depth)
    {
        long count;
        ResolvedPointer? target;

        if (member.Code == PsoTypeCode.Array)
        {
            // {u32 count, u32 capacity, pointer}
            PsoBytes.Check(_data, at, ArrayCountsSize);
            count = PsoBytes.U32(_data, at);
            target = Resolve(at + ArrayPointerOffset);
        }
        else
        {
            // {pointer, u16 count, u16 capacity, padding}
            PsoBytes.Check(_data, at + AtArrayCountOffset, AtArrayCountsSize);
            count = PsoBytes.U16(_data, at + AtArrayCountOffset);
            target = Resolve(at);
        }

        var element = ElementOf(schema, member.Aux);

        if (!target.HasValue || count == 0)
        {
            return new List<object?>();
        }

        var elementSize = MemberSize(schema, element);

        if (!elementSize.HasValue)
        {
            var summary = new OrderedMap<string, object?>();
            summary.Set("count", count);
            summary.Set("elem_type", DecimalText.Hex(element.Type));
            summary.Set("at", DecimalText.Hex(target.Value.Address));

            return summary;
        }

        var items = new List<object?>();

        for (long index = 0; index < count; index++)
        {
            items.Add(ReadMember(target.Value.Address + (index * elementSize.Value), schema, element, depth));
        }

        return items;
    }

    private List<object?> ReadFixedArray(long at, PsoStruct schema, PsoMember member, int depth)
    {
        var element = ElementOf(schema, member.Aux);
        var elementSize = MemberSize(schema, element);
        var items = new List<object?>();

        for (long index = 0; index < member.Ref; index++)
        {
            if (!elementSize.HasValue)
            {
                throw new PsoException("unsupported operand type(s) for *: 'int' and 'NoneType'", false);
            }

            items.Add(ReadMember(at + (index * elementSize.Value), schema, element, depth));
        }

        return items;
    }

    private object? ReadPointer(long at, int depth)
    {
        var target = Resolve(at);

        if (!target.HasValue)
        {
            return null;
        }

        if (_document.Structs.TryGetValue(target.Value.Type, out var schema))
        {
            return ReadObject(target.Value.Address, schema, depth + 1);
        }

        var reference = new OrderedMap<string, object?>();
        reference.Set("ptr", (long)target.Value.Block);
        reference.Set("type", DecimalText.Hex(target.Value.Type));

        return reference;
    }

    // Maps: u32 count, then the first slot holding a pointer to a block of node structs (Swiss-table groups).
    private OrderedMap<string, object?> ReadMap(long at, PsoTypeCode code, int depth)
    {
        var count = PsoBytes.U32(_data, at);
        var size = PsoTypes.Find(code)!.Size!.Value;
        var map = new OrderedMap<string, object?>();
        map.Set("_map", DecimalText.Hex((byte)code));

        for (var slot = MapSlotStep; slot < size; slot += MapSlotStep)
        {
            var raw = PsoBytes.U64(_data, at + slot);
            var pointer = PsoPointer.Decode(raw);

            if (raw == 0 || pointer.Block == 0 || pointer.Block > _document.Blocks.Count)
            {
                continue;
            }

            var block = _document.Blocks[(int)pointer.Block - 1];

            if (!_document.Structs.ContainsKey(block.Type))
            {
                continue;
            }

            var target = Resolve(at + slot)!.Value;
            var node = _document.Structs[target.Type];
            var fit = ((long)block.Size - (long)pointer.Offset) / Math.Max(node.Size, 1);
            var nodes = new List<object?>();

            for (long index = 0; index < Math.Min(count, fit); index++)
            {
                nodes.Add(ReadObject(target.Address + (index * node.Size), node, depth + 1));
            }

            map.Set("count", (long)count);
            map.Set("nodes", nodes);

            return map;
        }

        map.Set("raw", Convert.ToHexStringLower(PsoBytes.Slice(_data, at, size)));

        return map;
    }

    private object ReadEnum(long at, PsoMember member)
    {
        var size = PsoTypes.Find(member.Code)!.Size!.Value;
        var bytes = PsoBytes.Slice(_data, at, size);
        var value = SignedLittleEndian(bytes);

        if (_document.Enums.TryGetValue(member.Ref, out var values))
        {
            foreach (var candidate in values)
            {
                if (candidate.Value == value)
                {
                    return "enum:" + Hex8(candidate.Hash);
                }
            }
        }

        return value;
    }

    private ResolvedPointer? Resolve(long at)
    {
        var raw = PsoBytes.U64(_data, at);
        var pointer = PsoPointer.Decode(raw);

        if (pointer.Block == 0)
        {
            if (raw != 0)
            {
                throw new PsoException($"null block with offset {DecimalText.Hex((long)raw)} at {DecimalText.Hex(at)}", false);
            }

            return null;
        }

        if (pointer.Block > _document.Blocks.Count)
        {
            throw new PsoException($"bad block id {DecimalText.Hex(pointer.Block)} at {DecimalText.Hex(at)}", false);
        }

        var block = _document.Blocks[(int)pointer.Block - 1];

        if (!block.Offset.HasValue)
        {
            throw new PsoException("pointer into physical block " + pointer.Block, false);
        }

        if (pointer.Offset > block.Size)
        {
            throw new PsoException($"offset {DecimalText.Hex((long)pointer.Offset)} beyond block {pointer.Block} size {DecimalText.Hex(block.Size)}", false);
        }

        return new ResolvedPointer(block.Offset.Value + (long)pointer.Offset, block.Type, pointer.Block);
    }

    private long? MemberSize(PsoStruct schema, PsoMember member)
    {
        var code = member.Code;

        if (code == PsoTypeCode.Struct)
        {
            return _document.StructNamed(member.Ref).Size;
        }

        var info = PsoTypes.Find(code);

        if (info is not null && info.Size.HasValue && info.Size.Value != 0)
        {
            return info.Size.Value;
        }

        if (!PsoTypes.IsFixedArray(code))
        {
            return null;
        }

        var elementSize = MemberSize(schema, ElementOf(schema, member.Aux));

        if (!elementSize.HasValue)
        {
            throw new PsoException("unsupported operand type(s) for *: 'int' and 'NoneType'", false);
        }

        return member.Ref * elementSize.Value;
    }

    private static PsoMember ElementOf(PsoStruct schema, int index)
    {
        if (index >= schema.Members.Count)
        {
            throw new PsoException("list index out of range", false);
        }

        return schema.Members[index];
    }

    private static long SignedLittleEndian(ReadOnlySpan<byte> bytes)
    {
        if (bytes.IsEmpty)
        {
            return 0;
        }

        Span<byte> wide = stackalloc byte[sizeof(long)];
        wide.Clear();

        if ((sbyte)bytes[^1] < 0)
        {
            wide.Fill(byte.MaxValue);
        }

        bytes.CopyTo(wide);

        return BinaryPrimitives.ReadInt64LittleEndian(wide);
    }

    // Name hashes repeat in every object of a struct; their text is made once.
    private string Name(uint hash)
    {
        if (!_names.TryGetValue(hash, out var name))
        {
            name = Hex8(hash);
            _names[hash] = name;
        }

        return name;
    }

    private static string Hex8(uint value)
    {
        return value.ToString("x8", CultureInfo.InvariantCulture);
    }

    private readonly record struct ResolvedPointer(long Address, uint Type, uint Block);
}
