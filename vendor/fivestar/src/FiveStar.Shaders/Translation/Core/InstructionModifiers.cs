// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// The modifiers that follow an instruction's operands (<c>offset:16 glc dmask:0x3 row_shr:1 op_sel:[0,1]</c>), in the
/// order they appear. A modifier without a value (<c>glc</c>, <c>row_mirror</c>) is present with a null value.
/// </summary>
internal sealed partial class InstructionModifiers
{
    private readonly List<(string Key, string? Value)> _entries = [];

    private InstructionModifiers()
    {
    }

    /// <summary>SDWA: operand positions whose source select sign-extends (<c>sext(v1)</c>).</summary>
    public HashSet<int> SignExtended { get; } = [];

    /// <summary>The modifier keys in order of first appearance.</summary>
    public IEnumerable<string> Keys => _entries.Select(entry => entry.Key);

    /// <summary>Reads the modifiers of an operand text.</summary>
    public static InstructionModifiers Parse(string operands)
    {
        var modifiers = new InstructionModifiers();

        foreach (Match match in ModifierPattern().Matches(operands))
        {
            var token = match.Groups[1].Value;
            var colon = token.IndexOf(':', StringComparison.Ordinal);
            var key = colon < 0 ? token : token[..colon];
            var value = colon < 0 ? null : token[(colon + 1)..];

            if (value is not null && value.Length == 0)
            {
                value = null;
            }

            modifiers.Set(key, value);
        }

        return modifiers;
    }

    /// <summary>The operand text without its modifiers.</summary>
    public static string Strip(string operands)
    {
        return ModifierPattern().Replace(operands, string.Empty).Trim();
    }

    public bool Has(string key)
    {
        return _entries.Exists(entry => entry.Key == key);
    }

    /// <summary>The value of a modifier, or null when it is absent or has no value.</summary>
    public string? Value(string key)
    {
        foreach (var (entryKey, value) in _entries)
        {
            if (entryKey == key)
            {
                return value;
            }
        }

        return null;
    }

    /// <summary>The integer value of a modifier, or the fallback when it is absent.</summary>
    public int Integer(string key, int fallback)
    {
        var value = Value(key);

        if (value is null)
        {
            return fallback;
        }

        return int.Parse(value, CultureInfo.InvariantCulture);
    }

    /// <summary>The hex value of a modifier (<c>dmask:0x3</c>), or the fallback when it is absent.</summary>
    public int Hex(string key, int fallback)
    {
        const string HexPrefix = "0x";

        var value = Value(key);

        if (value is null)
        {
            return fallback;
        }

        return int.Parse(value.Replace(HexPrefix, string.Empty, StringComparison.Ordinal), NumberStyles.HexNumber, CultureInfo.InvariantCulture);
    }

    /// <summary>A copy holding only the given keys (or all keys except them).</summary>
    public InstructionModifiers Filter(IReadOnlyCollection<string> keys, bool keep)
    {
        var copy = new InstructionModifiers();

        foreach (var (key, value) in _entries)
        {
            if (keys.Contains(key) == keep)
            {
                copy.Set(key, value);
            }
        }

        return copy;
    }

    /// <summary>Sets a modifier; a repeated key keeps its first position and takes the new value.</summary>
    public void Set(string key, string? value)
    {
        var index = _entries.FindIndex(entry => entry.Key == key);

        if (index < 0)
        {
            _entries.Add((key, value));

            return;
        }

        _entries[index] = (key, value);
    }

    [GeneratedRegex(@"\s+(clamp|mul:\d|div:\d|dmask:0x[0-9a-f]+|dim:\S+|offset:swizzle\([^)]*\)|offset:-?\d+|offen|idxen|glc|slc|dlc|done|compr|vm|unorm|lwe|tfe|d16|a16|r128|op_sel:\[[^\]]*\]|op_sel_hi:\[[^\]]*\]|neg_lo:\[[^\]]*\]|neg_hi:\[[^\]]*\]|format:\[[^\]]*\]|dst_sel:\w+|dst_unused:\w+|src0_sel:\w+|src1_sel:\w+|quad_perm:\[[^\]]*\]|dpp8:\[[^\]]*\]|row_\w+(?::\d+)?|row_mask:0x[0-9a-f]+|bank_mask:0x[0-9a-f]+|bound_ctrl:\S+|fi:\d)(?=\s|$)")]
    private static partial Regex ModifierPattern();
}
