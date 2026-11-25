// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Shims.D3D12X.Present;
using FiveStar.Loader.Shims.D3D12X.States;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// Resource states through the game's slots: home states at creation, list slot 26 and Close, on the default adapter.
/// The debug layer can only be turned on before the process's first device, so it checks the states only when
/// FIVESTAR_D3D12_DEBUG is 1 and this test runs alone (<c>--filter D3D12XBarrierTests</c>).
/// </summary>
public sealed unsafe class D3D12XBarrierTests
{
    private const int CreateCommittedResourceSlot = 27;
    private const int CreateDescriptorHeapSlot = 14;
    private const int CreateRenderTargetViewSlot = 20;
    private const int HeapCpuStartSlot = 9;
    private const int CloseSlot = 9;
    private const int ResetSlot = 10;
    private const int CopyResourceSlot = 17;
    private const int ResourceBarrierSlot = 26;
    private const int ClearRenderTargetViewSlot = 52;
    private const int ExecuteSlot = 35;
    private const int SignalSlot = 16;
    private const int FenceWaitSlot = 9;
    private const int FenceValueSlot = 8;

    private const int DefaultHeap = 1;
    private const int Texture2D = 3;
    private const int Rgba8Unorm = 28;
    private const int RenderTargetAndUav = 0x5;
    private const int DepthStencilFlag = 0x2;
    private const int RtvHeapType = 2;
    private const int UnorderedAccess = 0x8;
    private const int RenderTarget = 0x4;
    private const int NonPixelShaderResource = 0x40;
    private const int PixelShaderResource = 0x80;
    private const int CopySource = 0x800;
    private const int CopyDest = 0x400;
    private const uint AllSubresources = 0xFFFFFFFF;
    private const uint Size = 64;
    private const uint RowPitch = 256;
    private const int ReadbackHeap = 3;
    private const int CopyResourceXSlot = 86;
    private const int CopyTextureRegionXSlot = 85;

    /// <summary>The clear color (0.25, 0.5, 0.75, 1) as R8G8B8A8_UNORM bytes.</summary>
    private const uint ClearedPixel = 0xFFBF8040;
    private const string DebugVariable = "FIVESTAR_D3D12_DEBUG";
    private static readonly Guid ResourceIid = new("696442be-a72e-4059-bc79-5b5c98040fad");
    private static readonly Guid HeapIid = new("8efb471d-616c-4f49-90f7-127bb763fa51");

    [Fact]
    public void A_game_barrier_moves_a_render_target_for_a_clear_and_a_copy_without_debug_layer_errors()
    {
        var log = new StringWriter();
        using var test = TestRuntime.Create(log: log);
        var d3d12x = test.Runtime.D3D12X;

        if (Environment.GetEnvironmentVariable(DebugVariable) == "1")
        {
            d3d12x.Debugging.Enable(false);
        }

        var device = new XboxDevice(test);
        nint queue = device.Queue();
        nint fence = device.Fence();

        // Created in UNORDERED_ACCESS, as the game creates its render targets; it rests in RENDER_TARGET.
        nint target = Texture(device, RenderTargetAndUav, UnorderedAccess);
        nint copy = Texture(device, 0, CopyDest);
        nint view = RenderTargetView(device, target);
        Assert.Equal(RenderTarget, d3d12x.Objects.AsResource(target)->HomeState);

        nint first = device.DirectList();
        Barrier(first, target, UnorderedAccess, RenderTarget);
        Clear(first, view);
        Barrier(first, target, RenderTarget, CopySource);
        ((delegate* unmanaged<nint, nint, nint, void>)XboxDevice.Vtable(first)[CopyResourceSlot])(first, copy, target);

        var states = d3d12x.ListStates.Find((XboxObject*)first);
        Assert.NotNull(states);
        Assert.Equal(CopySource, states.Current(d3d12x.Objects.Unwrap(target), RenderTarget));
        Submit(queue, fence, first, 1);
        Assert.Equal(0, states.Count);

        // Close moved the target home, so a list without a barrier clears it again.
        nint second = device.DirectList();
        Clear(second, view);
        Submit(queue, fence, second, 2);

        Assert.Equal(2ul, ((delegate* unmanaged<nint, ulong>)XboxDevice.Vtable(fence)[FenceValueSlot])(fence));
        Assert.DoesNotContain("[d3d12-debug] error", log.ToString(), StringComparison.Ordinal);
        Assert.DoesNotContain("[d3d12-debug] corruption", log.ToString(), StringComparison.Ordinal);
    }

