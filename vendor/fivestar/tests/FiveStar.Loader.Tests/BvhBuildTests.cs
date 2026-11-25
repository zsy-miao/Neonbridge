// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims;
using FiveStar.Loader.Shims.Bvh;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Shims.D3D12X.Resources;
using FiveStar.Shaders.Bvh;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// The CPU path behind BuildRaytracingAccelerationStructureBatched2X: a batched desc as the game fills it, over
/// buffers addressed like game memory, puts the console-layout BLAS of those buffers at the destination and its size
/// in the compacted-size slot; a top-level desc writes nothing.
/// </summary>
public sealed unsafe class BvhBuildTests
{
    private const int DescSize = 0x50;
    private const int GeometrySize = 0x38;
    private const int DestSize = 0x1000;

    private static readonly float[] Positions = [0, 0, 0, 1, 0, 0, 0, 1, 0, 1, 1, 0];
    private static readonly uint[] Indices = [0, 1, 2, 1, 3, 2];

    [Fact]
    public void Batched_bottom_level_build_writes_the_structure_and_its_size()
    {
        using var physical = new XMemPhysicalMemory(new LoaderLog(TextWriter.Null, null, false));
        var builder = new BvhBatchBuilder(new GameMemorySource(physical), null, new OnceLog(new LoaderLog(TextWriter.Null, null, false)));
        byte* block = (byte*)NativeMemory.AllocZeroed(DestSize * 2);

        try
        {
            fixed (float* vertices = Positions)
            fixed (uint* indices = Indices)
            {
                byte* geometry = block;
                byte* desc = block + GeometrySize;
                ulong* compacted = (ulong*)(desc + DescSize);
                byte* postbuild = desc + DescSize + sizeof(ulong);
                byte* dest = block + DestSize;

                *(uint*)(geometry + 0x10) = 42;
                *(uint*)(geometry + 0x14) = 6;
                *(uint*)(geometry + 0x18) = (uint)Indices.Length;
                *(uint*)(geometry + 0x1C) = (uint)(Positions.Length / 3);
                *(ulong*)(geometry + 0x20) = (ulong)indices;

                *(ulong*)(geometry + 0x28) = (ulong)vertices;
                *(ulong*)(geometry + 0x30) = 12;

                *(ulong*)desc = (ulong)dest;
                *(uint*)(desc + 0x08) = 1;
                *(uint*)(desc + 0x10) = 1;

                *(ulong*)(desc + 0x18) = (ulong)geometry;
                *(uint*)(desc + 0x30) = 1;
                *(ulong*)(desc + 0x38) = (ulong)postbuild;
                *(ulong*)postbuild = (ulong)compacted;

                Assert.Equal(1, builder.Build(1, desc));
                var expected = XdxrBlas.Build(Triangles(vertices, indices));
                Assert.Equal(expected, new ReadOnlySpan<byte>(dest, expected.Length).ToArray());
                Assert.Equal((ulong)expected.Length, *compacted);

                *(uint*)(desc + 0x08) = 2;
                Assert.Equal(0, builder.Build(1, desc));
            }
        }
        finally
        {
            NativeMemory.Free(block);
        }
    }

    private static List<BvhTriangle> Triangles(float* vertices, uint* indices)
    {
        var triangles = new List<BvhTriangle>();
        var buffers = new TriangleBuffers(
            new ReadOnlySpan<byte>(vertices, Positions.Length * sizeof(float)), 12, 6, (uint)(Positions.Length / 3),
            new ReadOnlySpan<byte>(indices, Indices.Length * sizeof(uint)), 42, (uint)Indices.Length, [], 0, 0);
        TriangleDecoder.Append(triangles, buffers);

        return triangles;
    }
}
