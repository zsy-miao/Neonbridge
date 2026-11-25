// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The backend's <see cref="Simplifier"/>: a program shaped like the translator's (helpers called with constants, wave
/// reads in divergent regions, repeated and unread wave reads) computes the same words on WARP optimized and plain,
/// in a smaller module; and on a hand-built function it merges a block chain, folds a constant branch, makes a wave
/// read once and drops an unread one.
/// </summary>
public sealed class DxilSimplifierTests : IClassFixture<WarpFixture>
{
    private const int OutputSlot = 1;
    private const int InputSlot = 2;
    private const int Threads = 64;
    private const int Casts = 2;

    private const string Program = """
        uint field(uint x, uint shift, uint width) { return (x >> (shift & 31u)) & ((1u << width) - 1u); }
        uint fetch(uint index) { ByteAddressBuffer b = ResourceDescriptorHeap[2]; return b.Load(index * 4u); }
        [numthreads(64, 1, 1)]
        void main(uint3 id : SV_DispatchThreadID) {
          RWByteAddressBuffer output = ResourceDescriptorHeap[1];
          uint lane = WaveGetLaneIndex();
          bool odd = (id.x & 1u) != 0;
          uint outer = WaveActiveBallot(odd).x;
          uint again = WaveActiveBallot(odd).x;
          bool unread = WaveActiveAnyTrue(odd);
          uint v = field(id.x * 0x01010101u, 8u, 8u) + 0u;
          if (odd) {
            uint inner = WaveActiveBallot(odd).x;
            v += fetch(id.x) + inner + WaveReadLaneFirst(lane);
          } else {
            v ^= fetch(id.x + 1u) * 1u;
          }
          if ((id.x & 3u) == 0u) {
            v += WaveActiveCountBits(odd) + WaveActiveBallot(odd).x;
          }
          output.Store(id.x * 4u, v + outer + again + WaveGetLaneIndex());
        }
        """;

    private readonly WarpFixture _fixture;

    public DxilSimplifierTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [WarpFact]
    public void A_simplified_program_computes_the_same_words_in_a_smaller_module()
    {
        var (plain, plainBytes) = Run(optimize: false);
        var (simplified, simplifiedBytes) = Run(optimize: true);

        Assert.Equal(plain, simplified);
        Assert.NotEqual(new uint[Threads], plain);
        Assert.True(simplifiedBytes < plainBytes, $"{simplifiedBytes} bytes simplified, {plainBytes} plain");
    }

    [Fact]
    public void A_chain_merges_a_constant_branch_folds_and_wave_reads_are_shared_or_dropped()
    {
        var module = new DxilModule();
        var types = module.Types;
        var main = module.Function("main", types.Function(types.Void, []), AttributeSet.None, hasBody: true);

        var entry = module.NewBlock();
        var next = module.NewBlock();
        var taken = module.NewBlock();
        var skipped = module.NewBlock();
        main.Blocks.AddRange([entry, next, taken, skipped]);

        var builder = new IrBuilder(module, entry);
        var ops = new DxOps(builder);
        var thread = ops.ThreadIdComponent(DxOpCode.ThreadId, "threadId", 0);
        var odd = builder.Compare(Predicate.NotEqual, builder.Binary(BinaryOp.And, thread, module.Int32(1)), module.Int32(0));
        builder.Branch(next);

        builder.Block = next;
        var first = AnyTrue(ops, odd);
        var second = AnyTrue(ops, odd);
        AnyTrue(ops, builder.Compare(Predicate.Equal, thread, module.Int32(0)));
        builder.ConditionalBranch(module.Bool(true), taken, skipped);

        builder.Block = taken;
        var handle = ops.HeapHandle(module.Int32(OutputSlot), sampler: false, DxOps.RawBufferKind | DxOps.UavBit, 0);
        ops.RawStore(handle, thread, [builder.Cast(CastOp.ZExt, first, types.Int32), builder.Cast(CastOp.ZExt, second, types.Int32)]);
        builder.Return();

        builder.Block = skipped;
        builder.Return();

        DeadCode.Remove(main);
        Simplifier.Run(main, module);

        var block = Assert.Single(main.Blocks);
        Assert.Single(block.Body, instruction => instruction.Callee?.Name == "dx.op.waveAnyTrue");

        var casts = block.Body.Where(instruction => instruction.Opcode == Opcode.Cast).ToList();
        Assert.Equal(Casts, casts.Count);
        Assert.Same(casts[0].Operands[0], casts[1].Operands[0]);
    }

    private static Value AnyTrue(DxOps ops, Value condition)
    {
        return ops.Call(DxOpCode.WaveAnyTrue, "waveAnyTrue", AttributeSet.NoUnwind, condition.Type, condition);
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

        var input = new BufferResource(InputSlot, Enumerable.Range(0, Threads + 1).Select(index => (uint)((index * 7919) ^ 0x5a5a)).ToArray(), Writable: false);
        var output = new BufferResource(OutputSlot, new uint[Threads], Writable: true);
        var result = _fixture.Runner.Compute(new ComputeJob(container, 1, 1, 1, new Bindings { Buffers = [output, input] }));

        return (result.Buffers[OutputSlot], container.Length);
    }
}
