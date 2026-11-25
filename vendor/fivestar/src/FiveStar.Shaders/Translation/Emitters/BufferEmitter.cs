// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// MUBUF/MTBUF buffer loads and stores on the V#'s view (<see cref="BufferAddress.View"/>). Raw dword accesses use
/// BufLoad/BufStore (a V# the program also writes is loaded through its RW view). Formatted accesses decode/encode in
/// the prelude's FmtLoad/FmtStore with a format code (layout * 8 + numeric type, GCN dfmt/nfmt numbering) from the
/// instruction (tbuffer) or from the hardware format in V# dword3, and loads apply dword3's destination swizzle.
/// </summary>
internal sealed partial class BufferEmitter
{
    // MUBUF operand order: vdata, vaddr, srsrc, soffset.
    private const int Data = 0;
    private const int Address = 1;
    private const int Resource = 2;
    private const int SOffset = 3;
    private const int FormatDword = 3;
    private const int BytesPerDword = 4;
    private const int ByteBits = 8;
    private const int ShortBits = 16;
    private const int DwordBits = 32;

    private const int Vec4 = 4;
    private const int LayoutShift = 8;
    private const int HalfShift = 16;
    private const int HalvesPerRegister = 2;

    /// <summary>The bits of the sub-dword loads.</summary>
    private static readonly IReadOnlyDictionary<string, int> SubDwordBits = new Dictionary<string, int>
    {
        ["ubyte"] = ByteBits,
        ["sbyte"] = ByteBits,
        ["ushort"] = ShortBits,
        ["sshort"] = ShortBits,
        ["byte"] = ByteBits,
        ["short"] = ShortBits,
    };

    private static readonly IReadOnlyDictionary<string, int> DwordCounts = new Dictionary<string, int>
    {
        ["dword"] = 1,
        ["dwordx2"] = 2,
        ["dwordx3"] = 3,
        ["dwordx4"] = 4,
    };

    private static readonly IReadOnlyDictionary<string, int> ComponentCounts = new Dictionary<string, int>
    {
        ["x"] = 1,
        ["xy"] = 2,
        ["xyz"] = 3,
        ["xyzw"] = 4,
    };

    // Data format layouts (GCN BUF_DATA_FORMAT numbering).
    private static readonly IReadOnlyDictionary<string, int> Layouts = new Dictionary<string, int>
    {
        ["8"] = 1,
        ["16"] = 2,
        ["8_8"] = 3,
        ["32"] = 4,
        ["16_16"] = 5,
        ["10_11_11"] = 6,
        ["11_11_10"] = 7,
        ["10_10_10_2"] = 8,
        ["2_10_10_10"] = 9,
        ["8_8_8_8"] = 10,
        ["32_32"] = 11,
        ["16_16_16_16"] = 12,
        ["32_32_32"] = 13,
        ["32_32_32_32"] = 14,
    };

    // Numeric formats (GCN BUF_NUM_FORMAT numbering).
    private static readonly IReadOnlyDictionary<string, int> NumericTypes = new Dictionary<string, int>
    {
        ["UNORM"] = 0,
        ["SNORM"] = 1,
        ["USCALED"] = 2,
        ["SSCALED"] = 3,
        ["UINT"] = 4,
        ["SINT"] = 5,
        ["FLOAT"] = 7,
    };

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;
    private HashSet<int>? _writtenResources;

    public BufferEmitter(TranslationState state, OperandAccess operands)
    {
        _state = state;
        _operands = operands;
    }

    /// <summary>buffer_load_dword* / buffer_store_dword*, and the byte and short loads.</summary>
    public void Translate(string op, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var suffix = op[(op.LastIndexOf('_') + 1)..];

        if (op.StartsWith("buffer_load_", StringComparison.Ordinal) && SubDwordBits.TryGetValue(suffix, out var bits))
        {
            LoadSubDword(suffix, bits, a, modifiers);

            return;
        }

        if (op.StartsWith("buffer_store_", StringComparison.Ordinal) && SubDwordBits.TryGetValue(suffix, out bits))
        {
            StoreSubDword(bits, a, modifiers);

            return;
        }

        if (!DwordCounts.TryGetValue(suffix, out var count))
        {
            throw new UnsupportedProgramException(op);
        }

        var resource = a[Resource];
        var address = BufferAddress.Build(_operands, a[Address], resource, a[SOffset], modifiers);
        var range = BufferAddress.Range(_operands, a[Address], resource, a[SOffset], modifiers);

        if (op.StartsWith("buffer_load", StringComparison.Ordinal))
        {
            var writable = resource.Kind == OperandKind.Scalar && WrittenResources().Contains(resource.Low);
            var view = BufferAddress.View(_operands, resource, writable, address);
            var function = writable ? "BufLoadRW" : "BufLoad";

            for (var dword = 0; dword < count; dword++)
            {
                var inRange = BufferAddress.InRange(range, dword * BytesPerDword);
                _operands.Write(a[Data], $"select({inRange}, {function}({view}.x, {view}.y + {dword * BytesPerDword}u), 0u)", dword);
            }

            return;
        }

        var target = BufferAddress.View(_operands, resource, true, address);

        for (var dword = 0; dword < count; dword++)
        {
            var inRange = BufferAddress.InRange(range, dword * BytesPerDword);
            _state.Emit($"  if (active && {inRange}) {{ TRACE_VSTORE({_operands.Read(resource, 0)}, {_operands.Read(resource, 1)}, {address} + {dword * BytesPerDword}u, {_operands.Read(a[Data], dword)}); BufStore({target}.x, {target}.y + {dword * BytesPerDword}u, {_operands.Read(a[Data], dword)}); }}");
        }
    }

