// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Shaders.Translation;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// The GPU-memory window of the translated programs (heap slots 0 and 1, docs/shader-translation.md, Resources) as far
/// as descriptor tables need it. A program reads a table through a 32-bit pointer in a user SGPR: descriptor k of the
/// table at pointer + k * 32 (T# size; buffers and samplers use the first 16 bytes), and dword 0 of that descriptor is
/// the desktop heap index. The window holds one 32-byte record per descriptor of the game's view heap from 0, in game
/// order, then a null table and one 16-byte record per sampler of the game's sampler heap; <see cref="ShaderVisibleHeap"/>
/// keeps dword 0 of each record equal to the desktop index the game's descriptor stands for. A 32-bit descriptor
/// address is relative to its heap on the console: the game passes heap-relative addresses of its own in root constants
/// (MS_VS_Transform_PositionOnly reads its V# at root constant 2, which the game sets to 0, + index * 32), so the view
/// records start at window offset 0 and such an address reaches the record of its game index. (VIctory, a GPL-3.0 port
/// of the same game, gives tables their byte offset in the heap and the heap pointer 0 the same way.)
/// </summary>
internal sealed unsafe class DescriptorWindow
{
    /// <summary>D3D12_MAX_SHADER_VISIBLE_DESCRIPTOR_HEAP_SIZE_TIER_2 and D3D12_MAX_SHADER_VISIBLE_SAMPLER_HEAP_SIZE.</summary>
    public const uint MaxViews = 1000000;
    public const uint MaxSamplers = 2048;

    /// <summary>Bytes of a view record (a T#) and a sampler record (an S#) in the window.</summary>
    public const uint ViewRecordBytes = 32;
    public const uint SamplerRecordBytes = 16;

    /// <summary>Where a view record's mirror holds the desktop index it names, past the V# (<see cref="ShaderVisibleHeap"/>).</summary>
    public const int MirroredDesktopDword = 4;

    /// <summary>
    /// Records mirrored for a table of unbounded or unknown size: the game's per-draw tables place descriptors at
    /// byte offsets below 0x8000 (the 0x7fe0 mask at 0x144d9f04f), 1024 records.
    /// </summary>
    public const uint MaxTableRecords = 1024;

    /// <summary>
    /// Records mirrored for any table, whatever its root-signature ranges say: the game places a table's descriptors
    /// at the offsets of its own binding entries (0x144d9efc0) by copying their bytes, and its programs read them there
    /// (default_ps_tex reads its T# at record 1 of a table whose root signature spans one descriptor;
    /// VS_PassthroughComposite reads records 28 and 41 of a table that spans 7, so 16 left the composite's constants
    /// stale and it drew a sliver; TL_PS_PBR_MATERIAL_STANDARD reads record 180 of its table 0).
    /// <c>--mirror-table-records</c> sets it: <see cref="DefaultTableRecords"/> by default, the game's whole range
    /// (<see cref="MaxTableRecords"/>) with <c>full</c>, which lets the lighting read its inputs but also lets mesh
    /// programs reach the real data they hang on (w44, w47), until each program's reach is known.
    /// </summary>
    public static uint MinTableRecords { get; set; } = DefaultTableRecords;

    /// <summary>Records every table bind mirrors by default: VS_PassthroughComposite's record 41 included.</summary>
    public const uint DefaultTableRecords = 64;

    /// <summary>The records a table bind mirrors: its root signature's extent, at least <see cref="MinTableRecords"/>.</summary>
    public static uint MirroredRecords(uint extent)
    {
        return Math.Max(extent, MinTableRecords);
    }

    /// <summary>
    /// <c>--mirror-table-records</c>: <c>full</c> for <see cref="MaxTableRecords"/>, else a count from 1 to it; null for
    /// anything else.
    /// </summary>
    public static uint? ParseTableRecords(string text)
    {
        if (text.Equals("full", StringComparison.OrdinalIgnoreCase))
        {
            return MaxTableRecords;
        }

        if (!uint.TryParse(text, System.Globalization.NumberStyles.None, System.Globalization.CultureInfo.InvariantCulture, out uint count) || count == 0 || count > MaxTableRecords)
        {
            return null;
        }

        return count;
    }

