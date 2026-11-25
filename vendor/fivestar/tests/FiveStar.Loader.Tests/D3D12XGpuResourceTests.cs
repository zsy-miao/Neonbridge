// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Shims.D3D12X.Resources;
using FiveStar.Loader.Shims.Xg;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>Textures and samplers at console GPU addresses: device slots 133, 70 and 61.</summary>
public sealed unsafe class D3D12XGpuResourceTests
{
    private const int Size = 64;
    private const int DefaultHeap = 1;
    private const ulong TextureBytes = 0x1000;
    private const int Mips = 2;
    private const int Rgba8Unorm = 28;
    private const int Texture2D = 3;
    private const int SrvTexture2D = 4;
    private const int DefaultComponentMapping = 0x1688;

    /// <summary>D3D12_TEX2D_SRV::MipLevels: the union starts at +0x10 (8-byte aligned), MostDetailedMip first.</summary>
    private const int TextureMipLevelsWord = 5;
    private const int ShaderResourceLayout = 6;
    private const int ShaderResourceState = 0xC0;
    private const ulong RangeBytes = 0x20000;
    private const uint Reserve64K = 0x20402000;
    private const uint CpuReadWrite = 0x4;
    private const uint GpuOnly = 0x801;
    private const int MapSlot = 8;
    private const int CloseSlot = 9;
    private const int CopyTextureRegionXSlot = 85;
    private const int UploadHeap = 2;
    private const int GenericRead = 0xAC3;

    [Fact]
    public void A_texture_whose_memory_went_to_another_is_released_after_its_grace_period()
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        var registry = GpuTextureRegistry.OfQueues();

        // Streamed: B takes part of A's memory. Aliased: C is another texture over exactly B's range.
        registry.Register(Texture(test, device, 0x10000));
        registry.Register(Texture(test, device, 0x10800));
        registry.Register(Texture(test, device, 0x10800));

        registry.Advance(GpuTextureRegistry.GraceFrames - 1);
        Assert.Equal(0, registry.Released);
        registry.Advance(GpuTextureRegistry.GraceFrames);

