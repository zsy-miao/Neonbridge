// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Shaders.Containers;

namespace FiveStar.Shaders.Cache;

/// <summary>Reads the 28-byte state metadata of a compute pipeline (u32 0x4005CF00 at 0).</summary>
public static class ComputeStateParser
{
    /// <summary>Size of the state metadata.</summary>
    public const int Size = 0x1C;

    // u16 numthreads x @0x0C, u16 y @0x0E, u16 z @0x10 (equal to PSV0 for every compute pipeline of the reference dump).
    private const int ThreadsXField = 0x0C;
    private const int ThreadsYField = 0x0E;
    private const int ThreadsZField = 0x10;

    /// <summary>The thread-group size of the pipeline.</summary>
    public static ThreadGroupSize NumThreads(ReadOnlySpan<byte> state)
    {
        if (state.Length < Size)
        {
            throw new InvalidDataException("compute state metadata is shorter than 28 bytes");
        }

        return new ThreadGroupSize(
            BinaryPrimitives.ReadUInt16LittleEndian(state[ThreadsXField..]),
            BinaryPrimitives.ReadUInt16LittleEndian(state[ThreadsYField..]),
            BinaryPrimitives.ReadUInt16LittleEndian(state[ThreadsZField..]));
    }
}
