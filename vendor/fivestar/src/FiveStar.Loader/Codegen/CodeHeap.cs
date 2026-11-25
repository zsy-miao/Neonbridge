// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Codegen;

/// <summary>
/// Bump allocator for generated x64 code. The current chunk is read-write while it is filled; <see cref="Seal"/>
/// makes it read-execute (never both), and later writes go to a fresh chunk, so code that may already run is never
/// writable again.
/// </summary>
internal sealed unsafe class CodeHeap
{
    private const int ChunkSize = 64 * 1024;

    /// <summary>Default alignment of a generated function (a cache-friendly 16 bytes).</summary>
    private const int FunctionAlignment = 16;

    private readonly object _gate = new();
    private nint _chunk;
    private int _used;
    private bool _sealed = true;

    public nint Write(ReadOnlySpan<byte> code)
    {
        lock (_gate)
        {
            int offset = (_used + FunctionAlignment - 1) & ~(FunctionAlignment - 1);

            if (_sealed || offset + code.Length > ChunkSize)
            {
                StartChunk();
                offset = 0;
            }

            nint address = _chunk + offset;
            code.CopyTo(new Span<byte>((void*)address, code.Length));
            _used = offset + code.Length;

            return address;
        }
    }

    /// <summary>Makes everything written so far executable; call before any generated code can run.</summary>
    public void Seal()
    {
        lock (_gate)
        {
            if (!_sealed)
            {
                SealChunk();
            }
        }
    }

    private void StartChunk()
    {
        if (!_sealed)
        {
            SealChunk();
        }

        _chunk = Kernel32.VirtualAlloc(0, ChunkSize, Win32Constants.MemReserveCommit, Win32Constants.PageReadWrite);

        if (_chunk == 0)
        {
            throw new InvalidOperationException("CodeHeap VirtualAlloc failed");
        }

        _sealed = false;
    }

    private void SealChunk()
    {
        uint previous;
        _ = Kernel32.VirtualProtect(_chunk, ChunkSize, Win32Constants.PageExecuteRead, &previous);
        _ = Kernel32.FlushInstructionCache(Kernel32.GetCurrentProcess(), _chunk, ChunkSize);
        _sealed = true;
    }
}