        Assert.Equal(1, registry.Released);
        Assert.Equal(1, registry.RetiredCount);
        registry.Dispose();
    }

    [Fact]
    public void A_retired_texture_waits_for_the_work_the_queues_had_submitted_when_it_was_retired()
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        ulong[] submitted = [5, 9];
        ulong[] completed = [5, 8];
        var registry = new GpuTextureRegistry(() => submitted, () => completed);

        registry.Register(Texture(test, device, 0x10000));
        registry.Register(Texture(test, device, 0x10800));

        // Past the grace, but queue 2 has not finished value 9: kept; then it has.
        registry.Advance(GpuTextureRegistry.GraceFrames);
        Assert.Equal(0, registry.Released);
        completed = [5, 9];
        registry.Advance(GpuTextureRegistry.GraceFrames + 1);
        Assert.Equal(1, registry.Released);

        Assert.True(GpuTextureRegistry.Finished([], []));
        Assert.False(GpuTextureRegistry.Finished([1, 2], [1]));
        registry.Dispose();
    }

    [Theory]
    [InlineData(GpuTextureRegistry.DefaultKeptAliases)]
    [InlineData(2)]
    public void Older_textures_over_one_range_are_released_after_the_longer_grace_and_the_newest_kept(int kept)
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        var registry = GpuTextureRegistry.OfQueues(kept);

        // A streaming slot reused four times: three retired over exactly its range, the older ones past those kept.
        for (int i = 0; i < 4; i++)
        {
            registry.Register(Texture(test, device, 0x10000));
        }

        registry.Advance(GpuTextureRegistry.AliasGraceFrames - 1);
        Assert.Equal(0, registry.Released);
        registry.Advance(GpuTextureRegistry.AliasGraceFrames);

        Assert.Equal(3 - kept, registry.Released);
        Assert.Equal(kept, registry.RetiredCount);
        registry.Dispose();
    }

    [Fact]
    public void A_placed_texture_gives_its_heap_range_back_when_it_is_destroyed()
    {
        using var test = TestRuntime.Create();
        _ = new XboxDevice(test);
        nint device = test.Runtime.D3D12X.Objects.DesktopDevice;
        var heaps = new PlacedTextureHeaps();
        var description = new ResourceDescription { Dimension = Texture2D, Width = Size, Height = Size, DepthOrArraySize = 1, MipLevels = 1, Format = Rgba8Unorm, SampleCount = 1 };

        Assert.Equal(0, heaps.Create(device, &description, ShaderResourceState, null, out nint first));
        Assert.StartsWith("placed textures: 1 live in 1 heaps", heaps.Summary, StringComparison.Ordinal);

        // The runtime releases the range's owner with the texture; the next texture takes the range again.
        FiveStar.Loader.Native.ComObject.Release(first);
        Assert.StartsWith("placed textures: 0 live in 1 heaps", heaps.Summary, StringComparison.Ordinal);
        Assert.Equal(0, heaps.Create(device, &description, ShaderResourceState, null, out nint second));
        Assert.StartsWith("placed textures: 1 live in 1 heaps", heaps.Summary, StringComparison.Ordinal);
        FiveStar.Loader.Native.ComObject.Release(second);
    }

    [Fact]
    public void A_shared_heap_that_stays_empty_goes_back_but_one_empty_heap_stays()
    {
        using var test = TestRuntime.Create();
        _ = new XboxDevice(test);
        nint device = test.Runtime.D3D12X.Objects.DesktopDevice;
        var heaps = new PlacedTextureHeaps { EmptyMilliseconds = 0 };

        // Five 64 MB textures: four fill the first 256 MB heap, the fifth starts a second.
        var description = new ResourceDescription { Dimension = Texture2D, Width = 4096, Height = 4096, DepthOrArraySize = 1, MipLevels = 1, Format = Rgba8Unorm, SampleCount = 1 };
        var textures = new List<nint>();

        for (int i = 0; i < 5; i++)
        {
            Assert.Equal(0, heaps.Create(device, &description, ShaderResourceState, null, out nint texture));
            textures.Add(texture);
        }

        Assert.Contains("live in 2 heaps (0.5 GB, 0.3 GB used)", heaps.Summary, StringComparison.Ordinal);
        textures.ForEach(texture => FiveStar.Loader.Native.ComObject.Release(texture));
        Assert.Contains("0 live in 1 heaps (0.2 GB, 0.0 GB used), 1 empty heaps returned", heaps.Summary, StringComparison.Ordinal);
    }

    [Fact]
    public void A_tiled_texture_written_by_the_cpu_reads_back_linear_through_a_slot_70_view()
    {
        using var test = TestRuntime.Create();
        var (cpu, gpu) = AliasedPages(test);
        var device = new XboxDevice(test);
        var description = Description();

        // Slot 133 first (as the game does), while the memory is still empty; the CPU then writes the console tiling.
        nint placed = PlaceTexture(device, gpu, description);
        byte[][] linear = [Pattern(Size, 0), Pattern(Size / 2, 1)];
        Tile(description, cpu, linear);

        using var probe = new TextureProbe(test.Runtime.D3D12X.Objects.DesktopDevice);
        CreateView(device, gpu, description, (nint)probe.Heap.GetCPUDescriptorHandleForHeapStart().Ptr);

        for (uint mip = 0; mip < Mips; mip++)
        {
            uint extent = (uint)Size >> (int)mip;
            uint[] texels = probe.Read(extent, extent, mip);
            Assert.Equal(linear[mip], texels.SelectMany(BitConverter.GetBytes).ToArray());
        }

        var counters = test.Runtime.D3D12X.Textures.Counters;
        Assert.NotEqual(0, placed);
        Assert.Equal(1, counters[GpuResourceEvent.ViewResolved]);

        // The empty memory at creation, then the tiled bytes the view found changed.
        Assert.Equal(2, counters[GpuResourceEvent.Upload]);
    }

    [Fact]
    public void A_mapped_texture_a_gpu_copy_wrote_keeps_the_copy_when_its_memory_changes()
    {
        using var test = TestRuntime.Create();
        var (cpu, gpu) = AliasedPages(test);
        var device = new XboxDevice(test);
        var description = Description();
        nint device12 = test.Runtime.D3D12X.Objects.DesktopDevice;
        var textures = test.Runtime.D3D12X.Textures;

        // The game maps the texture (checked every frame) and writes it; the frame check uploads that.
        nint placed = PlaceTexture(device, gpu, description);
        void* mapped = null;
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, void*, void**, int>)XboxDevice.Vtable(placed)[MapSlot])(placed, 0, null, &mapped));
        Tile(description, cpu, [Pattern(Size, 4), Pattern(Size / 2, 5)]);
        textures.RefreshWatched(device12, 1);

        // A glyph-cache style update: list slot 85 copies new texels from an upload buffer into mip 0.
        byte[] copied = Pattern(Size, 6);
        nint upload = device.Buffer(UploadHeap, (ulong)copied.Length, GenericRead);
        void* staging = null;
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, void*, void**, int>)XboxDevice.Vtable(upload)[MapSlot])(upload, 0, null, &staging));
        copied.CopyTo(new Span<byte>(staging, copied.Length));

        nint list = device.DirectList();
        var to = new CopyLocation { Resource = placed };
        var from = new CopyLocation { Resource = upload, Type = 1, Format = Rgba8Unorm, Width = Size, Height = Size, Depth = 1, RowPitch = Size * sizeof(uint) };
        ((delegate* unmanaged<nint, CopyLocation*, uint, uint, uint, CopyLocation*, void*, uint, void>)XboxDevice.Vtable(list)[CopyTextureRegionXSlot])(list, &to, 0, 0, 0, &from, null, 0);
        Assert.Equal(0, ((delegate* unmanaged<nint, int>)XboxDevice.Vtable(list)[CloseSlot])(list));
        device.Run(list);

        // The memory changes again (on the console it would now hold the copy): the frame check must not undo the copy.
        Tile(description, cpu, [Pattern(Size, 7), Pattern(Size / 2, 8)]);
        textures.RefreshWatched(device12, 2);

        using var probe = new TextureProbe(device12);
        CreateView(device, gpu, description, (nint)probe.Heap.GetCPUDescriptorHandleForHeapStart().Ptr);
        Assert.Equal(copied, probe.Read(Size, Size, 0).SelectMany(BitConverter.GetBytes).ToArray());
        Assert.True(textures.Counters[GpuResourceEvent.UploadKeptCopy] >= 1);
    }

    [Fact]
    public void A_texture_whose_upload_buffer_the_desktop_refuses_is_left_unfilled_and_filled_later()
    {
        using var test = TestRuntime.Create();
        var (cpu, gpu) = AliasedPages(test);
        var device = new XboxDevice(test);
        var description = Description();
        var uploader = test.Runtime.D3D12X.Textures.Uploader;
        var refuse = new SharpGen.Runtime.SharpGenException(SharpGen.Runtime.Result.OutOfMemory);

        var normal = uploader.CreateStaging;

        // An upload buffer is system memory, which the heaps over the game's sections share; on a 16 GB machine they
        // took it all, the fill threw on the game's thread and the process ended (0xE0434352). Slot 133 succeeds.
        uploader.CreateStaging = (_, _) => throw refuse;
        nint placed = PlaceTexture(device, gpu, description);
        var counters = test.Runtime.D3D12X.Textures.Counters;
        Assert.NotEqual(0, placed);
        Assert.Equal(1, counters[GpuResourceEvent.UploadFailed]);
        Assert.Equal(0, counters[GpuResourceEvent.Upload]);

        // Once the desktop gives the buffer again, the view's refresh fills the texture.
        uploader.CreateStaging = normal;
        byte[][] linear = [Pattern(Size, 2), Pattern(Size / 2, 3)];
        Tile(description, cpu, linear);
        using var probe = new TextureProbe(test.Runtime.D3D12X.Objects.DesktopDevice);
        CreateView(device, gpu, description, (nint)probe.Heap.GetCPUDescriptorHandleForHeapStart().Ptr);

        Assert.Equal(linear[0], probe.Read(Size, Size, 0).SelectMany(BitConverter.GetBytes).ToArray());
        Assert.Equal(1, counters[GpuResourceEvent.Upload]);
    }

    [Fact]
    public void A_console_sampler_desc_becomes_the_desktop_sampler_it_describes()
    {
        var console = new ConsoleSamplerDescription
        {
            MagFilter = 2, MinFilter = 1, MipFilter = 2, AddressU = 1, AddressV = 3, AddressW = 4,
            MinLod = 1, MaxLod = 9, MipLodBias = -0.5f, MaxAnisotropy = 0, ComparisonFunc = 4,
        };
        console.Border[3] = 1;

        var desktop = ConsoleSamplers.ToDesktop(console);

        // D3D12_FILTER_COMPARISON_MIN_POINT_MAG_LINEAR_MIP_LINEAR: comparison reduction, mag and mip linear.
        Assert.Equal(0x80 | 0x4 | 0x1, desktop.Filter);
        Assert.Equal((1, 3, 4), (desktop.AddressU, desktop.AddressV, desktop.AddressW));
        Assert.Equal((1f, 9f, -0.5f), (desktop.MinLod, desktop.MaxLod, desktop.MipLodBias));
        Assert.Equal((1, 4, 1f), (desktop.MaxAnisotropy, desktop.ComparisonFunc, desktop.Border[3]));

        var anisotropic = new ConsoleSamplerDescription { MagFilter = 3, MinFilter = 3, MaxAnisotropy = 8, ComparisonFunc = 1 };
        Assert.Equal(0x55, ConsoleSamplers.ToDesktop(anisotropic).Filter);
        Assert.Equal(8, ConsoleSamplers.ToDesktop(anisotropic).MaxAnisotropy);
    }

    /// <summary>The game's streaming pattern: a CPU range, then a GPU-only range of the same size, pages mapped into the first.</summary>
    private static (nint Cpu, nint Gpu) AliasedPages(TestRuntime test)
    {
        var virtualAlloc = (delegate* unmanaged<nint, nuint, uint, ulong, uint, nint>)test.Shim("xmem.dll", "XMemVirtualAlloc");
        var allocate = (delegate* unmanaged<ulong, ulong*, ulong*, int>)test.Shim("xmem.dll", "XMemAllocatePhysicalPages");
        var map = (delegate* unmanaged<nint, ulong, ulong*, nint>)test.Shim("xmem.dll", "XMemMapPhysicalPages");

        nint cpu = virtualAlloc(0, (nuint)RangeBytes, Reserve64K, 0x21, CpuReadWrite);
        nint gpu = virtualAlloc(0, (nuint)RangeBytes, Reserve64K, 0x3, GpuOnly);
        ulong* pages = stackalloc ulong[2];
        ulong count = 2;
        Assert.Equal(1, allocate(0, &count, pages));
        Assert.Equal(cpu, map(cpu, 2, pages));

        return (cpu, gpu);
    }

    /// <summary>A registry entry over 0x1000 bytes at <paramref name="address"/>, holding its own reference on a desktop buffer.</summary>
    private static GpuTexture Texture(TestRuntime test, XboxDevice device, ulong address)
    {
        nint desktop = test.Runtime.D3D12X.Objects.Unwrap(device.Buffer(DefaultHeap, Size, 0));
        _ = ((delegate* unmanaged<nint, uint>)XboxDevice.Vtable(desktop)[1])(desktop);

        return new GpuTexture(address, Description(), desktop, Rgba8Unorm, 0, false, new XgSurface { Size = TextureBytes });
    }

    private static ResourceDescription Description()
    {
        var description = new ResourceDescription
        {
            Dimension = Texture2D, Width = Size, Height = Size, DepthOrArraySize = 1, MipLevels = Mips,
            Format = Rgba8Unorm, SampleCount = 1,
        };
        description.Layout = (int)(XgSwizzleModes.ModeBase | XgSwizzleModes.Suggest(Size * Size * sizeof(uint)));

        return description;
    }

    private static nint PlaceTexture(XboxDevice device, nint address, ResourceDescription description)
    {
        var iid = new Guid("696442be-a72e-4059-bc79-5b5c98040fad");
        nint resource = 0;
        var create = (delegate* unmanaged<nint, nint, ResourceDescription*, uint, void*, Guid*, nint*, int>)XboxDevice.Vtable(device.Device)[133];
        Assert.Equal(0, create(device.Device, address, &description, ShaderResourceLayout, null, &iid, &resource));

        return resource;
    }

    /// <summary>Device slot 70 with the game's view desc: the desktop Texture2D SRV desc and the address at +0x28.</summary>
    private static void CreateView(XboxDevice device, nint address, ResourceDescription description, nint handle)
    {
        var view = new AddressViewDescription { Format = Rgba8Unorm, ViewDimension = SrvTexture2D, Address = (ulong)address };
        var words = (int*)&view;
        words[2] = DefaultComponentMapping;
        words[TextureMipLevelsWord] = Mips;
        var create = (delegate* unmanaged<nint, ResourceDescription*, AddressViewDescription*, nint, void>)XboxDevice.Vtable(device.Device)[70];
        create(device.Device, &description, &view, handle);
    }

    private static void Tile(ResourceDescription description, nint memory, byte[][] mips)
    {
        var xg = new XgResourceDesc
        {
            Dimension = Texture2D, Width = Size, Height = Size, DepthOrArraySize = 1, MipLevels = Mips,
            Format = Rgba8Unorm, SampleCount = 1, Layout = (uint)description.Layout,
        };
        XgLayoutBuilder.Build(xg, out var surface);
        Assert.True(surface.Size <= RangeBytes);

        for (int mip = 0; mip < Mips; mip++)
        {
            uint pitch = (uint)(Size >> mip) * sizeof(uint);

            fixed (byte* linear = mips[mip])
            {
                Assert.Equal(0, XgCopier.Into(surface, (byte*)memory, 0, (uint)mip, linear, pitch, pitch * (uint)(Size >> mip)));
            }
        }
    }

    /// <summary>D3D12_TEXTURE_COPY_LOCATION: a subresource index (0 here) or a placed footprint.</summary>
    [System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
    private struct CopyLocation
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

    private static byte[] Pattern(int extent, int seed)
    {
        var bytes = new byte[extent * extent * sizeof(uint)];

        for (int index = 0; index < bytes.Length; index++)
        {
            bytes[index] = (byte)((index * 7) + (index / 251) + (seed * 31));
        }

        return bytes;
    }
}
