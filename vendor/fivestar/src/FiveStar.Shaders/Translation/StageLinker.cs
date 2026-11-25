// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;
using FiveStar.Shaders.Containers;

namespace FiveStar.Shaders.Translation;

/// <summary>
/// Links a translated producer (vertex shader, mesh shader or mesh-fallback vertex shader) to the pixel shader it runs
/// with, so D3D12 finds every pixel-shader input in the register it reads it from. dxc assigns registers in declaration
/// order and never packs elements of different interpolation modes together, so the producer's <c>Out</c> struct is
/// declared as the pixel shader declares its inputs: the same elements in the same order with the same modes, then
/// the producer's other outputs. An input the producer does not write is added: a packed f16 half
/// (<c>ATTRLO</c>/<c>ATTRHI</c>) is computed from the parameter's members, anything else is 0.
/// </summary>
public static partial class StageLinker
{
    private const string Position = "SV_POSITION";
    private const string LowHalf = "ATTRLO";
    private const string HighHalf = "ATTRHI";
    private const string AddedPrefix = "lnk";
    private const string DroppedPrefix = "unread_";
    private const string ReturnAnchor = "\n  return o;";
    private const string StagedAnchor = "\n    verts[in_gi] = o;";
    private const string MeshEntry = "out vertices Out verts[";

    // HLSL scalar type per DXIL signature component type (1 uint, 2 int, 3 float, 4..6 their 16-bit forms).
    private static readonly string[] ScalarTypes = ["float", "uint", "int", "float", "min16uint", "min16int", "min16float"];

    /// <summary>The producer HLSL with its <c>Out</c> struct laid out for the consumer's PSV0 input elements.</summary>
    public static string Link(string producerHlsl, IReadOnlyList<PsvElement> consumerInputs)
    {
        return Link(producerHlsl, consumerInputs, dropUnread: false);
    }

    /// <summary>
    /// As <see cref="Link(string, IReadOnlyList{PsvElement})"/>; with <paramref name="dropUnread"/> the outputs the
    /// consumer does not read (but the position) leave the <c>Out</c> struct and are written to static variables
    /// instead, so a mesh shader's outputs fit its 32 KB.
    /// </summary>
    public static string Link(string producerHlsl, IReadOnlyList<PsvElement> consumerInputs, bool dropUnread)
    {
        var match = OutStructPattern().Match(producerHlsl);

        if (!match.Success)
        {
            throw new UnsupportedProgramException("no Out struct");
        }

        var group = match.Groups["members"];
        var layout = new Layout(group.Value.Split('\n').Select(Parse).ToList());

        foreach (var element in consumerInputs)
        {
            layout.Place(element);
        }

        var dropped = new List<OutStructMember>();

        if (dropUnread)
        {
            dropped = layout.Unplaced().Where(member => member.Semantic != Position).ToList();
        }

        var lines = layout.Lines().Where(line => !dropped.Exists(member => line.Trim() == member.Declaration));
        var linked = producerHlsl[..group.Index] + string.Join("\n", lines) + producerHlsl[(group.Index + group.Length)..];

        foreach (var half in layout.Halves)
        {
            linked = CopyHalf(linked, layout.Members, half);
        }

        return Drop(linked, dropped);
    }

    /// <summary>Writes and reads of dropped outputs go to a static variable of their type each.</summary>
    private static string Drop(string hlsl, List<OutStructMember> dropped)
    {
        if (dropped.Count == 0)
        {
            return hlsl;
        }

        var statics = new System.Text.StringBuilder();

        foreach (var member in dropped)
        {
            var type = member.Declaration[..member.Declaration.IndexOf(' ', StringComparison.Ordinal)];
            var variable = $"{DroppedPrefix}{member.Name}";
            statics.Append(CultureInfo.InvariantCulture, $"static {type} {variable};\n");
            hlsl = Regex.Replace(hlsl, $@"(?:verts\[in_gi\]|\bo)\.{Regex.Escape(member.Name)}\b", variable);
        }

        var at = OutStructPattern().Match(hlsl).Index;

        return hlsl.Insert(at, statics.ToString());
    }

