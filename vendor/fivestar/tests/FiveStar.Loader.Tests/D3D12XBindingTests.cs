// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Graphics.Pipelines;
using FiveStar.Loader.Shims.D3D12X.Binding;
using FiveStar.Loader.Tests.Support;
using FiveStar.Shaders.Translation;
using Vortice.Direct3D12;
using Xunit;
using HeapProperties = FiveStar.Loader.Shims.D3D12X.Binding.HeapProperties;

namespace FiveStar.Loader.Tests;

/// <summary>
/// Command-list binding through the D3D12.X slot numbers: root signature layouts, a dispatch whose descriptor table
/// a program reads the way translated programs do (user SGPR, window record, heap index), and game memory reached
/// at a console GPU address through the address map with the translator's own runtime (rdna_prelude.hlsli).
/// </summary>
public sealed unsafe class D3D12XBindingTests
{
    private const uint Marker = 0x1234;
    private const int CreateDescriptorHeapSlot = 14;
    private const int CreateRootSignatureSlot = 16;
    private const int CreateShaderResourceViewSlot = 18;
    private const int CreateUnorderedAccessViewSlot = 19;
    private const int CreateSamplerSlot = 22;
    private const int DescriptorIncrementSlot = 15;
    private const int HeapCpuStartSlot = 9;
    private const int HeapGpuStartSlot = 10;
    private const int ListCloseSlot = 9;
    private const int ListDispatchSlot = 14;
    private const int ListSetPipelineSlot = 25;
    private const int ListSetHeapsSlot = 32;
    private const int ListSetComputeRootSlot = 33;
    private const int ListSetComputeTableSlot = 35;
    private const int QueueExecuteSlot = 35;
    private const int QueueSignalSlot = 16;
    private const int FenceWaitSlot = 9;
    private const int GenericRead = 0xAC3;
    private const int WriteBack = 3;

    /// <summary>The game's main view heap (0x141cf3280), a heap that fits the desktop, and a page far from both ends.</summary>
    private const uint GameViews = 1245184;
    private const uint SmallHeap = 16;

    /// <summary>The composite's table: 7 records in its root signature, its constants at record 41.</summary>
    private const uint ComposedTable = 64;
    private const uint CompositeExtent = 7;
    private const uint CompositeRecord = 41;
    private const uint FarPage = 500000;
    private const uint TableSize = 2;
    private const uint StructureStride = 16;

    /// <summary>DXGI_FORMAT_R16G16B16A16_FLOAT, R8G8B8A8_UNORM and B8G8R8A8_UNORM.</summary>
    private const int HalfFloat4 = 10;
    private const int Byte4Unorm = 28;
    private const int Byte4UnormBgra = 87;

    /// <summary>
    /// Hardware V# dword 3 (RDNA 2 ISA): RESOURCE_LEVEL 1 (bit 24), the unified format (bits 12-18: 20 32_UINT, 71
    /// 16_16_16_16_FLOAT, 56 8_8_8_8_UNORM) and DST_SEL x y z w (0xFAC) or z y x w (0xF2E).
    /// </summary>
    private const uint Dword = 0x01014FAC;
    private const uint Half4 = 0x01047FAC;
    private const uint Byte4 = 0x01038FAC;
    private const uint Byte4Bgra = 0x01038F2E;

    /// <summary>STRIDE: dword 1 bits 16-29.</summary>
    private const int StrideShift = 16;

    /// <summary>Reads table entry 0 (an SRV) and 1 (a UAV) through the window and stores the SRV's first dword + 1.</summary>
    private const string Program = """
        cbuffer UserData : register(b0) { uint4 g_user4[32]; };
        [numthreads(1, 1, 1)]
        void main()
        {
            uint table = g_user4[1].x;
            ByteAddressBuffer window = ResourceDescriptorHeap[0];
            ByteAddressBuffer source = ResourceDescriptorHeap[window.Load(table)];
            RWByteAddressBuffer destination = ResourceDescriptorHeap[window.Load(table + 32)];
            destination.Store(0, source.Load(0) + 1);
        }
        """;

    /// <summary>As <see cref="Program"/> through the translator's runtime: the table address is user SGPR s4 (compute).</summary>
    private const string PreludeProgram = """
        #include "rdna_prelude.hlsli"
        [numthreads(1, 1, 1)]
        void main()
        {
            uint table = user_sgpr(4);
            ByteAddressBuffer window = ResourceDescriptorHeap[0];
            ByteAddressBuffer source = ResourceDescriptorHeap[window.Load(table)];
            RWByteAddressBuffer destination = ResourceDescriptorHeap[window.Load(table + 32)];
            destination.Store(0, source.Load(0) + 1);
        }
        """;