    /// <summary>Heap slots the programs reach by constant: the window (rdna_prelude GPU_MEM_HEAP), its writable view, the mesh fallback table.</summary>
    public const uint WindowSlot = 0;
    public const uint WritableWindowSlot = 1;
    public const uint FallbackTableSlot = 2;

    /// <summary>The fallback table's SRV, a null descriptor nothing rewrites: what a record naming no descriptor reads.</summary>
    public const uint NullViewSlot = FallbackTableSlot + FallbackUavs;

    /// <summary>Where views go that no desktop descriptor is left for.</summary>
    public const uint SinkSlot = FallbackTableSlot + FallbackTableSize;

    /// <summary>rdna_prelude GPU_MAP_HEAP: the address map of game memory (<see cref="GameMemoryViews"/>).</summary>
    public const uint AddressMapSlot = FiveStar.Shaders.Translation.AddressMap.HeapSlot;

    /// <summary>The null SRV and UAV (next slot) that unmapped pages of the address map name.</summary>
    public const uint UnmappedSlot = AddressMapSlot + 1;

    /// <summary>The null texture descriptors of each dimension (<see cref="NullTextureSlots"/>), after the null pair.</summary>
    public const uint NullTextureBase = UnmappedSlot + 2;

    /// <summary>The first desktop index of the game's descriptors: past the null textures (rdna_prelude FIRST_GAME_VIEW).</summary>
    public const uint ReservedViews = NullTextureSlots.End;

    /// <summary>The define of the first game view for programs made on demand (rdna_prelude tex_slot): T#s below it read a null texture.</summary>
    public static string Header => $"#define FIVESTAR_FIRST_GAME_VIEW {ReservedViews}u\n";

    /// <summary>rdna_prelude STATIC_SAMPLER_BASE: samplers for the literal S#s of the programs (<see cref="StaticSamplerHeap"/>).</summary>
    public const uint StaticSamplerBase = FiveStar.Shaders.Translation.StaticSampler.FirstSlot;

    /// <summary>
    /// Window layout: view records from 0 (heap-relative addresses, see the class), a null table (for a table handle
    /// outside the heaps), sampler records.
    /// </summary>
    public const uint ViewBase = 0;
    public const uint NullTableBase = ViewBase + (MaxWindowViews * ViewRecordBytes);
    public const uint SamplerBase = NullTableBase + (MaxTableRecords * ViewRecordBytes);

    /// <summary>
    /// Game view records the window has room for: the game's main view heap holds 1,245,184 descriptors, more than the
    /// desktop's 1,000,000, so the regions past the records (<see cref="WindowGuard"/>, the debug rings) start after
    /// this many. They started after <see cref="MaxViews"/> records before, under the records of game indices
    /// 1,000,000 and up: the shim's mirror wrote those records into the store-trace ring, the window flags and the mesh
    /// probe's ring (w24's ring count of 2,180,802,880).
    /// </summary>
    public const uint MaxWindowViews = 1u << 21;

    /// <summary>The end of the records: the regions past them (<see cref="WindowGuard"/>, the debug rings) start here.</summary>
    public const uint RecordsEnd = SamplerBase + (MaxSamplers * SamplerRecordBytes);

    /// <summary>The mesh fallback table: u0..u2 then t0 (BindlessRootSignature).</summary>
    private const uint FallbackTableSize = 4;
    private const uint FallbackUavs = 3;
    private const int CommonState = 0;

    private readonly LoaderLog _log;
    private ulong _viewGpuStart;
    private uint _viewIncrement;

    public DescriptorWindow(LoaderLog log)
    {
        _log = log;
        StaticSamplers = new StaticSamplerHeap(log);
    }

    /// <summary>The samplers of the programs' literal S#s, at their slots from <see cref="StaticSamplerBase"/>.</summary>
    public StaticSamplerHeap StaticSamplers { get; }

