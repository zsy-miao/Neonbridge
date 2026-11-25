// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Shims.D3D12X.Resources;

namespace FiveStar.Loader.Shims.Bvh;

/// <summary>
/// What stands behind <see cref="BvhBuilderShim"/>. By default nothing is built and sizes come from the desktop
/// device's prebuild query. With ray tracing on (<c>--raytracing</c>) the shipped DLL's CPU entry points take over:
/// console sizes, so the game lays out AMD-format structures as on the console, and CPU rehydration of the serialized
/// BLASes the game streams in, CPU builds of bottom-level structures and compacting copies (see docs/raytracing.md).
/// </summary>
internal sealed unsafe class BvhBuilderService
{
    /// <summary>ID3D12Device5::GetRaytracingAccelerationStructurePrebuildInfo (d3d12.h).</summary>
    private const int PrebuildInfoSlot = 63;

    /// <summary>D3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO: result, scratch and update-scratch sizes.</summary>
    private const int PrebuildInfoFields = 3;

    /// <summary>D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_COMPACT, the only mode the DLL acts on.</summary>
    private const int CompactMode = 1;

    private static readonly Guid Device5Iid = new("8b4f173b-2fea-4b80-8f58-4307191ab95d");

    private readonly XboxObjects _objects;
    private readonly ConsoleBvhModule _console;
    private readonly BvhRehydrator _rehydrator;
    private readonly BvhBatchBuilder _builder;
    private readonly GameMemorySource _memory;
    private readonly OnceLog _once;
    private readonly LoaderLog _log;
    private nint _device5;

    public BvhBuilderService(XboxObjects objects, ImageLoader loader, XMemPhysicalMemory memory, LoaderLog log)
    {
        _objects = objects;
        _log = log;
        _once = new OnceLog(log);
        _console = new ConsoleBvhModule(loader, log);

        _memory = new GameMemorySource(memory);
        _rehydrator = new BvhRehydrator(_console, _memory, _once);
        _builder = new BvhBatchBuilder(_memory, _console, _once);
    }

    /// <summary>Whether the console DLL's CPU paths are used (<c>--raytracing</c>); off by default.</summary>
    public bool Enabled { get; set; }

    /// <summary><c>--raytracing-gpu</c>: the shipped constructor runs for an inventory of its pipelines (<see cref="BvhBuilderInventory"/>).</summary>
    public bool GpuInventory { get; set; }

    /// <summary>
    /// With <see cref="GpuInventory"/>: runs the shipped constructor on the game's object, its pipelines answered by
    /// stand-ins never built, and logs them by their CS container's offset in the module and size.
    /// </summary>
    public void Inventory(nint self, nint device, byte morton)
    {
        bool ran = false;
        var pipelines = BvhBuilderInventory.During(() => ran = _console.Construct(self, device, morton));

        if (!ran)
        {
            _log.Warn("[bvh] --raytracing-gpu: the shipped constructor is missing; no inventory");

            return;
        }

        _log.Info($"[bvh] --raytracing-gpu: the shipped constructor ran and asked for {pipelines.Count} compute pipelines (stand-ins, never built; no builder program runs)");

        foreach (var (bytecode, length, root) in pipelines)
        {
            _log.Info($"[bvh]   CS container at module +0x{bytecode - _console.ImageBase:x}, 0x{length:x} bytes, root signature 0x{root:x}");
        }
    }

    public void Note(string call)
    {
        _once.Warn(call, $"[bvh] XboxBvhBuilder::{call}: stubbed, no acceleration structure written");
    }

    /// <summary>Console sizes with ray tracing on; else the desktop prebuild sizes, or zeros without an ID3D12Device5.</summary>
    public void PrebuildInfo(void* inputs, void* stream, ulong* info)
    {
        if (Enabled && inputs != null && _console.TryBind())
        {
            _console.PrebuildInfo(inputs, stream, info);

            return;
        }

        DesktopPrebuildInfo(inputs, info);
    }

    /// <summary>Rehydrates serialized BLASes on the CPU with ray tracing on; else skips them.</summary>
    public int Rehydrate(uint count, void* descs, ulong* sources)
    {
        if (!Enabled || !_console.TryBind())
        {
            Note("RehydrateRaytracingAccelerationStructureBatched2X (no rehydration)");

            return HResult.Ok;
        }

        int result = _rehydrator.Rehydrate(count, (byte*)descs, sources);
        _log.Detail($"[bvh] rehydration of {count} descs: 0x{result:X8}");

        return result;
    }

    /// <summary>Builds bottom-level structures on the CPU with ray tracing on; else skips the batch.</summary>
    public void Build(uint count, void* descs)
    {
        if (!Enabled || descs == null || !_console.TryBind())
        {
            Note("BuildRaytracingAccelerationStructureBatched2X (no build)");

            return;
        }

        int built = _builder.Build(count, (byte*)descs);
        _log.Detail($"[bvh] batched build: {built} of {count} structures written");
    }

    /// <summary>A compacting copy of a built or rehydrated structure with ray tracing on: its bytes, unchanged (node pointers are relative).</summary>
    public void Copy(ulong destination, ulong source, int mode)
    {
        if (!Enabled || mode != CompactMode)
        {
            Note("CopyRaytracingAccelerationStructure (no copy)");

            return;
        }

        byte* header = _memory.Find(source, SerializedBvhHeader.Size);

        if (header == null || SerializedBvhHeader.IsSerialized(header))
        {
            Note("CopyRaytracingAccelerationStructure (source unreadable or serialized)");

            return;
        }

        ulong size = SerializedBvhHeader.TotalSize(header);
        byte* from = _memory.Find(source, size);
        byte* to = _memory.Find(destination, size);

        if (from == null || to == null)
        {
            Note("CopyRaytracingAccelerationStructure (range not mapped)");

            return;
        }

        Buffer.MemoryCopy(from, to, size, size);
    }

    private void DesktopPrebuildInfo(void* inputs, ulong* info)
    {
        for (int i = 0; i < PrebuildInfoFields; i++)
        {
            info[i] = 0;
        }

        nint device = Device5();

        if (device == 0 || inputs == null)
        {
            Note("GetRaytracingAccelerationStructurePrebuildInfo (no ID3D12Device5: zero sizes)");

            return;
        }

        ((delegate* unmanaged<nint, void*, ulong*, void>)ComObject.Slot(device, PrebuildInfoSlot))(device, inputs, info);
    }

    private nint Device5()
    {
        if (_device5 != 0)
        {
            return _device5;
        }

        nint desktop = _objects.DesktopDevice;

        if (desktop == 0)
        {
            return 0;
        }

        Guid iid = Device5Iid;
        nint device5 = 0;

        if (HResult.Failed(((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(desktop, ComObject.QueryInterfaceSlot))(desktop, &iid, &device5)))
        {
            return 0;
        }

        // The device lives for the process; the reference is kept.
        _device5 = device5;

        return device5;
    }
}
