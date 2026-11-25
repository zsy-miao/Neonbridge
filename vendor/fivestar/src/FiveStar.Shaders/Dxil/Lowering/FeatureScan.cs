// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Container;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// The shader flags a module's instructions need, as the validator recomputes them: 64-bit integers, doubles, and the
/// double extensions (double division, fma, and conversions between doubles and integers).
/// </summary>
public static class FeatureScan
{
    public static ShaderFlagBits Flags(IrFunction main)
    {
        var flags = ShaderFlagBits.None;

        foreach (var block in main.Blocks)
        {
            foreach (var instruction in block.Body)
            {
                flags |= Of(instruction);
            }

            foreach (var phi in block.Phis)
            {
                flags |= Of(phi);
            }
        }

        return flags;
    }

    private static ShaderFlagBits Of(Instruction instruction)
    {
        var flags = Of(instruction.Type);

        foreach (var operand in instruction.Operands)
        {
            flags |= Of(operand.Type);
        }

        flags |= OperationFlags(instruction);

        if ((flags & ShaderFlagBits.EnableDoublePrecision) != 0 && NeedsExtensions(instruction))
        {
            flags |= ShaderFlagBits.EnableDoubleExtensions;
        }

        return flags;
    }

    private static ShaderFlagBits Of(IrType type)
    {
        if (type.Kind == IrTypeKind.Integer && type.Bits == sizeof(long) * BitsPerByte)
        {
            return ShaderFlagBits.Int64Ops;
        }

        if (type.Kind == IrTypeKind.Double)
        {
            return ShaderFlagBits.EnableDoublePrecision;
        }

        return ShaderFlagBits.None;
    }

    /// <summary>Wave operations and descriptor-heap handles, from the calls the module keeps.</summary>
    private static ShaderFlagBits OperationFlags(Instruction instruction)
    {
        if (instruction.Opcode != Opcode.Call || instruction.Operands.Count == 0 || instruction.Operands[0] is not Constant code)
        {
            return ShaderFlagBits.None;
        }

        var operation = (DxOpCode)code.Bits;

        if (operation == DxOpCode.CreateHandleFromHeap)
        {
            var heap = instruction.Operands[SamplerOperand] as Constant;
            var sampler = heap is not null && heap.Bits != 0;

            return Choice.Of(sampler, ShaderFlagBits.SamplerDescriptorHeapIndexing, ShaderFlagBits.ResourceDescriptorHeapIndexing);
        }

        // A 64-bit atomic on a texel (the backend makes no other): its resource comes from the descriptor heap.
        if (operation is DxOpCode.AtomicBinOp or DxOpCode.AtomicCompareExchange && instruction.Type.Kind == IrTypeKind.Integer && instruction.Type.Bits == sizeof(long) * BitsPerByte)
        {
            return ShaderFlagBits.AtomicInt64OnTypedResource | ShaderFlagBits.AtomicInt64OnHeapResource;
        }

        if (WaveOperations.Contains(operation))
        {
            return ShaderFlagBits.WaveOps;
        }

        return TextureFlags(operation, instruction);
    }

    /// <summary>SampleCmpLevel, and texel offsets that are not constants (DxilShaderFlags: advanced texture ops).</summary>
    private static ShaderFlagBits TextureFlags(DxOpCode operation, Instruction instruction)
    {
        if (operation == DxOpCode.SampleCmpLevel)
        {
            return ShaderFlagBits.AdvancedTextureOps;
        }

        if (!OffsetOperands.TryGetValue(operation, out var offsets))
        {
            return ShaderFlagBits.None;
        }

        var (first, count) = offsets;

        for (var index = first; index < first + count; index++)
        {
            if (instruction.Operands[index] is not Constant)
            {
                return ShaderFlagBits.AdvancedTextureOps;
            }
        }

        return ShaderFlagBits.None;
    }

    private static bool NeedsExtensions(Instruction instruction)
    {
        if (instruction.Opcode == Opcode.Binary)
        {
            return instruction.Code is (int)BinaryOp.SDiv or (int)BinaryOp.SRem && instruction.Type.Kind == IrTypeKind.Double;
        }

        if (instruction.Opcode == Opcode.Cast)
        {
            return instruction.Code is (int)CastOp.FpToSi or (int)CastOp.FpToUi or (int)CastOp.SiToFp or (int)CastOp.UiToFp;
        }

        return instruction.Opcode == Opcode.Call && instruction.Operands.Count > 0
            && instruction.Operands[0] is Constant code && code.Bits == (long)DxOpCode.Fma;
    }

    private const int BitsPerByte = 8;
    private const int SamplerOperand = 2;

    // The texel offset operands of the texture operations (after the opcode, handle, sampler and coordinates). Gathers
    // are not here: their offsets may be computed since shader model 5, and the validator rejects the flag for them.
    private static readonly Dictionary<DxOpCode, (int First, int Count)> OffsetOperands = new()
    {
        [DxOpCode.Sample] = (7, 3),
        [DxOpCode.SampleBias] = (7, 3),
        [DxOpCode.SampleLevel] = (7, 3),
        [DxOpCode.SampleGrad] = (7, 3),
        [DxOpCode.SampleCmp] = (7, 3),
        [DxOpCode.SampleCmpLevelZero] = (7, 3),
        [DxOpCode.TextureLoad] = (6, 3),
    };

    private static readonly HashSet<DxOpCode> WaveOperations =
    [
        DxOpCode.WaveIsFirstLane, DxOpCode.WaveGetLaneIndex, DxOpCode.WaveGetLaneCount, DxOpCode.WaveAnyTrue,
        DxOpCode.WaveAllTrue, DxOpCode.WaveActiveAllEqual, DxOpCode.WaveActiveBallot, DxOpCode.WaveReadLaneAt,
        DxOpCode.WaveReadLaneFirst, DxOpCode.WaveActiveOp, DxOpCode.WaveActiveBit, DxOpCode.WavePrefixOp,
        DxOpCode.QuadReadLaneAt, DxOpCode.QuadOp, DxOpCode.WaveAllBitCount, DxOpCode.WavePrefixBitCount,
    ];
}
