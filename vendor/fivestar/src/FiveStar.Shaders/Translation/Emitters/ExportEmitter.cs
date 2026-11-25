// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// exp: writes an export target (mrtN, mrtz, posN, paramN, prim) into the internal <c>Exp x</c> registers, which the
/// entry point maps to its signature. The final pixel export (done vm) discards lanes whose EXEC bit is clear. Mesh
/// shaders write vertices and primitives directly (two-phase mode) or stage them in groupshared memory.
/// </summary>
internal sealed class ExportEmitter
{
    private const int Vec4 = 4;
    private const int CompressedRegisters = 2;

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;

    public ExportEmitter(TranslationState state, OperandAccess operands)
    {
        _state = state;
        _operands = operands;
    }

    public void Translate(string operandText, InstructionModifiers modifiers)
    {
        var arguments = ArgumentList.Split(operandText);
        var head = arguments[0].Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);
        var target = head[0];
        var sources = new List<string> { head[1] };
        sources.AddRange(arguments.Skip(1));

        if (_state.Stage == ShaderStage.Pixel && modifiers.Has("done") && modifiers.Has("vm"))
        {
            // The final pixel export: lanes with EXEC clear are killed.
            _state.Emit("  if (!active) discard;");
        }

        if (target == "null")
        {
            return;
        }

        if (_state.Stage == ShaderStage.Mesh && target == "prim")
        {
            Primitive(sources[0]);

            return;
        }

        var values = Values(sources, modifiers.Has("compr"));
        _state.Outputs.Add(target);
        WriteTarget(target, sources, values);
    }

    private void WriteTarget(string target, List<string> sources, List<string> values)
    {
        var vector = string.Join(", ", values);

        if (target == "mrtz")
        {
            _state.Emit($"  x.mrtz = {values[0]};");

            return;
        }

        if (_state.Stage == ShaderStage.Mesh && _state.MeshYield)
        {
            // Direct output: the placeholder is expanded with this target's signature mapping when emitting.
            _state.Emit($"  if (active) {{ float4 _e = float4({vector}); /*MSEXP:{target}*/ }}");

            return;
        }

        if (_state.Stage == ShaderStage.Mesh)
        {
            StageMeshExport(target, sources, vector);

            return;
        }

        _state.Emit($"  x.{target} = float4({vector});");
    }

    /// <summary>
    /// Mesh primitive export: three 9-bit vertex indices at bits 0/10/20, bit 31 = culled. A direct export culls a
    /// primitive that names a vertex past the group's vertex count and keeps its indices inside it: a garbage index hung
    /// NVIDIA's mesh pipeline (VIctory #587 bounds the outputs the same way).
    /// </summary>
    private void Primitive(string source)
    {
        var value = _operands.Read(OperandParser.Parse(source));

        if (_state.MeshYield)
        {
            int vertices = new Stages.MeshDeclarations(_state).MaxVertices;
            _state.Emit($"  if (active) {{ uint _p = {value}; uint3 _i = uint3(_p & 0x1ffu, (_p >> 10) & 0x1ffu, (_p >> 20) & 0x1ffu); "
                + $"uint _n = max(min(g_ms_nv, {vertices}u), 1u); tris[in_gi] = min(_i, _n - 1u); "
                + "prims[in_gi].cull = (_p >> 31) != 0 || any(_i >= _n); }");
        }
        else
        {
            _state.Emit($"  if (active) g_ms_prim[in_gi] = {value};");
        }
    }

    private List<string> Values(List<string> sources, bool compressed)
    {
        var values = new List<string>();

        if (!compressed)
        {
            foreach (var source in sources.Take(Vec4))
            {
                var value = source == "off" ? "0" : $"asfloat({_operands.Read(OperandParser.Parse(source))})";
                values.Add(value);
            }

            return values;
        }

        // compr: each register holds two f16 values. The disassembler prints a compressed export as
        // "src0, src0, src1, src1", so the registers are the first and the third operand.
        foreach (var source in sources.Where((_, index) => index % CompressedRegisters == 0).Take(CompressedRegisters))
        {
            if (source == "off")
            {
                values.AddRange(["0", "0"]);
                continue;
            }

            var packed = _operands.Read(OperandParser.Parse(source));
            values.AddRange([$"f16tof32({packed})", $"f16tof32({packed} >> 16)"]);
        }

        return values;
    }

    /// <summary>Stages a per-vertex export in groupshared memory, only as wide as written.</summary>
    private void StageMeshExport(string target, List<string> sources, string vector)
    {
        var width = 1;
        var raw = sources.Take(Vec4).ToList();

        for (var component = 0; component < raw.Count; component++)
        {
            if (raw[component] != "off")
            {
                width = Math.Max(width, component + 1);
            }
        }

        var known = _state.MeshWidths.GetValueOrDefault(target, 1);
        _state.MeshWidths[target] = Math.Max(known, width);
        _state.Emit($"  if (active) g_ms_exp[in_gi].{target} = MSW_{target}(float4({vector}));");
    }
}
