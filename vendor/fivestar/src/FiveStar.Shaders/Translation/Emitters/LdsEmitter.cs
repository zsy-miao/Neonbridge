// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// LDS (ds_*) accesses on <c>groupshared uint g_lds[]</c> (dword granularity; byte address = vaddr + offset). Sub-dword
/// writes merge with atomics; signed LDS min/max use a compare-exchange loop because g_lds is uint. ds_bpermute and
/// ds_permute move data between lanes (AMD lane numbers wrap at the AMD wave size).
/// </summary>
internal sealed partial class LdsEmitter
{
    private const int DwordBits = 32;
    private const int ByteBits = 8;
    private const int ShortBits = 16;
    private const int BytesPerDword = 4;
    private const int QwordBytes = 8;
    private const int Stride64 = 64;
    private const int DefaultWave = 64;

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;
    private readonly LdsAtomicEmitter _atomics;

    public LdsEmitter(TranslationState state, OperandAccess operands, LdsAtomicEmitter atomics)
    {
        _state = state;
        _operands = operands;
        _atomics = atomics;
    }

    public void Translate(string op, string operandText)
    {
        var supported = _state.Stage is ShaderStage.Compute or ShaderStage.Mesh or ShaderStage.Geometry;

        if (!supported && !_state.IsCopyShader)
        {
            throw new UnsupportedProgramException($"{op} outside compute/mesh");
        }

        _state.UsesLds = true;
        var offsets = new Dictionary<string, string>();

        foreach (Match match in OffsetPattern().Matches(operandText))
        {
            offsets[match.Groups[1].Value] = match.Groups["value"].Value;
        }

        var arguments = ArgumentList.Split(OffsetStripPattern().Replace(operandText, string.Empty));
        var a = arguments.Select(OperandParser.Parse).ToList();
        var kind = op["ds_".Length..];
        var access = new LdsAccess(kind, a, Offset(offsets, "offset"), Offset(offsets, "offset0"), Offset(offsets, "offset1"));

        if (!TryRead(access) && !TryWrite(access) && !_atomics.TryTranslate(access) && !TryPermute(access))
        {
            throw new UnsupportedProgramException(op);
        }
    }

    private static int Offset(Dictionary<string, string> offsets, string key)
    {
        if (!offsets.TryGetValue(key, out var value) || value.Length == 0)
        {
            return 0;
        }

        return int.Parse(value, CultureInfo.InvariantCulture);
    }

    private bool TryRead(LdsAccess access)
    {
        switch (access.Kind)
        {
            case "read_b32":
            case "read_b64":
            case "read_b96":
            case "read_b128":
                ReadDwords(access);
                break;
            case "read_u8":
            case "read_u16":
            case "read_i8":
            case "read_i16":
                ReadSubDword(access);
                break;
            case "read2_b32":
            case "read2_b64":
            case "read2st64_b32":
            case "read2st64_b64":
                ReadPair(access);
                break;
            default:
                return TryReadHalf(access);
        }

        return true;
    }

    private void ReadDwords(LdsAccess access)
    {
        var address = ByteAddress(access, Source0);
        var count = Bits(access.Kind) / DwordBits;

        for (var dword = 0; dword < count; dword++)
        {
            _operands.Write(access.Operands[Destination], $"g_lds[({address} >> 2) + {dword}]", dword);
        }
    }

    private void ReadSubDword(LdsAccess access)
    {
        var address = ByteAddress(access, Source0);
        var width = access.Kind.EndsWith('8') ? ByteBits : ShortBits;
        var value = SubDword(address, width);

        if (access.Kind.StartsWith("read_i", StringComparison.Ordinal))
        {
            value = $"asuint(asint({value} << {DwordBits - width}) >> {DwordBits - width})";
        }

        _operands.Write(access.Operands[Destination], value);
    }

    /// <summary>ds_read_*_d16(_hi): an 8/16-bit read into one half of the destination; the other half is kept.</summary>
    private bool TryReadHalf(LdsAccess access)
    {
        var match = ReadHalfPattern().Match(access.Kind);

        if (!match.Success)
        {
            return false;
        }

        var address = ByteAddress(access, Source0);
        var type = match.Groups[1].Value;
        var width = type.Contains('8', StringComparison.Ordinal) ? ByteBits : ShortBits;
        var value = SubDword(address, width);

        if (type.StartsWith('i'))
        {
            value = $"(asuint(asint({value} << {DwordBits - width}) >> {DwordBits - width}) & 0xffffu)";
        }

        _operands.WriteHalf(access.Operands[Destination], value, match.Groups["high"].Success);

        return true;
    }

    private void ReadPair(LdsAccess access)
    {
        var address = _operands.SnapOne(_operands.Read(access.Operands[Source0]));
        var (size, stride) = PairLayout(access.Kind);
        var dwords = size / BytesPerDword;
        int[] slots = [access.Offset0, access.Offset1];

        for (var slot = 0; slot < slots.Length; slot++)
        {
            for (var dword = 0; dword < dwords; dword++)
            {
                var value = $"g_lds[(({address} + {slots[slot] * stride}u) >> 2) + {dword}]";
                _operands.Write(access.Operands[Destination], value, (slot * dwords) + dword);
            }
        }
    }