    /// <summary>
    /// As <see cref="PreludeProgram"/> with heap-relative addresses the program makes itself, as the game's root
    /// constants carry them: views at base 0 + index * 32 (MS_VS_Transform_PositionOnly), game indices 3 and 4, and an
    /// S# at base 0 + index * 16 (Scaleform's FTexTG), sampler 7, whose record names desktop sampler 7. A view index
    /// past the desktop heap reads the null view (prelude view_slot).
    /// </summary>
    private const string HeapRelativeProgram = """
        #include "rdna_prelude.hlsli"
        [numthreads(1, 1, 1)]
        void main()
        {
            ByteAddressBuffer source = ResourceDescriptorHeap[PtrLoad(0u, 0u, 3u * 32u)];
            RWByteAddressBuffer destination = ResourceDescriptorHeap[PtrLoad(0u, 0u, 4u * 32u)];
            destination.Store(0, source.Load(0) + SamplerPtrLoad(0u, 0u, 7u * 16u) + BufLoad(0x7fffffffu, 0u));
        }
        """;

    private const uint HeapRelativeRead = 3;
    private const uint HeapRelativeSampler = 7;
    private const int SamplerHeapType = 1;

    private const int DwordsPerVector = 4;

    /// <summary>
    /// Table entry 0 holds a console GPU address (low, high) and a value: the program stores the value there and stores
    /// the dword 4 bytes further on into table entry 1, both through the address map (PtrStore, PtrLoad).
    /// </summary>
    private const string AddressMapProgram = """
        #include "rdna_prelude.hlsli"
        [numthreads(1, 1, 1)]
        void main()
        {
            uint table = g_user4[1].x;
            ByteAddressBuffer window = ResourceDescriptorHeap[0];
            ByteAddressBuffer source = ResourceDescriptorHeap[window.Load(table)];
            RWByteAddressBuffer destination = ResourceDescriptorHeap[window.Load(table + 32)];
            uint3 access = source.Load3(0);
            PtrStore(access.x, access.y, 0u, access.z);
            destination.Store(0, PtrLoad(access.x, access.y, 4u));
        }
        """;

    /// <summary>One 64 KiB XMem page; MEM_RESERVE | MEM_64K_PAGES with the game's XMem flags and protections for a CPU range and its GPU-only alias.</summary>
    private const ulong Page = 0x10000;
    private const uint Reserve64K = 0x20402000;
    private const ulong CpuFlags = 0x21;
    private const ulong GpuFlags = 0x3;
    private const uint CpuReadWrite = 0x4;
    private const uint GpuOnly = 0x801;
    private const uint Stored = 0x5EED;

    [Fact]
    public void A_root_signature_packs_its_parameters_into_user_sgprs()
    {
        var blob = Serialize(
            new RootParameter1(new RootDescriptorTable1(new DescriptorRange1(DescriptorRangeType.ShaderResourceView, 1, 0)), ShaderVisibility.All),
            new RootParameter1(new RootConstants(0, 0, 3), ShaderVisibility.All),
            new RootParameter1(RootParameterType.ShaderResourceView, new RootDescriptor1(1, 0), ShaderVisibility.All),
            new RootParameter1(new RootDescriptorTable1(new DescriptorRange1(DescriptorRangeType.Sampler, 1, 0)), ShaderVisibility.All));

        Assert.True(RootSignatureParser.TryParse(blob, out var layout));
        Assert.Equal(4, layout.Count);
        Assert.Equal(new byte[] { 0, 1, 4, 6 }, new[] { layout.Offsets[0], layout.Offsets[1], layout.Offsets[2], layout.Offsets[3] });
        Assert.Equal(new byte[] { 1, 3, 2, 1 }, new[] { layout.Sizes[0], layout.Sizes[1], layout.Sizes[2], layout.Sizes[3] });
        Assert.Equal(7, layout.Sgprs());

        // A draw uploads the dwords up to the root signature's last SGPR, in 16-byte steps: compute s4 + 7, graphics s8 + 7.
        var bindings = (CommandListBindings*)System.Runtime.InteropServices.NativeMemory.AllocZeroed((nuint)sizeof(CommandListBindings));
        Assert.Equal((uint)CommandListBindings.UserDataDwords, UserData.Used(bindings, true));

        bindings->ComputeLayout = &layout;
        bindings->GraphicsLayout = &layout;
        Assert.Equal(12u, UserData.Used(bindings, true));
        Assert.Equal(16u, UserData.Used(bindings, false));
        System.Runtime.InteropServices.NativeMemory.Free(bindings);
    }

    [Fact]
    public void The_remap_gives_touched_pages_desktop_pages_in_order_and_keeps_each_contiguous()
    {
        var remap = new DescriptorRemap(DescriptorWindow.ReservedViews, DescriptorWindow.MaxViews, GameViews);
        uint high = GameViews - 1;

        Assert.Equal(DescriptorWindow.ReservedViews + (high % DescriptorRemap.PageSize), remap.Map(high));
        Assert.Equal(remap.Map(high) - 1, remap.Map(high - 1));
        Assert.Equal(DescriptorWindow.ReservedViews + DescriptorRemap.PageSize, remap.Map(0));

        for (uint page = 1; page < remap.Capacity - 1; page++)
        {
            Assert.NotEqual(DescriptorRemap.Unmapped, remap.Map(page * DescriptorRemap.PageSize));
        }

        Assert.Equal(remap.Capacity, remap.MappedPages);
        Assert.Equal(DescriptorRemap.Unmapped, remap.Map(high - DescriptorRemap.PageSize));
    }

