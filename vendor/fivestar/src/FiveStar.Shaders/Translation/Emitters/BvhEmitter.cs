// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// image_bvh_intersect_ray / image_bvh64_intersect_ray vdata[4], vaddr, T#: calls the rdna_bvh.hlsli emulation.
/// The address holds the node pointer (1 or 2 dwords), ray extent, origin, direction and inverse direction (f32; with
/// a16 direction and inverse direction are three packed f16 pairs). T# dword0 is the BVH base address >> 8.
/// </summary>
internal sealed class BvhEmitter
{
    private const int AddressOperand = 1;
    private const int DescriptorOperand = 2;
    private const int Vector3 = 3;
    private const int DescriptorDwords = 4;
    private const string ComponentNames = "xyzw";

    // Address dwords needed: node pointer (1 or 2) + extent + origin (3) + direction and inverse direction (6, or 3 with a16).
    private const int Address32 = 11;
    private const int Address64 = 12;
    private const int Address32Half = 8;
    private const int Address64Half = 9;

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;

    public BvhEmitter(TranslationState state, OperandAccess operands)
    {
        _state = state;
        _operands = operands;
    }

    public void Translate(string op, List<string> arguments, InstructionModifiers modifiers)
    {
        var destination = OperandParser.Parse(arguments[Destination]);
        var address = ImageDimensions.ReadAddress(_operands, arguments[AddressOperand]);
        var is64 = op.StartsWith("image_bvh64", StringComparison.Ordinal);
        var halves = modifiers.Has("a16");
        var needed = AddressSize(is64, halves);

        if (address.Count < needed)
        {
            throw new UnsupportedProgramException($"{op} with {address.Count} address dwords");
        }

        var descriptor = OperandParser.Parse(arguments[DescriptorOperand]);
        var extent = is64 ? PairDwords : 1;
        var origin = Vector(address.Skip(extent + 1).Take(Vector3).Select(value => $"asfloat({value})"));
        var (direction, inverse) = Directions(address, extent + 1 + Vector3, halves);
        _state.UsesBvh = true;

        var result = $"_bvh{_state.NextTemporary()}";
        _state.Emit($"  uint4 {result} = uint4(0xffffffffu, 0xffffffffu, 0xffffffffu, 0xffffffffu);");
        var words = Enumerable.Range(0, DescriptorDwords).Select(dword => _operands.Read(descriptor, dword)).ToList();
        var pointerHigh = is64 ? address[1] : "0u";
        _state.Emit($"  if (active) {result} = bvh_intersect_ray(uint4({string.Join(", ", words)}), "
            + $"{address[0]}, {pointerHigh}, asfloat({address[extent]}), {origin}, {direction}, {inverse});");

        for (var component = 0; component < DescriptorDwords; component++)
        {
            _operands.Write(destination, $"{result}.{ComponentNames[component]}", component);
        }
    }

    private static int AddressSize(bool is64, bool halves)
    {
        if (halves)
        {
            var packed = is64 ? Address64Half : Address32Half;

            return packed;
        }

        var full = is64 ? Address64 : Address32;

        return full;
    }

    private static (string Direction, string Inverse) Directions(List<string> address, int start, bool halves)
    {
        if (!halves)
        {
            var direction = Vector(address.Skip(start).Take(Vector3).Select(value => $"asfloat({value})"));
            var inverse = Vector(address.Skip(start + Vector3).Take(Vector3).Select(value => $"asfloat({value})"));

            return (direction, inverse);
        }

        // a16: dir.x | dir.y, dir.z | inv.x, inv.y | inv.z.
        var packed = address.Skip(start).Take(Vector3).ToList();
        var values = packed.SelectMany(value => (string[])[$"f16tof32({value})", $"f16tof32({value} >> 16)"]).ToList();

        return (Vector(values.Take(Vector3)), Vector(values.Skip(Vector3)));
    }

    private static string Vector(IEnumerable<string> components)
    {
        return $"float3({string.Join(", ", components)})";
    }
}
