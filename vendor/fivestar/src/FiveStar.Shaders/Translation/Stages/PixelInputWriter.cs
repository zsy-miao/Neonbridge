// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Stages;

/// <summary>
/// Writes the PSIn struct of a pixel shader and the statements that assemble each attrN from it. An attribute read
/// only through v_interp_mov p0 is nointerpolation; one read per vertex uses GetAttributeAtVertex.
/// </summary>
internal sealed class PixelInputWriter
{
    private const string VertexModePrefix = "vtx";
    private const int HalfMembers = 2;

    // D3D12's pixel-shader input registers, one of them taken by SV_Position.
    private const int InputRegisters = 32;
    private const int PositionRegister = 1;

    private readonly TranslationState _state;

    public PixelInputWriter(TranslationState state)
    {
        _state = state;
    }

    /// <summary>
    /// Every signature register is declared, read or not, and the f16 halves come after all of them (pairs from
    /// attribute 0 up to the highest one read), so each input sits in the register the vertex stage writes it to:
    /// D3D12 links the stages by semantic and register. When that layout does not fit the 32 input registers (a
    /// vertex stage exports halves only for few parameters, so it could not link anyway), only what the program
    /// reads is declared.
    /// </summary>
    public void Write(List<string> lines)
    {
        var registers = SignatureLayout.Registers(_state.Signatures.Inputs, SignatureLayout.PixelSystemInputs);
        var halfPairs = _state.Attributes.Where(entry => ReadsHalves(entry.Value)).Select(entry => entry.Key + 1).DefaultIfEmpty(0).Max();
        var padded = registers.Count + (HalfMembers * halfPairs) + PositionRegister <= InputRegisters;
        var attributes = _state.Attributes.Keys.ToList();

        if (padded)
        {
            attributes = [.. attributes.Concat(Enumerable.Range(0, registers.Count)).Distinct()];
        }

        var halves = new List<string>();
        lines.Add("struct PSIn {");

        foreach (var attribute in attributes.Order())
        {
            WriteAttribute(lines, halves, registers, attribute, padded);
        }

        lines.AddRange(halves);
        lines.Add("};");
    }

    private static bool ReadsHalves(IEnumerable<string> modes)
    {
        return modes.Contains("f16lo") || modes.Contains("f16hi");
    }

    private void WriteAttribute(List<string> lines, List<string> halves, List<List<SignatureElement>> registers, int attribute, bool padded)
    {
        if (!_state.Attributes.TryGetValue(attribute, out var modes))
        {
            WriteUnread(lines, registers[attribute], attribute);

            return;
        }

        WriteRead(lines, registers, attribute, padded);

        if (!ReadsHalves(modes))
        {
            return;
        }

        var first = attribute;

        if (padded)
        {
            first = halves.Count / HalfMembers;
        }

        for (var index = first; index <= attribute; index++)
        {
            halves.Add($"  float4 attrlo{index} : ATTRLO{index};");
            halves.Add($"  float4 attrhi{index} : ATTRHI{index};");
        }

        _state.InputMap.Add($"  float4 attrlo{attribute} = pin.attrlo{attribute};");
        _state.InputMap.Add($"  float4 attrhi{attribute} = pin.attrhi{attribute};");
    }

    /// <summary>A signature register the program does not read: declared only to keep the register layout.</summary>
    private static void WriteUnread(List<string> lines, List<SignatureElement> elements, int attribute)
    {
        foreach (var element in elements)
        {
            var type = SignatureLayout.TypeName(element);
            var qualifier = type != "float" ? "nointerpolation " : string.Empty;
            lines.Add($"  {qualifier}{type}{SignatureLayout.Width(element.Mask)} unused{attribute}_{element.Mask} : {element.Name}{element.SemanticIndex};");
        }
    }

    private void WriteRead(List<string> lines, List<List<SignatureElement>> registers, int attribute, bool padded)
    {
        var modes = new HashSet<string>(_state.Attributes[attribute]);
        var vertices = modes.Where(mode => mode.StartsWith(VertexModePrefix, StringComparison.Ordinal))
            .Select(mode => int.Parse(mode[VertexModePrefix.Length..], CultureInfo.InvariantCulture)).Order().ToList();

        if (vertices.Count > 0 && modes.Contains("linear"))
        {
            throw new UnsupportedProgramException("attribute both interpolated and read per-vertex");
        }

        if (vertices.Count > 0 && attribute >= registers.Count)
        {
            throw new UnsupportedProgramException("per-vertex attribute without signature");
        }

        if (vertices.Count > 0)
        {
            modes.Add("flat");
        }

        var linear = modes.Contains("linear");
        var flatOnly = modes.Contains("flat") && !linear;

        if (linear || modes.Contains("flat"))
        {
            WriteInterpolated(lines, registers, attribute, vertices, flatOnly);
        }
        else if (padded && attribute < registers.Count)
        {
            WriteUnread(lines, registers[attribute], attribute);
        }
    }

    private void WriteInterpolated(List<string> lines, List<List<SignatureElement>> registers, int attribute, List<int> vertices, bool flatOnly)
    {
        var map = _state.InputMap;

        if (attribute >= registers.Count)
        {
            var qualifier = flatOnly ? "nointerpolation " : string.Empty;
            lines.Add($"  {qualifier}float4 attr{attribute} : TEXCOORD{attribute};");
            map.Add($"  float4 attr{attribute} = pin.attr{attribute};");

            return;
        }

        map.Add($"  float4 attr{attribute} = 0;");

        foreach (var vertex in vertices)
        {
            map.Add($"  float4 attr{attribute}v{vertex} = 0;");
        }

        foreach (var element in registers[attribute])
        {
            var mask = element.Mask;
            var member = $"a{attribute}_{mask}";

            foreach (var vertex in vertices)
            {
                map.Add($"  attr{attribute}v{vertex}.{mask} = {SignatureLayout.ToFloat(element, $"GetAttributeAtVertex(pin.{member}, {vertex})")};");
            }

            var type = SignatureLayout.TypeName(element);
            var qualifier = flatOnly || type != "float" ? "nointerpolation " : string.Empty;
            lines.Add($"  {qualifier}{type}{SignatureLayout.Width(mask)} {member} : {element.Name}{element.SemanticIndex};");
            map.Add($"  attr{attribute}.{mask} = {SignatureLayout.ToFloat(element, $"pin.{member}")};");
        }
    }
}
