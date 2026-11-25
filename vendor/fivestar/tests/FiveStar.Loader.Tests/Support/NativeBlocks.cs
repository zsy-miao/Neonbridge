// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using System.Text;

namespace FiveStar.Loader.Tests.Support;

/// <summary>Zeroed native blocks standing in for game memory, freed together.</summary>
public sealed unsafe class NativeBlocks : IDisposable
{
    private readonly List<nint> _blocks = [];

    public byte* Alloc(int size)
    {
        var block = (byte*)NativeMemory.AllocZeroed((nuint)Math.Max(size, 1));
        _blocks.Add((nint)block);

        return block;
    }

    public byte* Copy(ReadOnlySpan<byte> bytes)
    {
        var block = Alloc(bytes.Length);
        bytes.CopyTo(new Span<byte>(block, bytes.Length));

        return block;
    }

    public byte* Ansi(string text)
    {
        return Copy(Encoding.ASCII.GetBytes(text + "\0"));
    }

    /// <summary>Writes a {pointer, u64 size} range over a native copy of <paramref name="bytes"/>.</summary>
    public void Range(byte* field, ReadOnlySpan<byte> bytes)
    {
        *(nint*)field = (nint)Copy(bytes);
        *(long*)(field + sizeof(long)) = bytes.Length;
    }

    public void Dispose()
    {
        _blocks.ForEach(block => NativeMemory.Free((void*)block));
        _blocks.Clear();
    }
}
