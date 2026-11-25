// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The backend's folding and value numbering (<see cref="DxilBackend.Optimize"/>): a program shaped like the
/// translator's output (one buffer's handle made again in sibling branches, constant index arithmetic, a reread
/// after a store, wave sums under different conditions) computes the same words on WARP both ways, in less DXIL.
/// </summary>
public sealed class DxilOptimizeTests : IClassFixture<WarpFixture>
{
    private const int InputSlot = 0;
    private const int OutputSlot = 1;
    private const int Threads = 64;

    private const string Program = """
        [numthreads(64, 1, 1)]
        void main(uint3 id : SV_DispatchThreadID) {
          RWByteAddressBuffer output = ResourceDescriptorHeap[1];
          uint lane = id.x;
          uint row = (5u >> 2) + (lane & 0xffffffffu) * 1u;
          uint a = 0, b = 0, c = 0;
          if (lane & 1u) {
            ByteAddressBuffer input = ResourceDescriptorHeap[0];
            a = input.Load((row & 63u) * 4u) + input.Load((row & 63u) * 4u);
            c = WaveActiveSum(lane);
          } else {
            ByteAddressBuffer input = ResourceDescriptorHeap[0];
            a = input.Load(((row + 1u) & 63u) * 4u) ^ (lane < lane ? 7u : 0u);
          }
          output.Store(lane * 4u, a);
          b = output.Load(lane * 4u) + output.Load(((lane + 1u) & 63u) * 4u);
          output.Store(lane * 4u, a + b * 3u + c + WaveActiveSum(lane) * 5u);
        }
        """;

    private readonly WarpFixture _fixture;

    public DxilOptimizeTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [WarpFact]
    public void An_optimized_program_computes_the_same_words_in_less_dxil()
    {
        var (plain, plainBytes) = Run(optimize: false);
        var (optimized, optimizedBytes) = Run(optimize: true);

        Assert.Equal(plain, optimized);
        Assert.NotEqual(new uint[Threads], optimized);
        Assert.True(optimizedBytes < plainBytes, $"{optimizedBytes} bytes optimized, {plainBytes} plain");
    }

    private (uint[] Words, int Bytes) Run(bool optimize)
    {
        byte[] container;
        var previous = DxilBackend.Optimize;
        DxilBackend.Optimize = optimize;

        try
        {
            container = DxilBackend.Compile(Program, "cs");
        }
        finally
        {
            DxilBackend.Optimize = previous;
        }

        var input = new BufferResource(InputSlot, Enumerable.Range(0, Threads).Select(index => (uint)(index * 2654435761u)).ToArray(), Writable: false);
        var output = new BufferResource(OutputSlot, new uint[Threads], Writable: true);
        var result = _fixture.Runner.Compute(new ComputeJob(container, 1, 1, 1, new Bindings { Buffers = [input, output] }));

        return (result.Buffers[OutputSlot], container.Length);
    }
}
