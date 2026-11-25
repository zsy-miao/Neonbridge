// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// buffer_atomic_*, global_atomic_* and image_atomic_*: an HLSL Interlocked* on the RW view (buffer: the V#'s view,
/// <see cref="BufferAddress.View"/>; global: the pointer's, prelude gpu_rw; image: a typed RW texture). With glc the
/// pre-op value is returned in the data register. sub adds the negated value; fmin and fmax order 32-bit floats as
/// integers (a non-negative float by its signed bits, a negative one by its unsigned bits, reversed), so one integer
/// atomic does them on a buffer or by address (XboxBvhBuilder's bounds).
/// </summary>
internal sealed partial class AtomicEmitter
{
    private const int Data = 0;
    private const int Address = 1;
    private const int Resource = 2;
    private const int SOffset = 3;
    private const int ReturnedAddress = 1;
    private const int ScalarBaseAfterAddress = 2;
    private const int CompareValue = 1;
    private const long AddressMask = 0xffffffff;
    private const int WideMask = 0x3;
    private const int WideCompareMask = 0xf;
    private const int CompareLow = 2;
    private const int CompareHigh = 3;

    /// <summary>The function name of the float min and max, written by <see cref="Operation"/>.</summary>
    private const string FloatOrder = "float";

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;

    public AtomicEmitter(TranslationState state, OperandAccess operands)
    {
        _state = state;
        _operands = operands;
    }

    public void Translate(string op, string operandText, InstructionModifiers modifiers)
    {
        var arguments = ArgumentList.Split(operandText);
        var kind = AtomicPrefixPattern().Replace(op, string.Empty);
        var function = InterlockedFunction(kind);

        if (function is null && kind != "cmpswap")
        {
            throw new UnsupportedProgramException(op);
        }

        var signed = kind is "smin" or "smax";
        var data = OperandParser.Parse(arguments[Data]);
        var result = $"_at{_state.NextTemporary()}";
        var atomic = new AtomicOperation(kind, function, signed, data, result);

        if (op.StartsWith("global_", StringComparison.Ordinal))
        {
            Global(atomic, arguments, modifiers);

            return;
        }

        if (op.StartsWith("buffer_", StringComparison.Ordinal))
        {
            Buffer(atomic, arguments, modifiers);
        }
        else if (op.StartsWith("image_", StringComparison.Ordinal))
        {
            if (!Image(op, atomic, arguments, modifiers))
            {
                return;
            }
        }
        else
        {
            throw new UnsupportedProgramException(op);
        }

        if (modifiers.Has("glc"))
        {
            WriteResult(atomic);
        }
    }

    private static string? InterlockedFunction(string kind)
    {
        switch (kind)
        {
            case "add":
            case "sub":
                return "InterlockedAdd";
            case "fmin":
            case "fmax":
                return FloatOrder;
            case "umin":
            case "smin":
                return "InterlockedMin";
            case "umax":
            case "smax":
                return "InterlockedMax";
            case "and":
                return "InterlockedAnd";
            case "or":
                return "InterlockedOr";
            case "xor":
                return "InterlockedXor";
            case "swap":
                return "InterlockedExchange";
            default:
                return null;
        }
    }

    private void WriteResult(AtomicOperation atomic)
    {
        var value = atomic.Signed ? $"asuint({atomic.Result})" : atomic.Result;
        _operands.Write(atomic.Data, value);
    }

