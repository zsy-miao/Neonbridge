// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly;

/// <summary>
/// One disassembled instruction: byte address in the program, encoded size and the llvm-objdump text. A branch's
/// text ends with <c>@T0x&lt;target&gt;</c>, the resolved target address.
/// </summary>
public sealed record Instruction(int Address, int Size, string Text)
{
    /// <summary>The address of the next instruction.</summary>
    public int Next => Address + Size;

    /// <summary>The mnemonic: the first word of the text.</summary>
    public string Mnemonic
    {
        get
        {
            var space = Text.IndexOfAny([' ', '\t']);
            var mnemonic = space < 0 ? Text : Text[..space];

            return mnemonic;
        }
    }
}
