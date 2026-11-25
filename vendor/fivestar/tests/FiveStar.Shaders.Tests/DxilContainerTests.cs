// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Container;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;
using FiveStar.Shaders.Dxil.Validation;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The DXIL plumbing without the HLSL front end: a compute module built by hand passes dxil.dll's validator, which
/// computes the same container digest as ours, and runs on WARP.
/// </summary>
public sealed class DxilContainerTests : IClassFixture<WarpFixture>
{
    private const int OutputSlot = 1;
    private const int Threads = 64;

    private readonly WarpFixture _fixture;

    public DxilContainerTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [WarpFact]
    public void A_handwritten_compute_module_validates_with_our_digest_and_runs()
    {
        var container = ThreadIdWriter();
        var signed = (byte[])container.Clone();
        using var validator = DxilValidator.Load(Path.Combine(Path.GetDirectoryName(ShaderToolchain.Find().Dxc)!, "dxil.dll"));

        var errors = validator.Validate(signed);
        Assert.True(errors is null, errors);
        Assert.Equal(container[4..20], signed[4..20]);

        var output = new BufferResource(OutputSlot, new uint[Threads], Writable: true);
        var result = _fixture.Runner.Compute(new ComputeJob(container, 1, 1, 1, new Bindings { Buffers = [output] }));

        Assert.Equal(Enumerable.Range(0, Threads).Select(thread => (uint)((thread * 3) + 7)), result.Buffers[OutputSlot]);
    }

    /// <summary>Thread i of one 64-thread group writes 3i + 7 to dword i of the raw UAV at heap slot 1.</summary>
    private static byte[] ThreadIdWriter()
    {
        var module = new DxilModule();
        var types = module.Types;
        var main = module.Function("main", types.Function(types.Void, []), AttributeSet.None, hasBody: true);
        var entry = module.NewBlock();
        main.Blocks.Add(entry);
        var builder = new IrBuilder(module, entry);

        var ops = new DxOps(builder);
        var thread = ops.ThreadIdComponent(DxOpCode.ThreadId, "threadId", 0);
        var value = builder.Binary(BinaryOp.Add, builder.Binary(BinaryOp.Mul, thread, module.Int32(3)), module.Int32(7));
        var handle = ops.HeapHandle(module.Int32(OutputSlot), sampler: false, DxOps.RawBufferKind | DxOps.UavBit, 0);
        ops.RawStore(handle, builder.Binary(BinaryOp.Shl, thread, module.Int32(2)), [value]);
        builder.Return();

        var flags = ShaderFlagBits.ResourceDescriptorHeapIndexing;

        return ShaderContainer.Write(module, main, new ShaderDescription((Threads, 1, 1), null, flags, 0));
    }
}
