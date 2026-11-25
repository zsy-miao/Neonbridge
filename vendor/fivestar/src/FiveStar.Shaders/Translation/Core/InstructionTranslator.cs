// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>Translates one non-branch instruction: picks its family and hands it to that family's emitter.</summary>
internal sealed partial class InstructionTranslator
{
    // Instructions with no effect in this model: waits, hints, cache control, mode and priority changes.
    private static readonly HashSet<string> Ignored =
    [
        "s_waitcnt", "s_nop", "s_inst_prefetch", "s_endpgm", "s_code_end", "s_waitcnt_vscnt", "s_waitcnt_vmcnt",
        "s_waitcnt_lgkmcnt", "s_waitcnt_expcnt", "s_clause", "s_dcache_inv", "s_setprio", "s_denorm_mode", "s_round_mode",
        "v_nop", "s_setreg_b32", "s_setreg_imm32_b32", "s_gl1_inv", "s_dcache_wb", "s_icache_inv", "s_sleep", "s_ttracedata",
    ];

    // Operations that take op_sel on 16-bit halves; anything else must have an all-zero op_sel.
    private static readonly string[] HalfSelectPrefixes = ["v_fma_mix", "v_pack_b32_f16", "v_mad_mix", "v_pk_", "v_permlane"];

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;
    private readonly InstructionEmitters _emitters;

    public InstructionTranslator(TranslationState state, OperandAccess operands, InstructionEmitters emitters)
    {
        _state = state;
        _operands = operands;
        _emitters = emitters;
    }

    public void Translate(string op, string operands)
    {
        switch (Family(op))
        {
            case InstructionFamily.Interpolation:
                _emitters.Interpolation.Translate(op, operands);
                break;
            case InstructionFamily.Swizzle:
                _emitters.CrossLane.Swizzle(operands);
                break;
            case InstructionFamily.Lds:
                _emitters.Lds.Translate(op, operands);
                break;
            case InstructionFamily.Barrier:
                Barrier();
                break;
            case InstructionFamily.Message:
                _emitters.Messages.Translate(operands);
                break;
            default:
                TranslateWithModifiers(op, operands);
                break;
        }
    }

    private static InstructionFamily Family(string op)
    {
        if (op.StartsWith("v_interp", StringComparison.Ordinal))
        {
            return InstructionFamily.Interpolation;
        }

        if (op == "ds_swizzle_b32")
        {
            return InstructionFamily.Swizzle;
        }

        if (op.StartsWith("ds_", StringComparison.Ordinal))
        {
            return InstructionFamily.Lds;
        }

        switch (op)
        {
            case "s_barrier":
                return InstructionFamily.Barrier;
            case "s_sendmsg":
                return InstructionFamily.Message;
            default:
                return InstructionFamily.Other;
        }
    }

    /// <summary>s_barrier: a group barrier in compute and mesh shaders; graphics waves run alone.</summary>
    private void Barrier()
    {
        if (_state.Stage is ShaderStage.Compute or ShaderStage.Mesh)
        {
            _state.Emit("  GroupMemoryBarrierWithGroupSync();");
        }
    }

    private void TranslateWithModifiers(string op, string operands)
    {
        if (Ignored.Contains(op))
        {
            return;
        }

        var modifiers = InstructionModifiers.Parse(operands);
        var operandText = InstructionModifiers.Strip(operands);

        if (op == "s_getreg_b32")
        {
            // Mode and hardware registers read as their defaults.
            _operands.Write(OperandParser.Parse(ArgumentList.Split(operands)[0]), "0u");

            return;
        }

        CheckModifiers(op, modifiers);

        if (op.StartsWith("buffer_atomic_", StringComparison.Ordinal) || op.StartsWith("global_atomic_", StringComparison.Ordinal)
            || op.StartsWith("image_atomic_", StringComparison.Ordinal))
        {
            _emitters.Atomics.Translate(op, operandText, modifiers);

            return;
        }

        TranslateMemoryOrAlu(op, operandText, modifiers);
    }

    private static void CheckModifiers(string op, InstructionModifiers modifiers)
    {
        var halfAddress = modifiers.Has("a16") && !op.StartsWith("image_", StringComparison.Ordinal);

        if (modifiers.Has("tfe") || modifiers.Has("lwe") || halfAddress)
        {
            var keys = modifiers.Keys.Order(StringComparer.Ordinal).Select(key => $"'{key}'");

            throw new UnsupportedProgramException($"{op} modifiers [{string.Join(", ", keys)}]");
        }
    }