    private const uint RawBounds = 3u << 28;
    private const uint IndexBounds = 1u << 28;

    [Fact]
    public void A_buffer_view_record_carries_stride_count_and_format_into_the_window()
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        var heap = ViewHeap(device, SmallHeap);
        nint buffer = Buffer(test.Runtime.D3D12X.Objects.DesktopDevice, new HeapProperties { Type = HeapProperties.Upload }, 0, GenericRead);
        nint writable = Buffer(test.Runtime.D3D12X.Objects.DesktopDevice, new HeapProperties { Type = HeapProperties.Custom, CpuPageProperty = WriteBack, MemoryPool = HeapProperties.SystemPool }, DesktopCalls.UnorderedAccessFlag, 0);

        var structured = new BufferViewDescription { Dimension = BufferViewDescription.BufferDimension, SrvMapping = BufferViewDescription.DefaultMapping, SrvElements = 4, SrvStride = StructureStride };
        var raw = BufferViewDescription.RawShaderResource(2);
        var typed = new BufferViewDescription { Format = HalfFloat4, Dimension = BufferViewDescription.BufferDimension, SrvMapping = BufferViewDescription.DefaultMapping, SrvElements = 8 };
        var typedUav = new BufferViewDescription { Format = Byte4Unorm, Dimension = BufferViewDescription.BufferDimension, UavElements = 16 };
        var bgra = new BufferViewDescription { Format = Byte4UnormBgra, Dimension = BufferViewDescription.BufferDimension, SrvMapping = BufferViewDescription.DefaultMapping, SrvElements = 2 };

        var srv = (delegate* unmanaged<nint, nint, BufferViewDescription*, nint, void>)XboxDevice.Vtable(device.Device)[CreateShaderResourceViewSlot];
        var uav = (delegate* unmanaged<nint, nint, nint, BufferViewDescription*, nint, void>)XboxDevice.Vtable(device.Device)[CreateUnorderedAccessViewSlot];

        srv(device.Device, buffer, &structured, heap.Cpu);
        srv(device.Device, buffer, &raw, heap.Cpu + (nint)heap.Increment);
        srv(device.Device, buffer, &typed, heap.Cpu + (nint)(2 * heap.Increment));
        uav(device.Device, writable, 0, &typedUav, heap.Cpu + (nint)(3 * heap.Increment));
        srv(device.Device, buffer, &bgra, heap.Cpu + (nint)(4 * heap.Increment));

