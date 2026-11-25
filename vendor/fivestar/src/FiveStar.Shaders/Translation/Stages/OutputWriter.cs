// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Stages;

/// <summary>
/// Writes the internal export struct (Exp: one float4 per export target) and the entry point's Out struct, with the
/// statements that copy exports to Out members: paramN to the N-th non-system output register, pos0 to SV_Position,
/// pos1.. to clip/cull distances, mrtN to SV_TargetN with the signature's type, mrtz to SV_Depth.
/// </summary>
internal sealed class OutputWriter
{
    private const string ParamPrefix = "param";
    private const string PositionPrefix = "pos";
    private const string TargetPrefix = "mrt";

    // Packed-f16 linkage: the vertex stage also exports unpacked halves when the interpolant budget allows (mesh
    // shaders stage every export in groupshared memory, so only for tiny vertex formats).
    private const int VertexHalfParamLimit = 10;
    private const int MeshHalfParamLimit = 2;

    private readonly TranslationState _state;

    public OutputWriter(TranslationState state)
    {
        _state = state;
    }

    /// <summary>The Out register groups of a vertex or mesh shader (empty for other stages).</summary>
    public List<List<SignatureElement>> OutputRegisters()
    {
        if (_state.Stage is not ShaderStage.Vertex and not ShaderStage.Mesh)
        {
            return [];
        }

        return SignatureLayout.Registers(_state.Signatures.Outputs, SignatureLayout.VertexSystemOutputs);
    }

    public void Write(List<string> lines)
    {
        // Numeric order (param2 before param10): dxc assigns output registers in declaration order, and a pixel
        // shader links only when each of its inputs sits in the same register as the vertex output.
        var targets = _state.Outputs.OrderBy(TargetPrefixOf, StringComparer.Ordinal).ThenBy(TargetNumber).ToList();
        lines.Add("struct Exp {");

        foreach (var target in targets)
        {
            var member = target == "mrtz" ? "  float mrtz;" : $"  float4 {target};";
            lines.Add(member);
        }

        if (targets.Count == 0)
        {
            lines.Add("  float4 none;");
        }

        lines.Add("};");
        lines.Add("struct Out {");
        var context = new OutputContext(this, targets);

        foreach (var target in targets)
        {
            WriteTarget(lines, context, target);
        }

        // The unpacked f16 halves follow every parameter, so the parameters keep the registers of the signature.
        lines.AddRange(context.HalfLines);

        if (targets.Count == 0)
        {
            lines.Add("  float4 dummy : SV_Target0;");
        }

        lines.Add("};");
    }

    /// <summary>The target name without its number (<c>param</c>, <c>pos</c>, <c>mrt</c>, <c>mrtz</c>).</summary>
    private static string TargetPrefixOf(string target)
    {
        return target.TrimEnd("0123456789".ToCharArray());
    }

    private static int TargetNumber(string target)
    {
        var prefix = TargetPrefixOf(target);

        if (prefix.Length == target.Length)
        {
            return 0;
        }

        return Number(target, prefix);
    }

    private void WriteTarget(List<string> lines, OutputContext context, string target)
    {
        var map = _state.OutputMap;

        if (target == "mrtz")
        {
            lines.Add("  float mrtz : SV_Depth;");
            map.Add("  o.mrtz = x.mrtz;");
        }
        else if (target.StartsWith(ParamPrefix, StringComparison.Ordinal))
        {
            WriteParam(lines, context, target);
        }
        else if (target == "pos0")
        {
            lines.Add("  float4 pos0 : SV_Position;");
            map.Add("  o.pos0 = x.pos0;");
        }
        else if (target.StartsWith(PositionPrefix, StringComparison.Ordinal))
        {
            WritePosition(lines, context, target);
        }
        else if (target.StartsWith(TargetPrefix, StringComparison.Ordinal) && target.Length > TargetPrefix.Length && target[TargetPrefix.Length..].All(char.IsAsciiDigit))
        {
            WriteRenderTarget(lines, context, target);
        }
        else
        {
            throw new UnsupportedProgramException($"export target {target}");
        }
    }

