// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Emitters;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>The per-family emitters of one program translation.</summary>
internal sealed record InstructionEmitters
{
    public required InterpolationEmitter Interpolation { get; init; }

    public required CrossLaneEmitter CrossLane { get; init; }

    public required LdsEmitter Lds { get; init; }

    public required MessageEmitter Messages { get; init; }

    public required AtomicEmitter Atomics { get; init; }

    public required ExportEmitter Exports { get; init; }

    public required ImageEmitter Images { get; init; }

    public required DppRewriter Dpp { get; init; }

    public required SdwaEmitter Sdwa { get; init; }

    public required ValuEmitter Valu { get; init; }

    public required BufferEmitter Buffers { get; init; }

    public required GlobalMemoryEmitter Global { get; init; }

    public required ScalarMemoryEmitter ScalarMemory { get; init; }

    public required SaluEmitter Salu { get; init; }

    /// <summary>Builds every emitter over one translation state.</summary>
    public static InstructionEmitters Create(TranslationState state, OperandAccess operands)
    {
        var crossLane = new CrossLaneEmitter(state, operands);
        var halves = new HalfPrecisionEmitter(operands, new PackedMathEmitter(operands));
        var extended = new ValuExtendedEmitter(
            new WideIntegerEmitter(state, operands), crossLane, new IndexedMoveEmitter(state, operands), halves);
        var valu = new ValuEmitter(operands, extended, new VectorCompareEmitter(operands));
        var images = new ImageEmitter(state, operands, new ImageSampleBuilder(state, operands), new BvhEmitter(state, operands));
        var salu = new SaluEmitter(
            state, operands, new ScalarLogicEmitter(state, operands), new ScalarArithmeticEmitter(state, operands));

        return new InstructionEmitters
        {
            Interpolation = new InterpolationEmitter(state, operands),
            CrossLane = crossLane,
            Lds = new LdsEmitter(state, operands, new LdsAtomicEmitter(state, operands)),
            Messages = new MessageEmitter(state),
            Atomics = new AtomicEmitter(state, operands),
            Exports = new ExportEmitter(state, operands),
            Images = images,
            Dpp = new DppRewriter(state, operands),
            Sdwa = new SdwaEmitter(state, operands, valu),
            Valu = valu,
            Buffers = new BufferEmitter(state, operands),
            Global = new GlobalMemoryEmitter(state, operands),
            ScalarMemory = new ScalarMemoryEmitter(state, operands),
            Salu = salu,
        };
    }
}