    /// <summary>The desktop shader-visible view heap, once the game created its heap.</summary>
    public nint ViewHeap { get; private set; }

    /// <summary>The desktop shader-visible sampler heap, once the game created its heap.</summary>
    public nint SamplerHeap { get; private set; }

    /// <summary>The GPU handle of the mesh fallback table (root parameter 1 of every translated program).</summary>
    public ulong FallbackTable => _viewGpuStart + ((ulong)FallbackTableSlot * _viewIncrement);

    /// <summary>The window buffer; 0 until the view heap exists.</summary>
    public nint Buffer { get; private set; }

    /// <summary>The window's CPU mapping; null until the view heap exists.</summary>
    public byte* Data { get; private set; }

    /// <summary>The window flags of <see cref="WindowFeedback"/>, indexed by window id; null without it or a window.</summary>
    public uint* WindowFlags
    {
        get
        {
            if (!WindowFeedback.Enabled || Data == null)
            {
                return null;
            }

            return (uint*)(Data + WindowFeedback.Base);
        }
    }

    /// <summary>
    /// Sets up the window for the game's shader-visible view heap of <paramref name="views"/> descriptors: the window
    /// buffer (CPU-writable, GPU-readable and writable), its two views, the null descriptors and the records.
    /// </summary>
    public bool AttachViews(nint device, nint heap, uint views, uint increment)
    {
        if (views > MaxWindowViews)
        {
            _log.Warn($"[d3d12_xs] descriptor window: a view heap of {views} descriptors; records past {MaxWindowViews} are not mirrored");
        }

        uint bytes = RecordsEnd;

        // The window guard's counters past the room for records (rdna_prelude FIVESTAR_WINDOW_GUARD).
        if (WindowGuard.Enabled && bytes < WindowGuard.Bytes)
        {
            bytes = WindowGuard.Bytes;
        }

        // --trace-pool-stores: the store ring past the most view records the window can hold.
        if (DebugLayer.PoolStoreTrace.Enabled && bytes < DebugLayer.PoolStoreTrace.Bytes)
        {
            bytes = DebugLayer.PoolStoreTrace.Bytes;
        }

        // --evict-windows: the flags of the windows programs ask for, past the store-trace ring.
        if (WindowFeedback.Enabled && bytes < WindowFeedback.Bytes)
        {
            bytes = WindowFeedback.Bytes;
        }

        // --probe-pipeline: the mesh probe's ring, past the flags.
        if (DebugLayer.MeshProbes.Enabled && bytes < DebugLayer.MeshProbes.Bytes)
        {
            bytes = DebugLayer.MeshProbes.Bytes;
        }

        var properties = new HeapProperties { Type = HeapProperties.Custom, CpuPageProperty = HeapProperties.WriteCombine, MemoryPool = HeapProperties.SystemPool };
        nint buffer = DesktopCalls.CreateBuffer(device, properties, bytes, DesktopCalls.UnorderedAccessFlag, CommonState);
        byte* data = buffer == 0 ? null : DesktopCalls.Map(buffer);

        if (data == null)
        {
            _log.Warn("[d3d12_xs] descriptor window: no CPU-writable buffer, descriptor tables read nothing");

            return false;
        }

        FillRecords(data, bytes);
        WriteReserved(device, buffer, bytes, DesktopCalls.HeapCpuStart(heap), increment);

        Buffer = buffer;
        Data = data;
        ViewHeap = heap;
        _viewGpuStart = DesktopCalls.HeapGpuStart(heap);
        _viewIncrement = increment;
        _log.Detail($"  [d3d12_xs] descriptor window: 0x{bytes:x} bytes, view records from 0x{ViewBase:x}, game views from desktop index {ReservedViews}");

        return true;
    }