    /// <summary>global_atomic_* [vdst (glc),] vaddr, vdata, saddr|off on the pointer's writable view.</summary>
    private void Global(AtomicOperation atomic, List<string> arguments, InstructionModifiers modifiers)
    {
        // Without glc the operands are vaddr, vdata, saddr; with glc vdst comes first.
        var returns = modifiers.Has("glc");
        var first = returns ? ReturnedAddress : 0;
        var address = OperandParser.Parse(arguments[first]);
        var data = OperandParser.Parse(arguments[first + 1]);
        var scalarBase = arguments[first + ScalarBaseAfterAddress];

        var offset = Hlsl.HexUint(modifiers.Integer("offset", 0) & AddressMask);
        string pointer;

        // prelude ptr_add carries the offsets into the high dword as the hardware's 64-bit add does.
        if (scalarBase != "off")
        {
            var scalar = OperandParser.Parse(scalarBase);
            pointer = $"ptr_add({_operands.Read(scalar, 0)}, {_operands.Read(scalar, 1)}, {_operands.Read(address)}, asint({offset}))";
        }
        else
        {
            pointer = $"ptr_add({_operands.Read(address, 0)}, {_operands.Read(address, 1)}, 0u, asint({offset}))";
        }

        var full = _operands.SnapPair(pointer);
        var target = _operands.SnapPair($"gpu_rw({full}.x, {full}.y)");
        var result = atomic.Result;
        _state.Emit($"  if (active && {full}.y != 0u) TRACE_ADDR({full}.x, {full}.y, {_operands.Read(data, 0)});");
        _state.Emit($"  uint {result} = 0; if (active) {{ RWByteAddressBuffer _b{result} = ResourceDescriptorHeap[NonUniformResourceIndex(uav_slot({target}.x))];");
        _state.Emit($"    {Operation(atomic, data, $"{target}.y", $"_b{result}")} }}");

        if (returns)
        {
            WriteResult(atomic);
        }
    }

    /// <summary>The statement of a buffer or global atomic on the RW byte-address view <paramref name="view"/>.</summary>
    private string Operation(AtomicOperation atomic, Operand data, string byteAddress, string view)
    {
        if (atomic.Kind == "cmpswap")
        {
            var swap = _operands.Read(data, CompareValue);

            return $"{view}.InterlockedCompareExchange({byteAddress} & ~3u, {swap}, {_operands.Read(data, 0)}, {atomic.Result});";
        }

        if (atomic.Function == FloatOrder)
        {
            return FloatAtomic(atomic, _operands.Read(data), byteAddress, view);
        }

        var value = atomic.Signed ? $"asint({_operands.Read(data)})" : _operands.Read(data);

        if (atomic.Kind == "sub")
        {
            value = $"(0u - {value})";
        }

        return $"{view}.{atomic.Function}({byteAddress} & ~3u, {value}, {atomic.Result});";
    }

    /// <summary>
    /// fmax: a non-negative value takes the signed maximum of the bits, a negative one the unsigned minimum (a larger
    /// negative float has larger unsigned bits; every non-negative one has smaller); fmin the other way round.
    /// </summary>
    private static string FloatAtomic(AtomicOperation atomic, string value, string byteAddress, string view)
    {
        var (positive, negative) = atomic.Kind == "fmax" ? ("InterlockedMax", "InterlockedMin") : ("InterlockedMin", "InterlockedMax");
        var at = $"{byteAddress} & ~3u";

        return $"{{ int _s{atomic.Result}; if (asfloat({value}) >= 0.0f) {{ {view}.{positive}({at}, asint({value}), _s{atomic.Result}); {atomic.Result} = asuint(_s{atomic.Result}); }} "
            + $"else {view}.{negative}({at}, {value}, {atomic.Result}); }}";
    }

    private void Buffer(AtomicOperation atomic, List<string> arguments, InstructionModifiers modifiers)
    {
        var resource = OperandParser.Parse(arguments[Resource]);
        var vaddr = OperandParser.Parse(arguments[Address]);
        var soffset = OperandParser.Parse(arguments[SOffset]);
        var byteAddress = BufferAddress.Build(_operands, vaddr, resource, soffset, modifiers);
        var inRange = BufferAddress.InRange(BufferAddress.Range(_operands, vaddr, resource, soffset, modifiers), 0);
        var view = BufferAddress.View(_operands, resource, true, byteAddress);

        var result = atomic.Result;

        // Out of range: no write, and the returned value is 0.
        _state.Emit($"  if (active && {inRange}) {{ TRACE_VIEW({view}.x, {view}.y, {_operands.Read(atomic.Data, 0)}); TRACE_VSTORE({_operands.Read(resource, 0)}, {_operands.Read(resource, 1)}, {byteAddress}, {_operands.Read(atomic.Data, 0)}); }}");
        _state.Emit($"  uint {result} = 0; if (active && {inRange}) {{ RWByteAddressBuffer _b{result} = ResourceDescriptorHeap[NonUniformResourceIndex(uav_slot({view}.x))];");
        _state.Emit($"    {Operation(atomic, atomic.Data, $"{view}.y", $"_b{result}")} }}");
    }