        // V# dwords 1 to 3 as the hardware lays them out: the idxen stride over base_hi 0 (dword 0 is a heap index),
        // num_records (elements of the stride, bytes without one) and the format word with its destination swizzle and its
        // OOB_SELECT: structured 0, raw 3, typed 1.
        Assert.Equal([StructureStride << StrideShift, 4u, Dword], Fields(test, 0));
        Assert.Equal([0u, 8u, Dword | RawBounds], Fields(test, 1));
        Assert.Equal([8u << StrideShift, 8u, Half4 | IndexBounds], Fields(test, 2));
        Assert.Equal([4u << StrideShift, 16u, Byte4 | IndexBounds], Fields(test, 3));
        Assert.Equal([4u << StrideShift, 2u, Byte4Bgra | IndexBounds], Fields(test, 4));
    }

    [Fact]
    public void A_buffer_view_record_holds_its_console_base_and_the_window_mirrors_it()
    {
        const uint Increment = 32;
        const uint Records = 32;

        // A game view's desktop index: past the reserved buffers and null textures (DescriptorWindow.ReservedViews).
        const uint Index = 24;
        const nint DesktopStart = 0x1000;
        const ulong Address = 0x27E_0005_0010;

        byte* title = (byte*)System.Runtime.InteropServices.NativeMemory.AllocZeroed(Records * Increment);
        byte* data = (byte*)System.Runtime.InteropServices.NativeMemory.AllocZeroed(DescriptorWindow.ViewBase + (Records * DescriptorWindow.ViewRecordBytes));

        try
        {
            var window = new DescriptorWindow(new FiveStar.Loader.Diagnostics.LoaderLog(TextWriter.Null, null, false));
            typeof(DescriptorWindow).GetProperty(nameof(DescriptorWindow.Data))!.SetValue(window, System.Reflection.Pointer.Box(data, typeof(byte*)));
            var heap = new ShaderVisibleHeap((nint)title, Records, Increment, DesktopStart, DescriptorWindow.ViewRecords(Records), null, window, null);

            // A raw view from element 1 of a buffer at Address: its base is Address + 4, as the console's V#.
            heap.Destination(Index, 1, new BufferRecord(0, 256, Dword | RawBounds).At(Address, 1));
            heap.Destination(Index + 1, 1, new BufferRecord(0, 256, Dword | RawBounds));

            uint[] based = [0x0005_0014, 0x27E, 256, Dword | RawBounds, Index];
            Assert.Equal(based, new ReadOnlySpan<uint>(title + (Index * Increment), based.Length).ToArray());
            Assert.Equal(based, new ReadOnlySpan<uint>(data + DescriptorWindow.ViewBase + (Index * DescriptorWindow.ViewRecordBytes), based.Length).ToArray());
            Assert.Equal(DesktopStart + (nint)(Index * Increment), heap.Source(Index));

            // Without a console address dword 0 stays the desktop index, base_hi 0.
            uint* plain = (uint*)(data + DescriptorWindow.ViewBase + ((Index + 1) * DescriptorWindow.ViewRecordBytes));
            Assert.Equal([Index + 1, 0u], new[] { plain[0], plain[1] });

            // A copy in from a desktop heap writes no fields; the source view's, noted by its handle, are set after it
            // over the desktop descriptor the record names.
            const nint DesktopHeapView = 0x7FFF_0000;
            DesktopRecords.Note(DesktopHeapView, new BufferRecord(0, 256, Dword | RawBounds).At(Address, 1));
            heap.Destination(Index + 2, 1, BufferRecord.None);
            Assert.True(DesktopRecords.TryFind(DesktopHeapView, out var copied));
            heap.SetFields(Index + 2, copied);

            uint[] carried = [0x0005_0014, 0x27E, 256, Dword | RawBounds, Index + 2];
            Assert.Equal(carried, new ReadOnlySpan<uint>(data + DescriptorWindow.ViewBase + ((Index + 2) * DescriptorWindow.ViewRecordBytes), carried.Length).ToArray());
            Assert.Equal(DesktopStart + (nint)((Index + 2) * Increment), heap.Source(Index + 2));
        }
        finally
        {
            System.Runtime.InteropServices.NativeMemory.Free(title);
            System.Runtime.InteropServices.NativeMemory.Free(data);
        }
    }

    [Fact]
    public void A_read_view_the_address_map_covers_and_a_UAV_over_its_own_resource_get_their_console_base()
    {
        const ulong Address = 0x27E_0006_0000;
        using var test = TestRuntime.Create();
        _ = new XboxDevice(test);
        nint resource = Buffer(test.Runtime.D3D12X.Objects.DesktopDevice, new HeapProperties { Type = HeapProperties.Upload }, 0, GenericRead);
        test.Runtime.D3D12X.PlacedResources.Addresses.Add(new FiveStar.Loader.Shims.D3D12X.AddressRange(Address, 0x10000, DesktopCalls.GpuAddress(resource), resource));
        var raw = new BufferRecord(0, 256, Dword | RawBounds);

        Assert.Equal(Address + 4, BufferRecords.Based(raw, Address, 1, resource, ViewKind.UnorderedAccess).Base);
        Assert.Equal(0ul, BufferRecords.Based(raw, Address, 1, resource + 1, ViewKind.UnorderedAccess).Base);
        Assert.Equal(0ul, BufferRecords.Based(raw, Address + 0x10000, 0, resource, ViewKind.ShaderResource).Base);
        Assert.Equal(Address, BufferRecords.Based(raw, Address, 0, 0, ViewKind.ConstantBuffer).Base);
        Assert.Equal(Address + 4, BufferRecords.Based(raw, Address, 1, resource + 1, ViewKind.ShaderResource).Base);
        Assert.Equal(0ul, BufferRecords.Based(raw, 0, 1, resource, ViewKind.ShaderResource).Base);
    }

    [Fact]
    public void A_read_view_of_XMem_pages_the_address_map_covers_gets_its_console_base()
    {
        using var test = TestRuntime.Create();
        var virtualAlloc = (delegate* unmanaged<nint, nuint, uint, ulong, uint, nint>)test.Shim("xmem.dll", "XMemVirtualAlloc");
        var allocate = (delegate* unmanaged<ulong, ulong*, ulong*, int>)test.Shim("xmem.dll", "XMemAllocatePhysicalPages");
        var map = (delegate* unmanaged<nint, ulong, ulong*, nint>)test.Shim("xmem.dll", "XMemMapPhysicalPages");

        nint cpu = virtualAlloc(0, (nuint)Page, Reserve64K, CpuFlags, CpuReadWrite);
        ulong frame = 0;
        ulong count = 1;
        Assert.Equal(1, allocate(0, &count, &frame));
        Assert.Equal(cpu, map(cpu, 1, &frame));

        // XMem pages the game mapped have views in the address map without any placed buffer over them.
        var device = new XboxDevice(test);
        _ = ViewHeap(device, GameViews);
        var raw = new BufferRecord(0, 256, Dword | RawBounds);

        Assert.True(test.Runtime.D3D12X.MemoryViews.Maps((ulong)cpu, 256));
        Assert.Equal((ulong)cpu + 4, BufferRecords.Based(raw, (ulong)cpu, 1, 0, ViewKind.AddressView).Base);
        Assert.Equal(0ul, BufferRecords.Based(raw, (ulong)cpu, 1, 0, ViewKind.UnorderedAccess).Base);
    }

    [DxcFact]
    public void A_descriptor_table_set_through_the_game_slots_reaches_the_program()
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        var heap = ViewHeap(device, SmallHeap);

        Assert.Equal(Marker + 1, Dispatch(test, device, heap, 0, 1, heap.Gpu));
    }

    [Fact]
    public void User_data_as_root_constants_fits_the_root_signature_and_the_prelude_reads_as_many()
    {
        var prelude = RuntimeIncludes.Read("rdna_prelude.hlsli");
        int start = prelude.IndexOf("#ifdef " + BindlessRootSignature.UserConstantsDefine, StringComparison.Ordinal);
        string constants = prelude[start..prelude.IndexOf("#else", start, StringComparison.Ordinal)];

        Assert.True(BindlessRootSignature.Dwords(userConstants: true) <= BindlessRootSignature.MaxDwords);
        Assert.Contains($"g_user4[{BindlessRootSignature.UserConstants / DwordsPerVector}]", constants, StringComparison.Ordinal);
        Assert.Contains($"i < {BindlessRootSignature.UserConstants}u", constants, StringComparison.Ordinal);
    }

    [DxcFact]
    public void A_table_reaches_a_translated_program_as_a_root_cbv_or_as_root_constants()
    {
        foreach (bool constants in new[] { false, true })
        {
            UserDataConstants.Enabled = constants;

            try
            {
                using var test = TestRuntime.Create();
                var device = new XboxDevice(test);
                var heap = ViewHeap(device, SmallHeap);
                string header = constants ? UserDataConstants.Header : string.Empty;

                Assert.Equal(Marker + 1, Dispatch(test, device, heap, 0, 1, heap.Gpu, program: header + PreludeProgram));
            }
            finally
            {
                UserDataConstants.Enabled = false;
            }
        }
    }

    [DxcFact]
    public void Heap_relative_descriptor_addresses_reach_their_view_and_their_sampler()
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        var heap = ViewHeap(device, SmallHeap);

        var samplers = ViewHeap(device, SmallHeap, type: SamplerHeapType);

        // D3D12_SAMPLER_DESC: point filter, wrap addressing, LOD 0 to 1000.
        uint* point = stackalloc uint[] { 0, 1, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0, BitConverter.SingleToUInt32Bits(1000f) };
        ((delegate* unmanaged<nint, uint*, nint, void>)XboxDevice.Vtable(device.Device)[CreateSamplerSlot])(device.Device, point, samplers.Cpu + (nint)(HeapRelativeSampler * samplers.Increment));

        // Pointers are relative to their heap, as on the console: 32 x a game index is that view's record, and an S#
        // load (SamplerPtrLoad) reads 16 x a sampler index from the sampler records, a sampler table's pointer included.
        Assert.Equal(0u, test.Runtime.D3D12X.DescriptorHeaps.TableAddress(heap.Gpu, 1));
        Assert.Equal(HeapRelativeSampler * DescriptorWindow.SamplerRecordBytes, test.Runtime.D3D12X.DescriptorHeaps.TableAddress(samplers.Gpu + (HeapRelativeSampler * samplers.Increment), 1));
        Assert.Equal(Marker + HeapRelativeSampler, Dispatch(test, device, heap, HeapRelativeRead, HeapRelativeRead + 1, heap.Gpu, program: HeapRelativeProgram));
    }

    [Fact]
    public void The_prelude_reads_sampler_records_where_the_window_holds_them_and_clamps_to_the_desktop_heaps()
    {
        string prelude = RuntimeIncludes.Read("rdna_prelude.hlsli");

        Assert.Contains($"SAMPLER_WINDOW_BASE = 0x{DescriptorWindow.SamplerBase:x8}u;", prelude, StringComparison.Ordinal);
        Assert.Contains($"VIEW_SLOTS = {DescriptorWindow.MaxViews}u;", prelude, StringComparison.Ordinal);
        Assert.Contains($"SAMPLER_SLOTS = {DescriptorWindow.MaxSamplers}u;", prelude, StringComparison.Ordinal);
        Assert.Equal(DescriptorWindow.UnmappedSlot, FiveStar.Shaders.Translation.AddressMap.HeapSlot + 1);

        // The null textures follow the reserved buffers, and the game's views follow them (tex_slot's threshold).
        Assert.Equal(DescriptorWindow.NullTextureBase, FiveStar.Shaders.Translation.NullTextureSlots.First);
        Assert.Equal(DescriptorWindow.ReservedViews, FiveStar.Shaders.Translation.NullTextureSlots.End);
        Assert.Equal($"#define FIVESTAR_FIRST_GAME_VIEW {DescriptorWindow.ReservedViews}u\n", DescriptorWindow.Header);
        Assert.Contains("uint tex_slot(uint i, uint null_slot)", prelude, StringComparison.Ordinal);
    }

    [Fact]
    public unsafe void The_null_texture_descs_name_their_dimension_one_mip_and_one_slice()
    {
        var array = NullTextureViews.ShaderResource("Texture2DArray");
        var volume = NullTextureViews.UnorderedAccess("RWTexture3D");
        int* srv = (int*)&array;
        int* uav = (int*)&volume;

        // SRV: format, TEXTURE2DARRAY, mapping, MostDetailedMip 0, MipLevels 1, FirstArraySlice 0, ArraySize 1.
        Assert.Equal([2, 5, 0x1688, 0, 1, 0, 1], new[] { srv[0], srv[1], srv[2], srv[4], srv[5], srv[6], srv[7] });

        // UAV: format, TEXTURE3D, MipSlice 0, FirstWSlice 0, WSize 1.
        Assert.Equal([2, 8, 0, 0, 1], new[] { uav[0], uav[1], uav[2], uav[3], uav[4] });
    }

    [Fact]
    public void Binding_a_small_table_mirrors_the_records_the_game_copied_past_its_root_signature_range()
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        var heap = ViewHeap(device, ComposedTable);

        // The game copies a constant buffer's V# into record 41 of a table whose root signature spans 7 records, as
        // VS_PassthroughComposite reads it; no slot is called, so only the table bind can mirror it.
        uint[] buffer = [0xf5ba2cc0, 0x00100288, 6, 0x1104dfac];
        buffer.CopyTo(new Span<uint>((void*)(heap.Cpu + (nint)(CompositeRecord * heap.Increment)), buffer.Length));

        Assert.Equal(0u, test.Runtime.D3D12X.DescriptorHeaps.TableAddress(heap.Gpu, DescriptorWindow.MirroredRecords(CompositeExtent)));

        uint* mirror = (uint*)(test.Runtime.D3D12X.Bindings.Window.Data + DescriptorWindow.ViewBase + (CompositeRecord * DescriptorWindow.ViewRecordBytes));
        Assert.Equal(buffer, new Span<uint>(mirror, buffer.Length).ToArray());
    }

    [Fact]
    public void Table_binds_mirror_64_records_unless_told_full_or_a_count()
    {
        Assert.Equal(64u, DescriptorWindow.DefaultTableRecords);
        Assert.Equal(DescriptorWindow.MaxTableRecords, DescriptorWindow.ParseTableRecords("full"));
        Assert.Equal(200u, DescriptorWindow.ParseTableRecords("200"));

        Assert.Null(DescriptorWindow.ParseTableRecords("0"));
        Assert.Null(DescriptorWindow.ParseTableRecords("2048"));
        Assert.Null(DescriptorWindow.ParseTableRecords("many"));
        Assert.Equal(DescriptorWindow.DefaultTableRecords, new FiveStar.Loader.Host.HostOptions().MirrorTableRecords);
    }

    [DxcFact]
    public void A_table_past_the_desktop_limit_reaches_the_program_across_remap_pages()
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        var heap = ViewHeap(device, GameViews);
        uint source = GameViews - DescriptorRemap.PageSize - 1;

        // The table's two descriptors lie on two pages that other writes keep apart on the desktop.
        Assert.Equal(Marker + 1, Dispatch(test, device, heap, source, source + 1, heap.Gpu + ((ulong)source * heap.Increment), FarPage));
    }

    [DxcFact]
    public void A_table_the_game_copies_into_title_memory_itself_reaches_the_program()
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        var heap = ViewHeap(device, GameViews);
        uint copy = GameViews - 1 - FarPage;
        nint table = heap.Cpu + (nint)((ulong)copy * heap.Increment);

        // Views at 0 and 1 through the slots, then the game's own per-draw table: their bytes copied, no slot called.
        Assert.Equal(Marker + 1, Dispatch(test, device, heap, 0, 1, heap.Gpu, copy: table));
    }

    [DxcFact]
    public void A_table_the_game_copies_from_a_cpu_only_heap_itself_reaches_the_program()
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        var heap = ViewHeap(device, GameViews);
        var cpuOnly = ViewHeap(device, SmallHeap, shaderVisible: false);
        nint table = heap.Cpu + (nint)((ulong)FarPage * heap.Increment);

        // The views go into the CPU-only heap; the game copies their bytes into its table in the shader-visible heap.
        Assert.Equal(Marker + 1, Dispatch(test, device, heap, 0, 1, heap.Gpu, copy: table, views: cpuOnly.Cpu));
    }

    [DxcFact]
    public void A_program_reaches_the_frame_the_game_remapped_under_a_placed_buffer()
    {
        using var test = TestRuntime.Create();
        var virtualAlloc = (delegate* unmanaged<nint, nuint, uint, ulong, uint, nint>)test.Shim("xmem.dll", "XMemVirtualAlloc");
        var allocate = (delegate* unmanaged<ulong, ulong*, ulong*, int>)test.Shim("xmem.dll", "XMemAllocatePhysicalPages");
        var map = (delegate* unmanaged<nint, ulong, ulong*, nint>)test.Shim("xmem.dll", "XMemMapPhysicalPages");

        nint cpu = virtualAlloc(0, (nuint)Page, Reserve64K, CpuFlags, CpuReadWrite);
        nint gpu = virtualAlloc(0, (nuint)Page, Reserve64K, GpuFlags, GpuOnly);
        nint other = virtualAlloc(0, (nuint)Page, Reserve64K, CpuFlags, CpuReadWrite);

        ulong* frames = stackalloc ulong[2];
        ulong count = 2;
        Assert.Equal(1, allocate(0, &count, frames));
        Assert.Equal(cpu, map(cpu, 1, &frames[0]));

        // Frame 0 is mapped before the map exists and a buffer is placed over it; the game then maps frame 1 in its
        // place and frame 0 elsewhere. The program must reach frame 1, as the console GPU does.
        var device = new XboxDevice(test);
        var heap = ViewHeap(device, GameViews);
        Assert.Equal(0, device.PlacedBuffer(gpu, Page, out _));

        Assert.Equal(cpu, map(cpu, 1, &frames[1]));
        Assert.Equal(other, map(other, 1, &frames[0]));
        *(ulong*)cpu = (ulong)Marker << 32;
        *(ulong*)other = 0;

        uint loaded = Dispatch(test, device, heap, 0, 1, heap.Gpu, program: AddressMapProgram, words: [unchecked((uint)gpu), (uint)((ulong)gpu >> 32), Stored]);

        Assert.Equal(Marker, loaded);
        Assert.Equal(Stored, *(uint*)cpu);
        Assert.Equal(0UL, *(ulong*)other);
    }

    /// <summary>
    /// Creates an SRV of a marker buffer at game index <paramref name="read"/> and a UAV at <paramref name="write"/>
    /// through the slots, binds the table at <paramref name="table"/> and runs the program; returns what it stored.
    /// </summary>
    private static uint Dispatch(TestRuntime test, XboxDevice device, (nint Heap, nint Cpu, ulong Gpu, uint Increment) heap, uint read, uint write, ulong table, uint between = 0, nint copy = 0, nint views = 0, string program = Program, uint[]? words = null)
    {
        nint at = views == 0 ? heap.Cpu : views;
        nint desktop = test.Runtime.D3D12X.Objects.DesktopDevice;
        nint rootSignature = GameRootSignature(device);
        nint source = Buffer(desktop, new HeapProperties { Type = HeapProperties.Upload }, 0, GenericRead);
        nint destination = Buffer(desktop, new HeapProperties { Type = HeapProperties.Custom, CpuPageProperty = WriteBack, MemoryPool = HeapProperties.SystemPool }, DesktopCalls.UnorderedAccessFlag, 0);

        words ??= [Marker];
        words.CopyTo(new Span<uint>(DesktopCalls.Map(source), words.Length));

        var view = BufferViewDescription.RawShaderResource((uint)words.Length);
        var store = BufferViewDescription.RawUnorderedAccess(1);
        var createRead = (delegate* unmanaged<nint, nint, BufferViewDescription*, nint, void>)XboxDevice.Vtable(device.Device)[CreateShaderResourceViewSlot];
        createRead(device.Device, source, &view, at + (nint)((ulong)read * heap.Increment));

        if (between != 0)
        {
            createRead(device.Device, source, &view, heap.Cpu + (nint)((ulong)between * heap.Increment));
        }

        ((delegate* unmanaged<nint, nint, nint, BufferViewDescription*, nint, void>)XboxDevice.Vtable(device.Device)[CreateUnorderedAccessViewSlot])(device.Device, destination, 0, &store, at + (nint)((ulong)write * heap.Increment));

        if (copy != 0)
        {
            System.Buffer.MemoryCopy((void*)(at + (nint)((ulong)read * heap.Increment)), (void*)copy, heap.Increment * TableSize, heap.Increment * TableSize);
            table = (ulong)copy;
        }

        nint list = device.DirectList();
        nint* heaps = stackalloc nint[] { heap.Heap };
        ((delegate* unmanaged<nint, uint, nint*, void>)XboxDevice.Vtable(list)[ListSetHeapsSlot])(list, 1, heaps);

        ((delegate* unmanaged<nint, nint, void>)XboxDevice.Vtable(list)[ListSetPipelineSlot])(list, Pipeline(desktop, test.Runtime.D3D12X.Bindings.Root, program));
        ((delegate* unmanaged<nint, nint, void>)XboxDevice.Vtable(list)[ListSetComputeRootSlot])(list, rootSignature);
        ((delegate* unmanaged<nint, uint, ulong, void>)XboxDevice.Vtable(list)[ListSetComputeTableSlot])(list, 0, table);
        ((delegate* unmanaged<nint, uint, uint, uint, void>)XboxDevice.Vtable(list)[ListDispatchSlot])(list, 1, 1, 1);
        Run(device, list);

        return *(uint*)DesktopCalls.Map(destination);
    }

    private static (nint Heap, nint Cpu, ulong Gpu, uint Increment) ViewHeap(XboxDevice device, uint count, bool shaderVisible = true, int type = 0)
    {
        int flags = shaderVisible ? 1 : 0;
        int* description = stackalloc int[] { type, (int)count, flags, 0 };
        Guid iid = new("8efb471d-616c-4f49-90f7-127bb763fa51");
        nint heap = 0;
        Assert.Equal(0, ((delegate* unmanaged<nint, int*, Guid*, nint*, int>)XboxDevice.Vtable(device.Device)[CreateDescriptorHeapSlot])(device.Device, description, &iid, &heap));

        uint increment = ((delegate* unmanaged<nint, int, uint>)XboxDevice.Vtable(device.Device)[DescriptorIncrementSlot])(device.Device, type);
        nint cpu = ((delegate* unmanaged<nint, nint>)XboxDevice.Vtable(heap)[HeapCpuStartSlot])(heap);
        ulong gpu = 0;

        if (shaderVisible)
        {
            gpu = (ulong)((delegate* unmanaged<nint, nint>)XboxDevice.Vtable(heap)[HeapGpuStartSlot])(heap);
        }

        return (heap, cpu, gpu, increment);
    }

    /// <summary>The game's kind of root signature: one table of an SRV and a UAV, created through device slot 16.</summary>
    private static nint GameRootSignature(XboxDevice device)
    {
        var blob = Serialize(new RootParameter1(
            new RootDescriptorTable1(new DescriptorRange1(DescriptorRangeType.ShaderResourceView, 1, 0), new DescriptorRange1(DescriptorRangeType.UnorderedAccessView, 1, 0)),
            ShaderVisibility.All));
        Guid iid = new("c54a6b66-72df-4ee8-8be5-a946a1429214");
        nint root = 0;

        fixed (byte* data = blob)
        {
            var create = (delegate* unmanaged<nint, uint, byte*, nuint, Guid*, nint*, int>)XboxDevice.Vtable(device.Device)[CreateRootSignatureSlot];
            Assert.Equal(0, create(device.Device, 0, data, (nuint)blob.Length, &iid, &root));
        }

        return root;
    }

    private static byte[] Serialize(params RootParameter1[] parameters)
    {
        var description = new VersionedRootSignatureDescription(new RootSignatureDescription1(RootSignatureFlags.None, parameters));
        string? error = D3D12.D3D12SerializeVersionedRootSignature(description, out var blob);
        Assert.True(string.IsNullOrEmpty(error), error);

        using (blob)
        {
            return blob!.AsBytes();
        }
    }

    /// <summary>Dwords 1 to 3 of the window record of view-heap descriptor <paramref name="index"/>.</summary>
    private static uint[] Fields(TestRuntime test, int index)
    {
        uint* record = (uint*)(test.Runtime.D3D12X.Bindings.Window.Data + DescriptorWindow.ViewBase + (index * DescriptorWindow.ViewRecordBytes));

        return [record[1], record[2], record[3]];
    }

    private static nint Buffer(nint device, HeapProperties properties, int flags, int state)
    {
        nint buffer = DesktopCalls.CreateBuffer(device, properties, 256, flags, state);
        Assert.NotEqual(0, buffer);

        return buffer;
    }

    /// <summary>A program compiled with dxc (the translator's runtime headers on the include path), as a compute pipeline on the bindless root signature.</summary>
    private static nint Pipeline(nint device, nint root, string program)
    {
        var folder = Directory.CreateTempSubdirectory("fivestar-binding-");

        try
        {
            string source = Path.Combine(folder.FullName, "program.hlsl");
            string output = Path.Combine(folder.FullName, "program.dxil");
            File.WriteAllText(source, program);
            RuntimeIncludes.WriteTo(folder.FullName);

            using var dxc = Process.Start(new ProcessStartInfo(PipelineFactAttribute.Dxc, ["-T", "cs_6_6", "-E", "main", "-I", folder.FullName, "-Fo", output, source]) { RedirectStandardError = true })!;
            string errors = dxc.StandardError.ReadToEnd();
            dxc.WaitForExit();
            Assert.True(dxc.ExitCode == 0, errors);

            var desktop = new ID3D12Device(device);
            desktop.AddRef();

            using (desktop)
            {
                var signature = new ID3D12RootSignature(root);
                signature.AddRef();
                using var state = desktop.CreateComputePipelineState(new ComputePipelineStateDescription { RootSignature = signature, ComputeShader = File.ReadAllBytes(output) });
                state.AddRef();

                return state.NativePointer;
            }
        }
        finally
        {
            folder.Delete(recursive: true);
        }
    }

    /// <summary>Close (list 9), ExecuteCommandListsX (queue 35), Signal (queue 16) and a blocking SetEventOnCompletion (fence 9).</summary>
    private static void Run(XboxDevice device, nint list)
    {
        nint queue = device.Queue();
        nint fence = device.Fence();
        Assert.Equal(0, ((delegate* unmanaged<nint, int>)XboxDevice.Vtable(list)[ListCloseSlot])(list));

        nint* lists = stackalloc nint[] { list };
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, nint*, nint, uint, int>)XboxDevice.Vtable(queue)[QueueExecuteSlot])(queue, 1, lists, 0, 0));
        Assert.Equal(0, ((delegate* unmanaged<nint, nint, ulong, int>)XboxDevice.Vtable(queue)[QueueSignalSlot])(queue, fence, 1));
        Assert.Equal(0, ((delegate* unmanaged<nint, ulong, nint, int>)XboxDevice.Vtable(fence)[FenceWaitSlot])(fence, 1, 0));
    }
}