    /// <summary>
    /// buffer_load_ubyte, _sbyte, _ushort and _sshort: the dword holding the access, shifted to its byte and masked,
    /// sign-extended for the signed forms; 0 when the access's bytes do not fit the range (prelude buf_fits_bytes).
    /// The XboxBvhBuilder programs read 16-bit indices this way.
    /// </summary>
    private void LoadSubDword(string suffix, int bits, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var resource = a[Resource];
        var address = BufferAddress.Build(_operands, a[Address], resource, a[SOffset], modifiers);
        var range = BufferAddress.Range(_operands, a[Address], resource, a[SOffset], modifiers);
        var writable = resource.Kind == OperandKind.Scalar && WrittenResources().Contains(resource.Low);
        var view = BufferAddress.View(_operands, resource, writable, address);

        var function = writable ? "BufLoadRW" : "BufLoad";
        var value = $"(({function}({view}.x, {view}.y) >> (({view}.y & 3u) * 8u)) & {Hlsl.HexUint((1L << bits) - 1)})";

        if (suffix.StartsWith('s'))
        {
            value = $"asuint(asint({value} << {DwordBits - bits}) >> {DwordBits - bits})";
        }

        _operands.Write(a[Data], $"select(buf_fits_bytes({range.Offset}, {bits / ByteBits}u, {range.Limit}), {value}, 0u)");
    }

    /// <summary>
    /// buffer_store_byte and _short: the access's bits of the dword holding it, cleared then set with two atomics, so
    /// lanes that store the other bytes of the same dword do not undo each other; dropped out of range.
    /// </summary>
    private void StoreSubDword(int bits, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var resource = a[Resource];
        var address = BufferAddress.Build(_operands, a[Address], resource, a[SOffset], modifiers);
        var range = BufferAddress.Range(_operands, a[Address], resource, a[SOffset], modifiers);
        var view = BufferAddress.View(_operands, resource, true, address);

        var mask = Hlsl.HexUint((1L << bits) - 1);
        var name = $"_sb{_state.NextTemporary()}";
        _state.Emit($"  if (active && buf_fits_bytes({range.Offset}, {bits / ByteBits}u, {range.Limit})) {{ RWByteAddressBuffer {name} = ResourceDescriptorHeap[NonUniformResourceIndex(uav_slot({view}.x))]; "
            + $"uint {name}s = ({view}.y & 3u) * 8u; {name}.InterlockedAnd({view}.y & ~3u, ~({mask} << {name}s)); {name}.InterlockedOr({view}.y & ~3u, ({_operands.Read(a[Data])} & {mask}) << {name}s); }}");
    }

    /// <summary>buffer_load/store_format_* and tbuffer_load/store_format_*.</summary>
    public void TranslateFormatted(string op, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        if (!ComponentCounts.TryGetValue(op[(op.LastIndexOf('_') + 1)..], out var count))
        {
            throw new UnsupportedProgramException(op);
        }

        var resource = a[Resource];
        var dword3 = _operands.Read(resource, FormatDword);
        var code = op.StartsWith("tbuffer", StringComparison.Ordinal) ? InstructionFormat(modifiers) : $"buf_code({dword3})";
        var store = op.Contains("store", StringComparison.Ordinal);
        var half = HalfData(op);

        var address = BufferAddress.Build(_operands, a[Address], resource, a[SOffset], modifiers);
        var format = _operands.SnapOne(code);

        var range = BufferAddress.Range(_operands, a[Address], resource, a[SOffset], modifiers);
        var inRange = BufferAddress.InRange((range.Offset, $"fmt_limit({range.Limit}, {format})"), 0);
        var view = BufferAddress.View(_operands, resource, store, address);
        var byteAddress = $"{view}.y";
        var swizzle = _operands.SnapOne(dword3);

        if (store)
        {
            var data = new List<string>();

            for (var component = 0; component < Vec4; component++)
            {
                data.Add(StoredComponent(a[Data], component, count, half, format));
            }

            _state.Emit($"  if (active && {inRange}) FmtStore({view}.x, {byteAddress}, {format}, uint4({string.Join(", ", data)}), {count}u);");

            return;
        }

        var name = $"_fmt{_state.NextTemporary()}";
        _state.Emit($"  uint4 {name} = select({inRange}, FmtLoad({view}.x, {byteAddress}, {format}, {swizzle}), uint4(0u, 0u, 0u, 0u));");

        if (half != HalfKind.None)
        {
            WriteHalves(a[Data], name, count, half, format);

            return;
        }

        for (var component = 0; component < count; component++)
        {
            _operands.Write(a[Data], $"{name}.{"xyzw"[component]}", component);
        }
    }

