// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>The literal dword of an instruction: present when a source uses code 255, read after the encoding's dwords.</summary>
internal static class Sources
{
    /// <summary>
    /// The literal for these source codes, or 0 when none is used; <paramref name="words"/> grows by one when one is.
    /// False when the literal lies past the end of the program.
    /// </summary>
    public static bool Literal(CodeWords code, int at, IEnumerable<int> sources, ref int words, out uint literal)
    {
        literal = 0;

        if (!sources.Contains(OperandText.Literal))
        {
            return true;
        }

        if (!code.Has(at + words, 1))
        {
            return false;
        }

        literal = code[at + words];
        words++;

        return true;
    }

    /// <summary>Joins operands into the text after a mnemonic: a space, then the operands separated by commas.</summary>
    public static string Join(string mnemonic, IEnumerable<string> operands)
    {
        var list = string.Join(", ", operands);

        if (list.Length == 0)
        {
            return mnemonic;
        }

        return mnemonic + " " + list;
    }
}
