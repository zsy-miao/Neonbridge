// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Tests.Differential;

/// <summary>Reads the parts of translated HLSL the stage harnesses wrap: struct members and the entry point.</summary>
public static partial class HlslText
{
    /// <summary>The members of <c>struct name { ... };</c>, or an empty list when the program has no such struct.</summary>
    public static IReadOnlyList<StructMember> Members(string hlsl, string name)
    {
        var lines = hlsl.Split('\n');
        var start = Array.IndexOf(lines, $"struct {name} {{");
        var members = new List<StructMember>();

        if (start < 0)
        {
            return members;
        }

        foreach (var line in lines.Skip(start + 1).TakeWhile(line => line != "};"))
        {
            var member = MemberPattern().Match(line);

            if (!member.Success)
            {
                throw new NotSupportedException($"struct {name} member not understood: {line.Trim()}");
            }

            var width = 1;

            if (member.Groups["width"].Value.Length > 0)
            {
                width = int.Parse(member.Groups["width"].Value, CultureInfo.InvariantCulture);
            }

            members.Add(new StructMember(line.Trim(), member.Groups["scalar"].Value, width, member.Groups["name"].Value, member.Groups["semantic"].Value));
        }

        return members;
    }

    /// <summary>
    /// Renames the entry <c>Out main(...)</c> to <paramref name="inner"/> and returns the new text, the attribute lines
    /// that stood before it (removed) and its parameter list.
    /// </summary>
    public static (string Hlsl, string Attributes, string Parameters) RenameEntry(string hlsl, string inner)
    {
        var entry = EntryPattern().Match(hlsl);

        if (!entry.Success)
        {
            throw new NotSupportedException("entry point Out main(...) not found");
        }

        var renamed = hlsl[..entry.Index] + $"\nOut {inner}({entry.Groups["parameters"].Value}) {{" + hlsl[(entry.Index + entry.Length)..];

        return (renamed, entry.Groups["attributes"].Value, entry.Groups["parameters"].Value);
    }

    /// <summary>The argument list that passes every parameter of a parameter list on: "PSIn pin, float4 p : X" -> "pin, p".</summary>
    public static string Arguments(string parameters)
    {
        var names = parameters.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .Select(parameter => parameter.Split(':')[0].Trim().Split(' ')[^1]);

        return string.Join(", ", names);
    }

    [GeneratedRegex(@"^\s*(?:(?:nointerpolation|linear|centroid|noperspective|sample)\s+)*(?<scalar>[a-z][a-z0-9]*?)(?<width>[1-4]?) (?<name>\w+) : (?<semantic>\w+);")]
    private static partial Regex MemberPattern();

    [GeneratedRegex(@"\n(?<attributes>(?:\[[^\n]*\]\n)*)Out main\((?<parameters>[^)]*)\) \{")]
    private static partial Regex EntryPattern();
}