    /// <summary>image_atomic_*: false when the result is already written (cmpswap).</summary>
    private bool Image(string op, AtomicOperation atomic, List<string> arguments, InstructionModifiers modifiers)
    {
        if (modifiers.Has("a16") || modifiers.Has("d16"))
        {
            throw new UnsupportedProgramException($"{op} a16/d16");
        }

        var dimension = ImageDimensions.Name(modifiers);
        var (type, components) = ImageDimensions.Writable(dimension) ?? throw new UnsupportedProgramException($"{op} dim {dimension}");
        var coordinates = ImageDimensions.ReadAddress(_operands, arguments[Address]);
        var index = ImageDimensions.IntegerCoordinates(coordinates, components);

        var texture = _operands.Read(OperandParser.Parse(arguments[Resource]), 0);

        if (Wide(atomic.Kind, modifiers))
        {
            if (atomic.Kind == "sub")
            {
                throw new UnsupportedProgramException($"{op} 64-bit");
            }

            WideImage(atomic, type, index, texture, modifiers.Has("glc"));

            return false;
        }

        // Needs an integer-typed UAV view (R32_UINT/SINT) of the image.
        var element = atomic.Signed ? "int" : "uint";
        var result = atomic.Result;
        _state.Emit($"  uint {result} = 0; if (active) {{ {type}<{element}> _u{result} = ResourceDescriptorHeap[{NullTextureSlots.Slot(type, texture)}];");

        if (atomic.Kind == "cmpswap")
        {
            var swap = _operands.Read(atomic.Data, CompareValue);
            _state.Emit($"    InterlockedCompareExchange(_u{result}[{index}], {swap}, {_operands.Read(atomic.Data, 0)}, {result}); }}");

            if (modifiers.Has("glc"))
            {
                _operands.Write(atomic.Data, result);
            }

            return false;
        }

        if (atomic.Function == FloatOrder)
        {
            throw new UnsupportedProgramException($"{op} on an image");
        }

        var value = atomic.Signed ? $"asint({_operands.Read(atomic.Data)})" : _operands.Read(atomic.Data);

        if (atomic.Kind == "sub")
        {
            value = $"(0u - {value})";
        }

        _state.Emit($"    {atomic.Function}(_u{result}[{index}], {value}, {result}); }}");

        return true;
    }

    /// <summary>
    /// A 64-bit image atomic: dmask 0x3 (0xf for cmpswap, whose data is the new value then the comparand), on a 64-bit
    /// texel (an R32G32_UINT/SINT view, uint64_t in HLSL, SM 6.6 typed 64-bit atomics). Its data is two VGPRs, low
    /// dword first. Translated as a 32-bit atomic on the low dword before (MS/CS visibility passes' image_atomic_umax).
    /// </summary>
    private static bool Wide(string kind, InstructionModifiers modifiers)
    {
        var mask = modifiers.Hex("dmask", 1);

        if (kind == "cmpswap")
        {
            return mask == WideCompareMask;
        }

        return mask == WideMask;
    }

    private void WideImage(AtomicOperation atomic, string type, string index, string texture, bool returns)
    {
        var element = atomic.Signed ? "int64_t" : "uint64_t";
        var result = atomic.Result;
        var value = $"(({element})((uint64_t({_operands.Read(atomic.Data, 1)}) << 32) | {_operands.Read(atomic.Data, 0)}))";
        _state.Emit($"  {element} {result} = 0; if (active) {{ {type}<{element}> _u{result} = ResourceDescriptorHeap[{NullTextureSlots.Slot(type, texture)}];");

        if (atomic.Kind == "cmpswap")
        {
            var compare = $"(({element})((uint64_t({_operands.Read(atomic.Data, CompareHigh)}) << 32) | {_operands.Read(atomic.Data, CompareLow)}))";
            _state.Emit($"    InterlockedCompareExchange(_u{result}[{index}], {compare}, {value}, {result}); }}");
        }
        else
        {
            _state.Emit($"    {atomic.Function}(_u{result}[{index}], {value}, {result}); }}");
        }

        if (returns)
        {
            _operands.Write(atomic.Data, $"(uint)((uint64_t){result})", 0);
            _operands.Write(atomic.Data, $"(uint)(((uint64_t){result}) >> 32)", 1);
        }
    }

    [GeneratedRegex(@"^(buffer|image|global)_atomic_")]
    private static partial Regex AtomicPrefixPattern();
}
