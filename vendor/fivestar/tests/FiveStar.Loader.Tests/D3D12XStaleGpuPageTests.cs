// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>Pages remapped under a slot-133 buffer: the desktop buffer keeps its frames, and the loader says where that leads.</summary>
public sealed unsafe class D3D12XStaleGpuPageTests
{
    private const ulong Page = 0x10000;
    private const uint Reserve64K = 0x20402000;
    private const ulong CpuFlags = 0x21;
    private const ulong GpuFlags = 0x3;
    private const uint CpuReadWrite = 0x4;
    private const uint GpuOnly = 0x801;
    private const uint Marker = 0xC0FFEE11;

    [Fact]
    public void A_frame_remapped_under_a_buffer_and_mapped_elsewhere_is_reported_where_the_gpu_writes_land()
    {
        using var log = new StringWriter();
        using var test = TestRuntime.Create(log: log);
        var virtualAlloc = (delegate* unmanaged<nint, nuint, uint, ulong, uint, nint>)test.Shim("xmem.dll", "XMemVirtualAlloc");
        var allocate = (delegate* unmanaged<ulong, ulong*, ulong*, int>)test.Shim("xmem.dll", "XMemAllocatePhysicalPages");
        var map = (delegate* unmanaged<nint, ulong, ulong*, nint>)test.Shim("xmem.dll", "XMemMapPhysicalPages");

        // The game's streaming pattern: a CPU range, its GPU-only alias, and another range pages move to.
        nint cpu = virtualAlloc(0, (nuint)Page, Reserve64K, CpuFlags, CpuReadWrite);
        nint gpu = virtualAlloc(0, (nuint)Page, Reserve64K, GpuFlags, GpuOnly);
        nint other = virtualAlloc(0, (nuint)Page, Reserve64K, CpuFlags, CpuReadWrite);

        ulong* frames = stackalloc ulong[3];
        ulong count = 3;
        Assert.Equal(1, allocate(0, &count, frames));
        Assert.Equal(cpu, map(cpu, 1, &frames[0]));

        // A buffer over the first frame; the game then maps the second and third there and moves the first elsewhere.
        var device = new XboxDevice(test);
        Assert.Equal(0, device.PlacedBuffer(gpu, Page, out nint buffer));

        Assert.Equal(cpu, map(cpu, 1, &frames[1]));
        Assert.Equal(cpu, map(cpu, 1, &frames[2]));
        Assert.Equal(other, map(other, 1, &frames[0]));
        *(uint*)cpu = 0;
        *(uint*)other = 0;

        var stale = test.Runtime.D3D12X.StalePages;
        Assert.Equal(2, stale.Remapped);
        Assert.Equal(1, stale.Reused);
        Assert.Contains($"(CPU 0x{cpu:x}, now 0x{frames[2]:x}): the GPU buffer at 0x{gpu:x} (0x10000 bytes) still reaches frame 0x{frames[0]:x}", log.ToString(), StringComparison.Ordinal);
        Assert.Contains($"frame 0x{frames[0]:x} mapped again at 0x{other:x}", log.ToString(), StringComparison.Ordinal);

        // The GPU writes the buffer at the game's address; the bytes reach the range the warning named.
        nint upload = device.Buffer(XboxDevice.UploadHeap, Page, XboxDevice.GenericRead);
        XboxDevice.WriteFirst(upload, Marker);
        device.Copy(buffer, upload, 0, Page);

        Assert.Equal(Marker, *(uint*)other);
        Assert.Equal(0u, *(uint*)cpu);
    }
}
