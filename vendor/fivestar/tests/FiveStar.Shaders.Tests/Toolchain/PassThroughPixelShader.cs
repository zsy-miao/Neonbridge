// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Tests.Toolchain;

/// <summary>
/// A pixel shader that reads the Out struct of a translated VS / MS / GS (same layout, so the linkage matches) and writes
/// the chosen members bit-exactly to uint render targets, zero-padded to four components.
/// </summary>
public static partial class PassThroughPixelShader
{
    /// <summary>
    /// The source and the semantic of each written member. <paramref name="members"/> null writes every pN_* / paramN
    /// member; <paramref name="flat"/> declares the inputs nointerpolation, else they keep the producer's modes.
    /// </summary>
    public static (string Source, IReadOnlyList<string> Semantics) Build(string producerHlsl, IReadOnlyList<string>? members, bool flat)
    {
        var lines = OutStruct().Match(producerHlsl);

        if (!lines.Success)
        {
            throw new InvalidOperationException("no Out struct in the translated shader");
        }

        var source = new StringBuilder("struct Out {\n");
        var widths = new Dictionary<string, (int Width, string Semantic)>();

        foreach (var line in lines.Groups[1].Value.Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
        {
            var member = Member().Match(line);
            var declaration = line;

            if (flat && !line.Contains("SV_Position", StringComparison.Ordinal))
            {
                declaration = "nointerpolation " + line.Replace("nointerpolation ", string.Empty, StringComparison.Ordinal);
            }

            source.Append(CultureInvariant($"  {declaration}\n"));

            if (member.Success)
            {
                var width = member.Groups["width"].Value.Length == 0 ? 1 : member.Groups["width"].Value[0] - '0';
                widths[member.Groups["name"].Value] = (width, member.Groups["semantic"].Value);
            }
        }

        var written = members ?? [.. widths.Keys.Where(name => Written().IsMatch(name))];
        source.Append("};\nstruct PSOut {\n");

        for (var index = 0; index < written.Count; index++)
        {
            source.Append(CultureInvariant($"  uint4 o{index} : SV_Target{index};\n"));
        }

        source.Append("};\nPSOut main(Out i) {\n  PSOut r;\n");

        for (var index = 0; index < written.Count; index++)
        {
            var width = widths[written[index]].Width;
            var padding = string.Concat(Enumerable.Repeat(", 0", 4 - width));
            var value = width == 4 ? $"asuint(i.{written[index]})" : $"uint4(asuint(i.{written[index]}){padding})";
            source.Append(CultureInvariant($"  r.o{index} = {value};\n"));
        }

        source.Append("  return r;\n}\n");

        return (source.ToString(), [.. written.Select(name => widths[name].Semantic)]);
    }

    private static string CultureInvariant(FormattableString text)
    {
        return FormattableString.Invariant(text);
    }

    [GeneratedRegex(@"struct Out \{\n(.*?)\n\};", RegexOptions.Singleline)]
    private static partial Regex OutStruct();

    [GeneratedRegex(@"^(?:nointerpolation\s+)?(?:float|uint|int)(?<width>[1-4]?)\s+(?<name>\w+)\s*:\s*(?<semantic>\w+)")]
    private static partial Regex Member();

    [GeneratedRegex(@"^(p\d+_\w+|param\d+)$")]
    private static partial Regex Written();
}