    /// <summary>
    /// Whether the desktop runtime links the stages as compiled: every allocated interpolated or position input of the
    /// consumer is an output of the producer with the same semantic, in the same register, covering its components.
    /// The runtime compares no interpolation modes (docs/shader-translation.md, Linkage).
    /// </summary>
    public static bool Links(IReadOnlyList<PsvElement> producerOutputs, IReadOnlyList<PsvElement> consumerInputs)
    {
        foreach (var input in consumerInputs)
        {
            if (!input.Allocated || input.SemanticKind is not (PsvSemanticKind.Arbitrary or PsvSemanticKind.Position))
            {
                continue;
            }

            var covered = producerOutputs.Any(output => output.Allocated
                && output.SemanticKind == input.SemanticKind
                && string.Equals(output.Name, input.Name, StringComparison.OrdinalIgnoreCase)
                && output.SemanticIndex == input.SemanticIndex
                && output.StartRow == input.StartRow
                && output.Rows >= input.Rows
                && output.StartColumn <= input.StartColumn
                && output.StartColumn + output.Columns >= input.StartColumn + input.Columns);

            if (!covered)
            {
                return false;
            }
        }

        return true;
    }

    /// <summary>
    /// The pixel shader HLSL without the <c>PSIn</c> members it never reads (signature registers declared only for the
    /// layout, f16 halves below the ones it reads): a producer linked to it then exports only what it reads. A mesh
    /// program has 32 KB for the outputs of its 128 vertices, which the full layout passes.
    /// </summary>
    public static string TrimInputs(string pixelHlsl)
    {
        var match = PixelInputPattern().Match(pixelHlsl);

        if (!match.Success)
        {
            throw new UnsupportedProgramException("no PSIn struct");
        }

        var group = match.Groups["members"];
        var kept = group.Value.Split('\n').Where(line =>
        {
            var member = MemberPattern().Match(line);

            return !member.Success
                || member.Groups["semantic"].Value.StartsWith("SV_", StringComparison.OrdinalIgnoreCase)
                || pixelHlsl.Contains($"pin.{member.Groups["name"].Value}", StringComparison.Ordinal);
        });

        return pixelHlsl[..group.Index] + string.Join("\n", kept) + pixelHlsl[(group.Index + group.Length)..];
    }

    /// <summary>A key for the consumer inputs a link depends on: producers linked to equal inputs are the same.</summary>
    public static string InputsKey(IReadOnlyList<PsvElement> consumerInputs)
    {
        return string.Join(";", consumerInputs.Select(input =>
            $"{input.Name}{input.SemanticIndex}:{(int)input.SemanticKind}:{input.StartRow}:{input.Rows}:{input.StartColumn}:{input.Columns}:{input.ComponentType}:{input.InterpolationMode}:{input.Allocated}"));
    }

    private static OutStructMember Parse(string line)
    {
        var member = MemberPattern().Match(line);

        if (!member.Success)
        {
            throw new UnsupportedProgramException($"Out member not understood: {line.Trim()}");
        }

        var index = 0;

        if (member.Groups["index"].Length > 0)
        {
            index = int.Parse(member.Groups["index"].Value, CultureInfo.InvariantCulture);
        }

        return new OutStructMember(member.Groups["declaration"].Value, member.Groups["name"].Value, member.Groups["semantic"].Value.ToUpperInvariant(), index);
    }

    /// <summary>Writes an added f16 half from the members of its parameter register (all of them 0 when it has none).</summary>
    private static string CopyHalf(string hlsl, IReadOnlyList<OutStructMember> members, (string Member, int Register, bool High) half)
    {
        var parameter = new Regex($@"^(?:p{half.Register}_(?<mask>[xyzw]+)|param{half.Register})$");
        var sources = members.Select(member => (Member: member, Match: parameter.Match(member.Name))).Where(entry => entry.Match.Success).ToList();
        var shift = string.Empty;

        if (half.High)
        {
            shift = " >> 16";
        }

        if (sources.Count == 0)
        {
            return hlsl;
        }

        var gathered = string.Concat(sources.Select(entry => Gather(entry.Member.Name, entry.Match.Groups["mask"].Value)));
        var statement = $"\n  {{ float4 lk = 0;{gathered} o.{half.Member} = f16tof32(asuint(lk){shift}); }}";

        // Vertex shaders fill o and return it; staging mesh shaders fill o and store it.
        if (!hlsl.Contains(MeshEntry, StringComparison.Ordinal))
        {
            var end = hlsl.LastIndexOf(ReturnAnchor, StringComparison.Ordinal);

            if (end < 0)
            {
                throw new UnsupportedProgramException("no return of the Out struct");
            }

            return hlsl.Insert(end, statement);
        }

        var staged = hlsl.LastIndexOf(StagedAnchor, StringComparison.Ordinal);

        if (staged >= 0)
        {
            return hlsl.Insert(staged, statement);
        }

        // Mesh shaders that write their outputs at each export: copy the half where its parameter is exported.
        var export = new Regex($@"(verts\[in_gi\]\.(?:p{half.Register}_[xyzw]+|param{half.Register})\s*=[^;]*;)");

        return export.Replace(hlsl, $"$1 verts[in_gi].{half.Member} = f16tof32(asuint(_e){shift});");
    }