    [Fact]
    public void The_console_texture_copies_move_textures_from_their_home_states_and_back()
    {
        var log = new StringWriter();
        using var test = TestRuntime.Create(log: log);

        if (Environment.GetEnvironmentVariable(DebugVariable) == "1")
        {
            test.Runtime.D3D12X.Debugging.Enable(false);
        }

        var device = new XboxDevice(test);
        nint queue = device.Queue();
        nint fence = device.Fence();
        nint target = Texture(device, RenderTargetAndUav, UnorderedAccess);
        nint copy = Texture(device, 0, PixelShaderResource);
        nint readback = device.Buffer(ReadbackHeap, Size * RowPitch, CopyDest);

        // Cleared in its home state (RENDER_TARGET), then CopyResourceX and CopyTextureRegionX with no game barrier.
        nint list = device.DirectList();
        Clear(list, RenderTargetView(device, target));
        ((delegate* unmanaged<nint, nint, nint, uint, void>)XboxDevice.Vtable(list)[CopyResourceXSlot])(list, copy, target, 0);

        var to = new FootprintLocation { Resource = readback, Type = 1, Format = Rgba8Unorm, Width = Size, Height = Size, Depth = 1, RowPitch = RowPitch };
        var from = new FootprintLocation { Resource = copy };
        ((delegate* unmanaged<nint, FootprintLocation*, uint, uint, uint, FootprintLocation*, void*, uint, void>)XboxDevice.Vtable(list)[CopyTextureRegionXSlot])(list, &to, 0, 0, 0, &from, null, 0);
        Submit(queue, fence, list, 1);

        Assert.Equal(ClearedPixel, XboxDevice.ReadFirst(readback));
        Assert.DoesNotContain("NOT IMPLEMENTED", log.ToString(), StringComparison.Ordinal);
        Assert.DoesNotContain("[d3d12-debug] error", log.ToString(), StringComparison.Ordinal);
    }

    [Fact]
    public void A_compute_list_that_moves_a_render_target_still_closes_and_resets()
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        nint target = Texture(device, RenderTargetAndUav, UnorderedAccess);
        nint list = device.ComputeList();

        // RENDER_TARGET (its home) and PIXEL_SHADER_RESOURCE are graphics states: the desktop Close would fail.
        Barrier(list, target, RenderTarget, NonPixelShaderResource | PixelShaderResource);
        Assert.Equal(0, ((delegate* unmanaged<nint, int>)XboxDevice.Vtable(list)[CloseSlot])(list));