    /// <summary>
    /// The d16 forms (<c>_d16_</c>, <c>_d16_hi_</c> in the opcode): components travel as 16-bit halves, two per VGPR, low
    /// half first; <c>_d16_hi_x</c> uses the high half. Float and normalized formats as f16, integer formats as their low
    /// 16 bits (prelude fmt_half / fmt_unhalf).
    /// </summary>
    private static HalfKind HalfData(string op)
    {
        if (op.Contains("_d16_hi_", StringComparison.Ordinal))
        {
            return HalfKind.High;
        }

        if (op.Contains("_d16_", StringComparison.Ordinal))
        {
            return HalfKind.Low;
        }

        return HalfKind.None;
    }

    /// <summary>A d16 load's halves into their VGPRs; a half without a partner keeps the other half of its VGPR, as gfx10 does.</summary>
    private void WriteHalves(Operand data, string loaded, int count, HalfKind half, string format)
    {
        for (var component = 0; component < count; component += HalvesPerRegister)
        {
            var register = Register(data, component / HalvesPerRegister);
            var low = $"fmt_half({loaded}.{"xyzw"[component]}, {format})";

            if (half == HalfKind.High)
            {
                _operands.WriteHalf(register, low, high: true);
            }
            else if (component + 1 < count)
            {
                _operands.Write(register, $"({low} | (fmt_half({loaded}.{"xyzw"[component + 1]}, {format}) << 16))");
            }
            else
            {
                _operands.WriteHalf(register, low, high: false);
            }
        }
    }

    /// <summary>A store's component: a whole VGPR, or a d16 half widened back to the format's register form.</summary>
    private string StoredComponent(Operand data, int component, int count, HalfKind half, string format)
    {
        if (component >= count)
        {
            return "0u";
        }

        if (half == HalfKind.None)
        {
            return _operands.Read(data, component);
        }

        var register = _operands.Read(Register(data, component / HalvesPerRegister));
        var shift = half == HalfKind.High || component % HalvesPerRegister == 1 ? HalfShift : 0;

        return $"fmt_unhalf(({register} >> {shift}) & 0xffffu, {format})";
    }

    private static Operand Register(Operand data, int index)
    {
        return data with { Low = data.Low + index, High = data.Low + index, Text = $"v{data.Low + index}" };
    }

    private static string InstructionFormat(InstructionModifiers modifiers)
    {
        var format = FormatPattern().Match(modifiers.Value("format") ?? string.Empty);

        if (!format.Success || !Layouts.TryGetValue(format.Groups[1].Value, out var layout))
        {
            throw new UnsupportedProgramException($"tbuffer format {modifiers.Value("format")}");
        }

        return $"{(layout * LayoutShift) + NumericTypes[format.Groups["numeric"].Value]}u";
    }

    /// <summary>SGPR bases of the V#s this program writes through (buffer stores and atomics), from every instruction.</summary>
    private HashSet<int> WrittenResources()
    {
        if (_writtenResources is not null)
        {
            return _writtenResources;
        }

        _writtenResources = [];

        foreach (var instruction in _state.Instructions)
        {
            var match = WritePattern().Match(instruction.Text);

            if (!match.Success)
            {
                continue;
            }

            var arguments = ArgumentList.Split(InstructionModifiers.Strip(" " + match.Groups[1].Value));
            var register = arguments.Count > Resource ? OperandParser.TryParseRegister(arguments[Resource]) : null;

            if (register is not null && register.Kind == OperandKind.Scalar)
            {
                _writtenResources.Add(register.Low);
            }
        }

        return _writtenResources;
    }

    [GeneratedRegex(@"^\[BUF_FMT_(\w+?)_(?<numeric>UNORM|SNORM|USCALED|SSCALED|UINT|SINT|FLOAT)\]")]
    private static partial Regex FormatPattern();

    [GeneratedRegex(@"^(?:buffer_store\w*|buffer_atomic\w*|tbuffer_store\w*)\s+(.*)")]
    private static partial Regex WritePattern();
}