    private void WriteParam(List<string> lines, OutputContext context, string target)
    {
        var number = Number(target, ParamPrefix);
        var map = _state.OutputMap;

        if (number < context.Registers.Count)
        {
            foreach (var element in context.Registers[number])
            {
                var mask = element.Mask;
                var type = SignatureLayout.TypeName(element);
                lines.Add($"  {type}{SignatureLayout.Width(mask)} p{number}_{mask} : {element.Name}{element.SemanticIndex};");
                map.Add($"  o.p{number}_{mask} = {SignatureLayout.FromFloat(element, $"x.{target}.{mask}")};");
            }
        }
        else if (context.Registers.Count == 0)
        {
            lines.Add($"  float4 {target} : TEXCOORD{number};");
            map.Add($"  o.{target} = x.{target};");
        }

        if (context.HalfCopies)
        {
            context.HalfLines.Add($"  float4 {target}lo : ATTRLO{number};");
            context.HalfLines.Add($"  float4 {target}hi : ATTRHI{number};");
            map.Add($"  o.{target}lo = f16tof32(asuint(x.{target})); o.{target}hi = f16tof32(asuint(x.{target}) >> 16);");
        }
    }

    private void WritePosition(List<string> lines, OutputContext context, string target)
    {
        // pos1.. after an optional misc vector (layer / viewport) hold the clip and cull distance registers.
        var number = Number(target, PositionPrefix);
        var clip = number - 1;

        if (context.HasMiscVector)
        {
            clip--;
        }

        if (clip >= 0 && clip < context.ClipRegisters.Count)
        {
            var element = context.ClipRegisters[clip];
            lines.Add($"  float4 {target} : {element.SystemValue}{element.SemanticIndex};");
        }
        else
        {
            // Misc vector (point size / layer / viewport): PA_CL_VS_OUT_CNTL is not decoded; a plain interpolant.
            lines.Add($"  float4 {target} : POSEXP{number};");
        }

        _state.OutputMap.Add($"  o.{target} = x.{target};");
    }

    private void WriteRenderTarget(List<string> lines, OutputContext context, string target)
    {
        var number = Number(target, TargetPrefix);
        context.TargetTypes.TryGetValue(number, out var element);
        var type = SignatureLayout.TypeName(element);
        lines.Add($"  {type}4 {target} : SV_Target{number};");
        var value = type == "float" ? $"x.{target}" : $"as{type}(x.{target})";
        _state.OutputMap.Add($"  o.{target} = {value};");
    }

    private static int Number(string target, string prefix)
    {
        return int.Parse(target[prefix.Length..], CultureInfo.InvariantCulture);
    }

    /// <summary>What the Out struct is built from.</summary>
    private sealed class OutputContext
    {
        public OutputContext(OutputWriter writer, List<string> targets)
        {
            var state = writer._state;
            var outputs = state.Signatures.Outputs;
            var paramCount = targets.Count(target => target.StartsWith(ParamPrefix, StringComparison.Ordinal));
            var limit = state.Stage == ShaderStage.Vertex ? VertexHalfParamLimit : MeshHalfParamLimit;
            var vertexLike = state.Stage is ShaderStage.Vertex or ShaderStage.Mesh;
            HalfCopies = vertexLike && paramCount > 0 && paramCount <= limit;

            Registers = writer.OutputRegisters();
            ClipRegisters = ClipDistances(outputs);
            HasMiscVector = outputs.Any(element => element.SystemValue is "SV_RenderTargetArrayIndex" or "SV_ViewportArrayIndex");
            TargetTypes = [];

            if (state.Stage == ShaderStage.Pixel)
            {
                foreach (var element in outputs.Where(element => element.SystemValue == "SV_Target"))
                {
                    TargetTypes[element.SemanticIndex] = element;
                }
            }
        }

        public bool HalfCopies { get; }

        /// <summary>Gets the Out members of the unpacked halves, written after all other members.</summary>
        public List<string> HalfLines { get; } = [];

        public List<List<SignatureElement>> Registers { get; }

        public List<SignatureElement> ClipRegisters { get; }

        public bool HasMiscVector { get; }

        public Dictionary<int, SignatureElement> TargetTypes { get; }

        /// <summary>One clip/cull element per register (the last one listed), in register order.</summary>
        private static List<SignatureElement> ClipDistances(IReadOnlyList<SignatureElement> outputs)
        {
            var byRegister = new Dictionary<int, SignatureElement>();

            foreach (var element in outputs)
            {
                if (element.SystemValue is "SV_ClipDistance" or "SV_CullDistance")
                {
                    byRegister[element.Register] = element;
                }
            }

            return byRegister.Values.OrderBy(element => element.Register).ToList();
        }
    }
}