        nint allocator = device.Allocator(XboxDevice.ComputeType);
        Assert.Equal(0, ((delegate* unmanaged<nint, nint, nint, int>)XboxDevice.Vtable(list)[ResetSlot])(list, allocator, 0));
    }

    [Theory]
    [InlineData(DepthFormats.ConsoleD16S8, DepthFormats.R32G8X24Typeless, DepthFormats.D32S8, DepthFormats.R32X8X24)]
    [InlineData(DepthFormats.ConsoleR16X8, DepthFormats.R32G8X24Typeless, DepthFormats.ConsoleR16X8, DepthFormats.R32X8X24)]
    [InlineData(DepthFormats.ConsoleX16G8, DepthFormats.R32G8X24Typeless, DepthFormats.ConsoleX16G8, DepthFormats.X32G8X24)]
    [InlineData(Rgba8Unorm, Rgba8Unorm, Rgba8Unorm, Rgba8Unorm)]
    public void Console_depth_formats_take_the_D32_FLOAT_S8X24_family(int console, int resource, int depthView, int shaderView)
    {
        Assert.Equal(resource, DepthFormats.Resource(console));
        Assert.Equal(depthView, DepthFormats.Desktop(console));
        Assert.Equal(shaderView, DepthFormats.ShaderView(console));
    }

    [Theory]
    [InlineData(UnorderedAccess, DepthStencilFlag, 0)]
    [InlineData(UnorderedAccess | CopySource, DepthStencilFlag, CopySource)]
    [InlineData(RenderTarget, RenderTargetAndUav, RenderTarget)]
    public void A_barrier_leaves_out_write_states_the_desktop_flags_do_not_allow(int state, int flags, int expected)
    {
        Assert.Equal(expected, ResourceStates.Allowed(state, flags));
    }

    private static nint Texture(XboxDevice device, int flags, int state)
    {
        int* properties = stackalloc int[] { DefaultHeap, 0, 0, 0, 0 };
        var desc = new ResourceDescription
        {
            Dimension = Texture2D,
            Width = Size,
            Height = Size,
            DepthOrArraySize = 1,
            MipLevels = 1,
            Format = Rgba8Unorm,
            SampleCount = 1,
            Flags = flags,
        };
        Guid iid = ResourceIid;
        nint resource = 0;
        var create = (delegate* unmanaged<nint, int*, int, ResourceDescription*, int, void*, Guid*, nint*, int>)XboxDevice.Vtable(device.Device)[CreateCommittedResourceSlot];
        Assert.Equal(0, create(device.Device, properties, 0, &desc, state, null, &iid, &resource));

        return resource;
    }

    private static nint RenderTargetView(XboxDevice device, nint resource)
    {
        var desc = new DescriptorHeapDescription { Type = RtvHeapType, Count = 1 };
        Guid iid = HeapIid;
        nint heap = 0;
        var create = (delegate* unmanaged<nint, DescriptorHeapDescription*, Guid*, nint*, int>)XboxDevice.Vtable(device.Device)[CreateDescriptorHeapSlot];
        Assert.Equal(0, create(device.Device, &desc, &iid, &heap));

        nint handle = ((delegate* unmanaged<nint, nint>)XboxDevice.Vtable(heap)[HeapCpuStartSlot])(heap);
        ((delegate* unmanaged<nint, nint, void*, nint, void>)XboxDevice.Vtable(device.Device)[CreateRenderTargetViewSlot])(device.Device, resource, null, handle);

        return handle;
    }

    /// <summary>List slot 26 with one transition as the game would write it (the console states it believes in).</summary>
    private static void Barrier(nint list, nint resource, int before, int after)
    {
        var barrier = TransitionBarrier.Of(resource, before, after);
        Assert.Equal(AllSubresources, barrier.Subresource);
        ((delegate* unmanaged<nint, uint, TransitionBarrier*, void>)XboxDevice.Vtable(list)[ResourceBarrierSlot])(list, 1, &barrier);
    }

    private static void Clear(nint list, nint view)
    {
        // Each channel is an exact multiple of 1/255, so GPUs and WARP round it alike (0.5 is 127.5 and rounds either way).
        float* color = stackalloc float[] { 64 / 255f, 128 / 255f, 191 / 255f, 1 };
        ((delegate* unmanaged<nint, nint, float*, uint, void*, void>)XboxDevice.Vtable(list)[ClearRenderTargetViewSlot])(list, view, color, 0, null);
    }

    /// <summary>D3D12_TEXTURE_COPY_LOCATION: a subresource index (0 here) or a placed footprint.</summary>
    [StructLayout(LayoutKind.Sequential)]
    private struct FootprintLocation
    {
        public nint Resource;
        public int Type;
        public ulong Offset;
        public int Format;
        public uint Width;
        public uint Height;
        public uint Depth;
        public uint RowPitch;
    }

    private static void Submit(nint queue, nint fence, nint list, ulong value)
    {
        Assert.Equal(0, ((delegate* unmanaged<nint, int>)XboxDevice.Vtable(list)[CloseSlot])(list));
        nint* lists = stackalloc nint[] { list };
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, nint*, nint, uint, int>)XboxDevice.Vtable(queue)[ExecuteSlot])(queue, 1, lists, 0, 0));
        Assert.Equal(0, ((delegate* unmanaged<nint, nint, ulong, int>)XboxDevice.Vtable(queue)[SignalSlot])(queue, fence, value));
        Assert.Equal(0, ((delegate* unmanaged<nint, ulong, nint, int>)XboxDevice.Vtable(fence)[FenceWaitSlot])(fence, value, 0));
    }
}