    private bool TryWrite(LdsAccess access)
    {
        switch (access.Kind)
        {
            case "write_b32":
            case "write_b64":
            case "write_b96":
            case "write_b128":
                WriteDwords(access);
                break;
            case "write_b8":
            case "write_b16":
            case "write_b8_d16_hi":
            case "write_b16_d16_hi":
                WriteSubDword(access);
                break;
            case "write2_b32":
            case "write2_b64":
            case "write2st64_b32":
            case "write2st64_b64":
                WritePair(access);
                break;
            default:
                return false;
        }

        return true;
    }

    private void WriteDwords(LdsAccess access)
    {
        var address = ByteAddress(access, Destination);
        var count = Bits(access.Kind) / DwordBits;

        for (var dword = 0; dword < count; dword++)
        {
            _state.Emit($"  if (active) g_lds[({address} >> 2) + {dword}] = {_operands.Read(access.Operands[Source0], dword)};");
        }
    }

    private void WriteSubDword(LdsAccess access)
    {
        var address = ByteAddress(access, Destination);
        var width = access.Kind.StartsWith("write_b8", StringComparison.Ordinal) ? ByteBits : ShortBits;
        var data = _operands.Read(access.Operands[Source0]);

        if (access.Kind.EndsWith("_d16_hi", StringComparison.Ordinal))
        {
            // d16_hi: the data comes from the high half.
            data = $"({data} >> 16)";
        }

        _state.Emit($"  if (active) {{ uint _sh = ({address} & 3) * 8; uint _m = {Hlsl.HexUint((1L << width) - 1)} << _sh; "
            + $"InterlockedAnd(g_lds[{address} >> 2], ~_m); InterlockedOr(g_lds[{address} >> 2], ({data} << _sh) & _m); }}");
    }

    private void WritePair(LdsAccess access)
    {
        var address = _operands.SnapOne(_operands.Read(access.Operands[Destination]));
        var (size, stride) = PairLayout(access.Kind);
        var dwords = size / BytesPerDword;
        (int Offset, Operand Data)[] slots = [(access.Offset0, access.Operands[Source0]), (access.Offset1, access.Operands[Source1])];

        foreach (var (offset, data) in slots)
        {
            for (var dword = 0; dword < dwords; dword++)
            {
                _state.Emit($"  if (active) g_lds[(({address} + {offset * stride}u) >> 2) + {dword}] = {_operands.Read(data, dword)};");
            }
        }
    }

    private bool TryPermute(LdsAccess access)
    {
        if (access.Kind is not "bpermute_b32" and not "permute_b32")
        {
            return false;
        }

        var a = access.Operands;
        var laneMask = (_state.Config.WaveSize ?? DefaultWave) - 1;
        var lane = $"((({_operands.Read(a[Source0])} + {access.Offset}u) >> 2) & {laneMask}u)";
        var read = WaveOps.ReadLane;
        _state.ReachWholeWave();

        if (access.Kind == "bpermute_b32")
        {
            // EXEC applies to the source lane: reading a disabled lane yields 0.
            _operands.Write(a[Destination], $"{read}(active ? {_operands.Read(a[Source1])} : 0u, {lane})");

            return true;
        }

        // Forward permute: lane L receives the data of the highest active lane whose address selects L (else 0).
        var values = _operands.Snap(lane, _operands.Read(a[Source1]));
        var name = $"_pm{_state.NextTemporary()}";
        _state.Emit($"  uint {name} = 0; for (uint _i = 0; _i < {WaveOps.Lanes}; _i++) {{ "
            + $"uint _d = {read}({values[0]}, _i); uint _x = {read}({values[1]}, _i); "
            + $"bool _a = {read}(active ? 1u : 0u, _i) != 0; if (_a && _d == {WaveOps.Lane}) {name} = _x; }}");
        _operands.Write(a[Destination], name);

        return true;
    }

    /// <summary>The snapped byte address operand + offset.</summary>
    public string ByteAddress(LdsAccess access, int position)
    {
        return _operands.SnapOne($"({_operands.Read(access.Operands[position])} + {access.Offset}u)");
    }

    private static string SubDword(string address, int width)
    {
        return $"((g_lds[{address} >> 2] >> (({address} & 3) * 8)) & {Hlsl.HexUint((1L << width) - 1)})";
    }

    /// <summary>Element size and offset stride of a read2/write2: b64 elements are 8 bytes; st64 scales offsets by 64.</summary>
    private static (int Size, int Stride) PairLayout(string kind)
    {
        var size = kind.Contains("b64", StringComparison.Ordinal) ? QwordBytes : BytesPerDword;
        var scale = kind.Contains("st64", StringComparison.Ordinal) ? Stride64 : 1;

        return (size, size * scale);
    }

    private static int Bits(string kind)
    {
        return int.Parse(kind[(kind.IndexOf("_b", StringComparison.Ordinal) + "_b".Length)..], CultureInfo.InvariantCulture);
    }

    [GeneratedRegex(@"(offset\d?|gds):?(?<value>\d*)")]
    private static partial Regex OffsetPattern();

    [GeneratedRegex(@"\s+(offset\d?:\d+|gds)\b")]
    private static partial Regex OffsetStripPattern();

    [GeneratedRegex(@"^read_(u8|i8|u16|i16)_d16(?<high>_hi)?$")]
    private static partial Regex ReadHalfPattern();
}
