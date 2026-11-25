// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// global_/flat_ loads and stores through a 64-bit VGPR address, or an SGPR base plus a 32-bit VGPR offset (saddr form),
/// with the prelude's PtrLoad/PtrStore (the window for high dword 0, else game memory). Sub-dword loads read the
/// aligned dword and extract.
/// </summary>
internal sealed partial class GlobalMemoryEmitter
{
    private const int ScalarBase = 2;
    private const int BytesPerDword = 4;
    private const int ByteBits = 8;
    private const int ShortBits = 16;
    private const int DwordBits = 32;
    private const long AddressMask = 0xffffffff;

    private static readonly IReadOnlyDictionary<string, int> DwordCounts = new Dictionary<string, int>
    {
        ["dword"] = 1,
        ["dwordx2"] = 2,
        ["dwordx3"] = 3,
        ["dwordx4"] = 4,
    };

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;

    public GlobalMemoryEmitter(TranslationState state, OperandAccess operands)
    {
        _state = state;
        _operands = operands;
    }

    public void Translate(string op, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var known = DwordCounts.TryGetValue(op[(op.LastIndexOf('_') + 1)..], out var count);
        var subDword = SubDwordPattern().Match(op);

        if (!known && !subDword.Success)
        {
            throw new UnsupportedProgramException(op);
        }

        var offset = Hlsl.HexUint(modifiers.Integer("offset", 0) & AddressMask);
        var scalarBase = a.Count > ScalarBase && a[ScalarBase].Kind == OperandKind.Scalar ? a[ScalarBase] : null;

        if (subDword.Success)
        {
            LoadSubDword(subDword.Groups[1].Value, a, scalarBase, offset);

            return;
        }

        // Loads list vdst, vaddr; stores list vaddr, vdata.
        var data = a[Destination];
        var address = a[Source0];

        if (op.Split('_')[1] != "load")
        {
            (data, address) = (address, data);
        }

        var pointer = _operands.SnapPair(Pointer(address, scalarBase, offset));
        Access(op.Contains("load", StringComparison.Ordinal), data, $"{pointer}.x", $"{pointer}.y", count);
    }

    /// <summary>
    /// The 64-bit address (a uint2 expression) of a 64-bit VGPR address, or an SGPR base plus the 32-bit VGPR offset, plus
    /// the signed instruction offset: prelude ptr_add, which carries into the high dword as the hardware does.
    /// </summary>
    private string Pointer(Operand address, Operand? scalarBase, string offset)
    {
        if (scalarBase is not null)
        {
            return $"ptr_add({_operands.Read(scalarBase, Low)}, {_operands.Read(scalarBase, High)}, {_operands.Read(address)}, asint({offset}))";
        }

        return $"ptr_add({_operands.Read(address, Low)}, {_operands.Read(address, High)}, 0u, asint({offset}))";
    }

    private void Access(bool load, Operand data, string low, string high, int count)
    {
        for (var dword = 0; dword < count; dword++)
        {
            var byteOffset = dword * BytesPerDword;

            if (!load)
            {
                _state.Emit($"  if (active) PtrStore({low}, {high}, {byteOffset}u, {_operands.Read(data, dword)});");
                continue;
            }

            var value = $"PtrLoad({low}, {high}, {byteOffset}u)";

            if (_state.UsesCode)
            {
                value = $"({high} == CODE_TAG ? CodeLoad({low} + {byteOffset}u) : {value})";
            }

            _operands.Write(data, value, dword);
        }
    }

    private void LoadSubDword(string kind, IReadOnlyList<Operand> a, Operand? scalarBase, string offset)
    {
        var data = a[Destination];
        var address = a[Source0];
        var pointer = _operands.SnapPair(Pointer(address, scalarBase, offset));
        var byteAddress = $"{pointer}.x";
        var bits = kind.Contains("byte", StringComparison.Ordinal) ? ByteBits : ShortBits;
        var value = $"((PtrLoad({byteAddress} & ~3u, {pointer}.y, 0u) >> (({byteAddress} & 3u) * 8u)) & {Hlsl.HexUint((1L << bits) - 1)})";

        if (kind.StartsWith('s'))
        {
            value = $"asuint(asint({value} << {DwordBits - bits}) >> {DwordBits - bits})";
        }

        _operands.Write(data, value);
    }

    [GeneratedRegex(@"^global_load_(ubyte|sbyte|ushort|sshort)$")]
    private static partial Regex SubDwordPattern();
}
