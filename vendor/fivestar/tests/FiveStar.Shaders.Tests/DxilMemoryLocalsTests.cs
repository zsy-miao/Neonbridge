// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// Locals in memory (<see cref="DxilBackend.MemoryLocalsCases"/>), for the superstep form's dispatch switch: a program
/// shaped like it (a loop over a switch on a program counter, registers and statics carried between cases) computes
/// the same words on WARP with its locals in slots and in memory.
/// </summary>
public sealed class DxilMemoryLocalsTests : IClassFixture<WarpFixture>
{
    private const int OutputSlot = 1;
    private const int Threads = 64;

    private const string Dispatch = """
        static uint g_steps;
        [numthreads(64, 1, 1)]
        void main(uint3 id : SV_DispatchThreadID) {
          RWByteAddressBuffer output = ResourceDescriptorHeap[1];
          uint pc = 0, v0 = id.x, v1 = 7u, s0 = 0;
          float f0 = 0.5;
          g_steps = 0;
          [loop] while (pc < 100u) {
            switch (pc) {
            case 0: { v1 = v0 * 3u + v1; f0 = f0 * 2.0 + (float)v0; pc = 1; } break;
            case 1: { if (v0 & 1u) { s0 = v1 ^ 0x55u; pc = 2; } else { s0 = v1 + 9u; pc = 3; } } break;
            case 2: { v0 = s0 + (uint)f0; pc = 4; } break;
            case 3: { v0 = s0 * 2u - (uint)f0; pc = 4; } break;
            case 4: { g_steps += 1u; v1 = v1 + v0; pc = g_steps < 3u ? 1u : 100u; } break;
            default: { pc = 100u; } break;
            }
          }
          output.Store(id.x * 4u, v1 + g_steps * 1000u + asuint(f0));
        }
        """;

    private readonly WarpFixture _fixture;

    public DxilMemoryLocalsTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [WarpFact]
    public void A_dispatch_loop_computes_the_same_words_with_its_locals_in_memory()
    {
        var inSlots = Run(memoryLocalsCases: 0);
        var inMemory = Run(memoryLocalsCases: 1);

        Assert.Equal(inSlots, inMemory);
        Assert.NotEqual(new uint[Threads], inSlots);
    }

    private uint[] Run(int memoryLocalsCases)
    {
        byte[] container;
        var previous = DxilBackend.MemoryLocalsCases;
        DxilBackend.MemoryLocalsCases = memoryLocalsCases;

        try
        {
            container = DxilBackend.Compile(Dispatch, "cs");
        }
        finally
        {
            DxilBackend.MemoryLocalsCases = previous;
        }

        var output = new BufferResource(OutputSlot, new uint[Threads], Writable: true);
        var result = _fixture.Runner.Compute(new ComputeJob(container, 1, 1, 1, new Bindings { Buffers = [output] }));

        return result.Buffers[OutputSlot];
    }
}