    private void TranslateMemoryOrAlu(string op, string operandText, InstructionModifiers modifiers)
    {
        string[] halfDataPrefixes = ["image_", "buffer_", "tbuffer_"];

        if (modifiers.Has("d16") && !halfDataPrefixes.Any(prefix => op.StartsWith(prefix, StringComparison.Ordinal)))
        {
            throw new UnsupportedProgramException($"{op} d16");
        }

        if (op.StartsWith("exp", StringComparison.Ordinal))
        {
            _emitters.Exports.Translate(operandText, modifiers);

            return;
        }

        if (op.StartsWith("image_", StringComparison.Ordinal))
        {
            _emitters.Images.Translate(op, operandText, modifiers);

            return;
        }

        TranslateOperands(op, operandText, modifiers);
    }

    private void TranslateOperands(string op, string operandText, InstructionModifiers modifiers)
    {
        var raw = ArgumentList.Split(operandText);

        for (var index = 0; index < raw.Count; index++)
        {
            var match = SignExtendPattern().Match(raw[index]);

            if (match.Success)
            {
                raw[index] = match.Groups[1].Value;
                modifiers.SignExtended.Add(index);
            }
        }

        IReadOnlyList<Operand> a = raw.Select(OperandParser.Parse).ToList();
        var operation = EncodingSuffixPattern().Replace(op, string.Empty);

        if (operation.EndsWith("_dpp", StringComparison.Ordinal) || modifiers.Has("dpp8"))
        {
            operation = DppSuffixPattern().Replace(operation, string.Empty);
            a = _emitters.Dpp.Apply(a, modifiers);
        }

        operation = SdwaSuffixPattern().Replace(operation, string.Empty);
        string[] sdwaKeys = ["dst_sel", "src0_sel", "src1_sel"];

        if (sdwaKeys.Any(modifiers.Has) || modifiers.SignExtended.Count > 0)
        {
            _emitters.Sdwa.Translate(operation, a, modifiers);

            return;
        }

        Dispatch(operation, a, modifiers);
    }

    private void Dispatch(string operation, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        if (operation.StartsWith("v_", StringComparison.Ordinal))
        {
            TranslateVector(operation, a, modifiers);

            return;
        }

        string[] formatted = ["tbuffer_load_format", "buffer_load_format", "tbuffer_store_format", "buffer_store_format"];
        string[] global = ["global_load_", "global_store_", "flat_load_", "flat_store_"];

        if (formatted.Any(prefix => operation.StartsWith(prefix, StringComparison.Ordinal)))
        {
            _emitters.Buffers.TranslateFormatted(operation, a, modifiers);
        }
        else if (global.Any(prefix => operation.StartsWith(prefix, StringComparison.Ordinal)))
        {
            _emitters.Global.Translate(operation, a, modifiers);
        }
        else if (operation.StartsWith("s_load_", StringComparison.Ordinal) || operation.StartsWith("s_buffer_load_", StringComparison.Ordinal))
        {
            _emitters.ScalarMemory.Translate(operation, a);
        }
        else if (operation.StartsWith("buffer_load_", StringComparison.Ordinal) || operation.StartsWith("buffer_store_", StringComparison.Ordinal))
        {
            _emitters.Buffers.Translate(operation, a, modifiers);
        }
        else if (operation.StartsWith("s_", StringComparison.Ordinal))
        {
            _emitters.Salu.Translate(operation, a);
        }
        else
        {
            throw new UnsupportedProgramException(operation);
        }
    }

    private void TranslateVector(string operation, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var select = modifiers.Value("op_sel");
        var halfOperation = HalfSelectPrefixes.Any(prefix => operation.StartsWith(prefix, StringComparison.Ordinal))
            || HalfSuffixPattern().IsMatch(operation);

        if (modifiers.Has("op_sel") && !halfOperation && select is not null && select.Trim('[', ']').Replace(",", string.Empty, StringComparison.Ordinal).Trim('0').Length > 0)
        {
            throw new UnsupportedProgramException($"{operation} op_sel");
        }

        try
        {
            _emitters.Valu.Translate(operation, a, modifiers);
        }
        finally
        {
            _state.DppKeep = null;
        }
    }

    [GeneratedRegex(@"^sext\((.*)\)$")]
    private static partial Regex SignExtendPattern();

    [GeneratedRegex(@"_e(32|64)$")]
    private static partial Regex EncodingSuffixPattern();

    [GeneratedRegex(@"_dpp$")]
    private static partial Regex DppSuffixPattern();

    [GeneratedRegex(@"_sdwa$")]
    private static partial Regex SdwaSuffixPattern();

    [GeneratedRegex(@"_[fiu]16$")]
    private static partial Regex HalfSuffixPattern();
}
