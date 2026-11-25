// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>The program as little-endian dwords, with the wave size that decides how lane masks print.</summary>
internal sealed class CodeWords
{
    private readonly uint[] _words;

    public CodeWords(uint[] words, bool wave64)
    {
        _words = words;
        Wave64 = wave64;
    }

    public bool Wave64 { get; }

    public int Count => _words.Length;

    public uint this[int index] => _words[index];

    /// <summary>Whether <paramref name="count"/> dwords starting at <paramref name="at"/> lie inside the program.</summary>
    public bool Has(int at, int count)
    {
        return at + count <= _words.Length;
    }
}
