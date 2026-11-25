// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims;
using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>Indirect dispatch arguments are clamped into the list's scratch before the GPU runs them, on the default adapter.</summary>
public sealed unsafe class D3D12XIndirectClampTests
{
    private const int CloseSlot = 9;
    private const int CopyBufferRegionSlot = 15;
    private const int MapSlot = 8;
    private const uint Stride = 12;
    private const uint Records = 3;

    [Fact]
    public void Dispatch_records_past_the_limits_run_empty_or_clamped_and_good_ones_pass()
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        var objects = ShimContext.Runtime.D3D12X.Objects;

        // A one-argument dispatch signature, as the game creates them (stride 12).
        var argument = new IndirectArgument { Type = IndirectArgument.DispatchType };
        var description = new CommandSignatureDescription { ByteStride = Stride, ArgumentCount = 1, Arguments = &argument };
        nint signature = 0x5160;
        CommandSignatures.Created(signature, &description, 0);

        nint upload = device.Buffer(XboxDevice.UploadHeap, 256, XboxDevice.GenericRead);
        uint* args = Map(upload);
        uint[] asked = [4, 5, 6, uint.MaxValue, uint.MaxValue, uint.MaxValue, 70000, 2, 1];
        asked.CopyTo(new Span<uint>(args, asked.Length));

        nint list = device.DirectList();
        Assert.True(IndirectClamp.Record((XboxObject*)list, signature, Records, objects.Unwrap(upload), 0, 0, 0, out nint scratch, out ulong offset));
        IndirectClamp.Done((XboxObject*)list, scratch);

        nint readback = device.Buffer(XboxDevice.ReadbackHeap, 256, XboxDevice.CopyDest);
        nint inner = ((XboxObject*)list)->Inner;
        ((delegate* unmanaged<nint, nint, ulong, nint, ulong, ulong, void>)XboxDevice.Vtable(inner)[CopyBufferRegionSlot])(inner, objects.Unwrap(readback), 0, scratch, offset, Stride * Records);
        Assert.Equal(0, ((delegate* unmanaged<nint, int>)XboxDevice.Vtable(list)[CloseSlot])(list));
        device.Run(list);

        uint* clamped = Map(readback);
        Assert.Equal([4u, 5u, 6u], new Span<uint>(clamped, 3).ToArray());
        Assert.Equal([0u, 0u, 0u], new Span<uint>(clamped + 3, 3).ToArray());
        Assert.Equal([65535u, 2u, 1u], new Span<uint>(clamped + 6, 3).ToArray());
    }

    [Fact]
    public void Records_past_the_list_budget_run_empty()
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        var objects = ShimContext.Runtime.D3D12X.Objects;

        var argument = new IndirectArgument { Type = IndirectArgument.DispatchType };
        var description = new CommandSignatureDescription { ByteStride = Stride, ArgumentCount = 1, Arguments = &argument };
        nint signature = 0x5161;
        CommandSignatures.Created(signature, &description, 0);

        // Four dispatches of 2^20 groups (2^26 work items each): the 2^27 budget of a recording holds two of them.
        const uint many = 4;
        nint upload = device.Buffer(XboxDevice.UploadHeap, 256, XboxDevice.GenericRead);
        uint* args = Map(upload);

        for (int i = 0; i < many; i++)
        {
            args[i * 3] = 1024;
            args[(i * 3) + 1] = 1024;
            args[(i * 3) + 2] = 1;
        }

        nint list = device.DirectList();
        Assert.True(IndirectClamp.Record((XboxObject*)list, signature, many, objects.Unwrap(upload), 0, 0, 0, out nint scratch, out ulong offset));
        IndirectClamp.Done((XboxObject*)list, scratch);

        nint readback = device.Buffer(XboxDevice.ReadbackHeap, 256, XboxDevice.CopyDest);
        nint inner = ((XboxObject*)list)->Inner;
        ((delegate* unmanaged<nint, nint, ulong, nint, ulong, ulong, void>)XboxDevice.Vtable(inner)[CopyBufferRegionSlot])(inner, objects.Unwrap(readback), 0, scratch, offset, Stride * many);
        Assert.Equal(0, ((delegate* unmanaged<nint, int>)XboxDevice.Vtable(list)[CloseSlot])(list));
        device.Run(list);

        uint* clamped = Map(readback);
        int ran = Enumerable.Range(0, (int)many).Count(i => clamped[i * 3] != 0);
        Assert.Equal(2, ran);
    }

    private static uint* Map(nint resource)
    {
        void* data = null;
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, void*, void**, int>)XboxDevice.Vtable(resource)[MapSlot])(resource, 0, null, &data));

        return (uint*)data;
    }
}
