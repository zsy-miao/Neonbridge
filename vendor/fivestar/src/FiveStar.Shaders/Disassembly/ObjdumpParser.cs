// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Disassembly;

/// <summary>
/// Reads llvm-objdump's AMDGPU listing: <c>\t&lt;text&gt; // &lt;address&gt;: &lt;hex words&gt; [&lt;.text+0x...&gt;]</c>. The optional
/// label is a branch target and is appended to the text as <c>@T0x&lt;target&gt;</c>.
/// </summary>
public static partial class ObjdumpParser
{
    private const int BytesPerWord = 4;

    /// <summary>The instructions of an objdump listing, in address order.</summary>
    public static IReadOnlyList<Instruction> Parse(string listing)
    {
        var instructions = new List<Instruction>();

        foreach (var line in listing.Replace("\r\n", "\n", StringComparison.Ordinal).Split('\n'))
        {
            var match = LinePattern().Match(line);

            if (!match.Success)
            {
                continue;
            }

            var text = match.Groups["text"].Value;
            var address = int.Parse(match.Groups["address"].Value, NumberStyles.HexNumber, CultureInfo.InvariantCulture);
            var words = match.Groups["words"].Value.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);
            var label = match.Groups["label"];

            if (label.Success)
            {
                text += $" @T0x{LabelTarget(label.Value):x}";
            }

            instructions.Add(new Instruction(address, BytesPerWord * words.Length, text));
        }

        return instructions;
    }

    /// <summary>The target offset; a target before the section start wraps to a 64-bit value.</summary>
    private static ulong LabelTarget(string label)
    {
        var offset = LabelOffsetPattern().Match(label);

        if (!offset.Success)
        {
            return 0;
        }

        return ulong.Parse(offset.Groups[1].Value, NumberStyles.HexNumber, CultureInfo.InvariantCulture);
    }

    [GeneratedRegex(@"^\s+(?<text>.*?)\s*//\s*(?<address>[0-9A-Fa-f]+):\s*(?<words>(?:[0-9A-Fa-f]{8}\s*)+)(?:<(?<label>[^>]*)>)?\s*$")]
    private static partial Regex LinePattern();

    [GeneratedRegex(@"\+0x([0-9a-f]+)$")]
    private static partial Regex LabelOffsetPattern();
}