    private static string Gather(string member, string mask)
    {
        if (mask.Length == 0)
        {
            return $" lk = asfloat(o.{member});";
        }

        return $" lk.{mask} = asfloat(o.{member});";
    }

    private static string Added(PsvElement element, int number)
    {
        var type = ScalarTypes[Math.Clamp(element.ComponentType, 0, ScalarTypes.Length - 1)];

        if (element.Columns > 1)
        {
            type += element.Columns.ToString(CultureInfo.InvariantCulture);
        }

        var array = string.Empty;

        if (element.Rows > 1)
        {
            array = $"[{element.Rows}]";
        }

        return $"{type} {AddedPrefix}{number}{array} : {element.Name}{element.SemanticIndex};";
    }

    [GeneratedRegex(@"^struct Out \{\n(?<members>.*?)\n\};", RegexOptions.Singleline | RegexOptions.Multiline)]
    private static partial Regex OutStructPattern();

    [GeneratedRegex(@"^struct PSIn \{\n(?<members>.*?)\n\};", RegexOptions.Singleline | RegexOptions.Multiline)]
    private static partial Regex PixelInputPattern();

    [GeneratedRegex(@"^\s*(?:(?:nointerpolation|linear|noperspective|centroid|sample)\s+)*(?<declaration>\w+\s+(?<name>\w+)(?:\[\d+\])?\s*:\s*(?<semantic>[A-Za-z_]\w*?)(?<index>\d*)\s*;)\s*$")]
    private static partial Regex MemberPattern();

    /// <summary>The members in their new order, the added ones and the f16 halves still to compute.</summary>
    private sealed class Layout
    {
        private readonly List<string> _lines = [];
        private readonly HashSet<OutStructMember> _placed = [];

        public Layout(List<OutStructMember> members)
        {
            Members = members;
        }

        public List<OutStructMember> Members { get; }

        public List<(string Member, int Register, bool High)> Halves { get; } = [];

        /// <summary>Declares the producer member of one consumer element, or adds one.</summary>
        public void Place(PsvElement element)
        {
            if (element.SemanticKind == PsvSemanticKind.Position)
            {
                Take(Members.FirstOrDefault(member => member.Semantic == Position), string.Empty);

                return;
            }

            if (element.SemanticKind != PsvSemanticKind.Arbitrary)
            {
                return;
            }

            var name = element.Name.ToUpperInvariant();
            var prefix = InterpolationModes.Prefix(element.InterpolationMode);
            var member = Members.FirstOrDefault(member => member.Semantic == name && member.SemanticIndex == element.SemanticIndex && !_placed.Contains(member));

            if (member is not null)
            {
                Take(member, prefix);

                return;
            }

            var number = _lines.Count;
            _lines.Add("  " + prefix + Added(element, number));

            if (name is LowHalf or HighHalf)
            {
                Halves.Add(($"{AddedPrefix}{number}", element.SemanticIndex, name == HighHalf));
            }
        }

        /// <summary>The producer's members the consumer reads none of.</summary>
        public IEnumerable<OutStructMember> Unplaced()
        {
            return Members.Where(member => !_placed.Contains(member));
        }

        /// <summary>The placed members, then the producer's other members in their order.</summary>
        public IEnumerable<string> Lines()
        {
            return _lines.Concat(Members.Where(member => !_placed.Contains(member)).Select(member => "  " + member.Declaration));
        }

        private void Take(OutStructMember? member, string prefix)
        {
            if (member is not null && _placed.Add(member))
            {
                _lines.Add("  " + prefix + member.Declaration);
            }
        }
    }
}