    /// <summary>
    /// Takes the game's shader-visible sampler heap: the static sampler slots get a trilinear stand-in, then the samplers
    /// the program folders name (<see cref="StaticSamplers"/>).
    /// </summary>
    public void AttachSamplers(nint device, nint heap, uint count, uint increment)
    {
        uint* sampler = stackalloc uint[TrilinearSampler.Dwords];
        TrilinearSampler.Write(sampler);
        nint cpu = DesktopCalls.HeapCpuStart(heap);

        for (uint slot = StaticSamplerBase; slot < count; slot++)
        {
            DesktopCalls.Sampler(device, sampler, cpu + (nint)(slot * increment));
        }

        SamplerHeap = heap;
        StaticSamplers.Attach(device, cpu, increment);
    }

    /// <summary>Window records of the game's view heap: game order from <see cref="ViewBase"/>.</summary>
    public static HeapRecords ViewRecords(uint desktopCount)
    {
        return new HeapRecords(ViewBase, ViewRecordBytes, ReservedViews, desktopCount, NullViewSlot);
    }

    /// <summary>Window records of the game's sampler heap: the game's indices are the desktop ones.</summary>
    public static HeapRecords SamplerRecords(uint desktopCount)
    {
        return new HeapRecords(SamplerBase, SamplerRecordBytes, 0, Math.Min(desktopCount, MaxSamplers), 0);
    }

    /// <summary>Null view records everywhere (the null table included), sampler records naming their own index.</summary>
    private static void FillRecords(byte* data, uint bytes)
    {
        new Span<byte>(data, (int)bytes).Clear();

        for (uint offset = ViewBase; offset < SamplerBase; offset += ViewRecordBytes)
        {
            *(uint*)(data + offset) = NullViewSlot;
        }

        for (uint sampler = 0; sampler < MaxSamplers; sampler++)
        {
            *(uint*)(data + SamplerBase + (sampler * SamplerRecordBytes)) = sampler;
        }
    }

    private static void WriteReserved(nint device, nint buffer, uint bytes, nint cpu, uint increment)
    {
        var read = BufferViewDescription.RawShaderResource(bytes / sizeof(uint));
        var write = BufferViewDescription.RawUnorderedAccess(bytes / sizeof(uint));
        DesktopCalls.ShaderResourceView(device, buffer, &read, cpu + (nint)(WindowSlot * increment));
        DesktopCalls.UnorderedAccessView(device, buffer, &write, cpu + (nint)(WritableWindowSlot * increment));

        // Null descriptors: the fallback table the root signature asks for, the sink, the address map until it exists
        // and the pair unmapped pages name.
        var nullWrite = BufferViewDescription.RawUnorderedAccess(0);
        var nullRead = BufferViewDescription.RawShaderResource(0);

        for (uint slot = FallbackTableSlot; slot < FallbackTableSlot + FallbackUavs; slot++)
        {
            DesktopCalls.UnorderedAccessView(device, 0, &nullWrite, cpu + (nint)(slot * increment));
        }

        for (uint slot = NullViewSlot; slot <= UnmappedSlot; slot++)
        {
            DesktopCalls.ShaderResourceView(device, 0, &nullRead, cpu + (nint)(slot * increment));
        }

        DesktopCalls.UnorderedAccessView(device, 0, &nullWrite, cpu + (nint)((UnmappedSlot + 1) * increment));
        WriteNullTextures(device, cpu, increment);
    }

    /// <summary>A null SRV of each texture dimension, then a null UAV of each writable one (<see cref="NullTextureSlots"/>).</summary>
    private static void WriteNullTextures(nint device, nint cpu, uint increment)
    {
        foreach (string type in NullTextureSlots.Views)
        {
            var view = NullTextureViews.ShaderResource(type);
            DesktopCalls.ShaderResourceView(device, 0, &view, cpu + (nint)(NullTextureSlots.For(type) * increment));
        }

        foreach (string type in NullTextureSlots.Uavs)
        {
            var view = NullTextureViews.UnorderedAccess(type);
            DesktopCalls.UnorderedAccessView(device, 0, &view, cpu + (nint)(NullTextureSlots.For(type) * increment));
        }
    }
}
