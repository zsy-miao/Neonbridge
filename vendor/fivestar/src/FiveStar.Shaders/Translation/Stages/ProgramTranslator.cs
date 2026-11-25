// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Translation.Core;
using FiveStar.Shaders.Translation.Waves;

namespace FiveStar.Shaders.Translation.Stages;

/// <summary>Translates one instruction stream of a program into a complete HLSL program.</summary>
internal static class ProgramTranslator
{
    /// <summary>Translates <paramref name="instructions"/> as <paramref name="stage"/>; a copy shader is emitted as a function.</summary>
    public static string Translate(
        ShaderProgram program, IReadOnlyList<Instruction> instructions, ShaderStage stage, bool copyShader, TranslationOptions options)
    {
        // Compute programs pin their wave size instead; geometry programs run one lane per HLSL invocation.
        var emulate = options.EmulateWaveSize && !copyShader && stage is ShaderStage.Pixel or ShaderStage.Vertex or ShaderStage.Mesh;

        return Emit(coherent => new TranslationState(program, instructions, stage, copyShader) { EmulateWaves = emulate, FastWaves = options.FastWaves, TypedImageViews = options.TypedImageViews, Coherent = coherent, Loops = options.Loops ?? LoopCaps.Default }, options);
    }

    /// <summary>
    /// The superstep form of a mesh or compute program (see Waves.SuperstepSplitter): console results on any hardware
    /// wave.
    /// </summary>
    public static string TranslateSupersteps(ShaderProgram program, TranslationOptions options)
    {
        return Emit(
            coherent => new TranslationState(program, program.Instructions, program.Stage, false)
            {
                EmulateWaves = true, Supersteps = true, SuperstepCarry = options.SuperstepCarry, Coherent = coherent,
                Loops = options.Loops ?? LoopCaps.Default, RoundsPerStop = options.RoundsPerStop, TypedImageViews = options.TypedImageViews,
            },
            options with { StructuredControlFlow = false });
    }

    private static string Emit(Func<IReadOnlySet<RegisterKey>, TranslationState> create, TranslationOptions options)
    {
        // Mask tracking follows emission order, not paths: a register that is a lane mask on one path and an integer on
        // another is read in whichever form the last emitted write left. A first translation finds those registers; the
        // second keeps them in their integer variables, which every mask write also sets (OperandAccess.ShadowInteger).
        var state = create(new HashSet<RegisterKey>());
        Run(state);
        var candidates = state.MasksEver
            .Where(key => key != SpecialRegisters.Exec)
            .Where(key => state.IntegerUses.Contains(key) || (state.IsWave64 && state.IntegerUses.Contains(key.Plus(1))))
            .ToHashSet();
        var mixed = MaskFormAnalysis.Mixed(state, candidates);

        if (mixed.Count > 0)
        {
            state = create(mixed);
            Run(state);
        }

        if (state.Supersteps)
        {
            // Every wave operation is a group round in this form: those whose result nothing reads go first.
            DeadWaveOperations.Remove(state.Blocks, SuperstepWriter.Pinned(state, new MeshDeclarations(state)));
            var splitter = new SuperstepSplitter();
            var blocks = splitter.Split(state.Blocks);

            state.Blocks.Clear();
            state.Blocks.AddRange(blocks);
            state.Hoisted.AddRange(splitter.Hoisted);
            state.SuperstepValues = splitter.UsesValues;
        }

        var mesh = new MeshDeclarations(state);
        var body = new BodyWriter(state, mesh, new StructuredBodyWriter(state), options.StructuredControlFlow);
        var emitter = new ProgramEmitter(state, new OutputWriter(state), mesh, body);

        return emitter.Emit();
    }

    private static void Run(TranslationState state)
    {
        var operands = new OperandAccess(state, new ImplicitInputs(state));
        var translator = new InstructionTranslator(state, operands, InstructionEmitters.Create(state, operands));
        new ControlFlowTranslator(state, operands, translator).Run();
    }
}
