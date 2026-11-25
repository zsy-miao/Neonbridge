// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Shims.D3D12X.Resources;
using FiveStar.Shaders.Bvh;

namespace FiveStar.Loader.Shims.Bvh;

/// <summary>
/// BuildRaytracingAccelerationStructureBatched2X on the CPU: a bottom-level desc's geometry is read from game memory
/// through the CPU addresses of its buffers and built with <see cref="XdxrBlas"/>; a top-level desc's instance descs
/// and the headers of their BLASes are read the same way and built with <see cref="XdxrTlas"/>. The structure is
/// written to the destination provided it fits in the size the console reports for the same inputs (the size the game
/// allocated). Compacted-size and current-size requests get the structure size.
/// </summary>
internal sealed unsafe class BvhBatchBuilder
{
    // D3D12XBOX_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_BATCHED_DESC2 as the game fills it (docs/raytracing.md).
    private const int DescSize = 0x50;
    private const int DestOffset = 0x00;
    private const int InputsOffset = 0x08;
    private const int TypeOffset = 0x08;
    private const int CountOffset = 0x10;
    private const int LayoutOffset = 0x14;
    private const int GeometriesOffset = 0x18;
    private const int PostbuildCountOffset = 0x30;
    private const int PostbuildDescsOffset = 0x38;

    /// <summary>D3D12_RAYTRACING_POSTBUILD_INFO_DESC (d3d12.h): DestBuffer, InfoType.</summary>
    private const int PostbuildDescSize = 0x10;
    private const int PostbuildTypeOffset = 0x08;

    /// <summary>D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_TYPE: COMPACTED_SIZE and CURRENT_SIZE.</summary>
    private const uint CompactedSize = 0;
    private const uint CurrentSize = 3;

    /// <summary>D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL and _BOTTOM_LEVEL.</summary>
    private const uint TopLevel = 0;
    private const uint BottomLevel = 1;

    /// <summary>D3D12_ELEMENTS_LAYOUT_ARRAY_OF_POINTERS.</summary>
    private const uint ArrayOfPointers = 1;

    private const int PrebuildFields = 3;
    private const int ResultField = 0;

    private readonly GameMemorySource _memory;
    private readonly ConsoleBvhModule? _console;
    private readonly OnceLog _once;
    private readonly List<BvhTriangle> _triangles = [];
    private readonly List<BvhInstance> _instances = [];
    private readonly object _gate = new();

    /// <param name="memory">Where console GPU addresses are read and written.</param>
    /// <param name="console">The DLL's prebuild query for the size check; null skips the check.</param>
    /// <param name="once">Logs each kind of skipped build once.</param>
    public BvhBatchBuilder(GameMemorySource memory, ConsoleBvhModule? console, OnceLog once)
    {
        _memory = memory;
        _console = console;
        _once = once;
    }

    /// <summary>Builds <paramref name="count"/> descs; returns how many structures were written.</summary>
    public int Build(uint count, byte* descs)
    {
        int written = 0;

        lock (_gate)
        {
            for (uint i = 0; i < count; i++)
            {
                if (BuildOne(descs + ((nint)i * DescSize)))
                {
                    written++;
                }
            }
        }

        return written;
    }

    private bool BuildOne(byte* desc)
    {
        uint type = *(uint*)(desc + TypeOffset);

        if (type == TopLevel)
        {
            return BuildTopLevel(desc);
        }

        if (type != BottomLevel)
        {
            return Skip("unknown acceleration-structure type");
        }

        if (!TryReadTriangles(desc))
        {
            return Skip("geometry unreadable or in an unsupported format");
        }

        return Store(desc, XdxrBlas.Build(_triangles));
    }

    private bool BuildTopLevel(byte* desc)
    {
        _instances.Clear();
        uint count = *(uint*)(desc + CountOffset);
        bool pointers = *(uint*)(desc + LayoutOffset) == ArrayOfPointers;
        ulong address = *(ulong*)(desc + GeometriesOffset);

        if (count > 0 && !InstanceDescs.TryRead(address, count, pointers, _memory, _instances))
        {
            return Skip("instance descs unreadable");
        }

        return Store(desc, XdxrTlas.Build(_instances));
    }

    private bool Store(byte* desc, byte[] structure)
    {
        if ((ulong)structure.Length > AllocatedSize(desc))
        {
            return Skip("structure larger than the console size");
        }

        byte* dest = _memory.Find(*(ulong*)(desc + DestOffset), (ulong)structure.Length);

        if (dest == null)
        {
            return Skip("destination not mapped");
        }

        structure.CopyTo(new Span<byte>(dest, structure.Length));
        WritePostbuild(desc, (ulong)structure.Length);

        return true;
    }

    private bool TryReadTriangles(byte* desc)
    {
        _triangles.Clear();
        uint count = *(uint*)(desc + CountOffset);
        bool pointers = *(uint*)(desc + LayoutOffset) == ArrayOfPointers;
        byte* array = *(byte**)(desc + GeometriesOffset);

        for (uint i = 0; i < count; i++)
        {
            byte* geometry = pointers ? ((byte**)array)[i] : array + ((nint)i * TriangleGeometry.Size);

            if (!TriangleGeometry.TryAppend(geometry, i, _memory, _triangles))
            {
                return false;
            }
        }

        return true;
    }

    /// <summary>The console result size for the desc's inputs, or unbounded without the DLL.</summary>
    private ulong AllocatedSize(byte* desc)
    {
        if (_console is null)
        {
            return ulong.MaxValue;
        }

        ulong* info = stackalloc ulong[PrebuildFields];
        _console.PrebuildInfo(desc + InputsOffset, null, info);

        return info[ResultField];
    }

    private void WritePostbuild(byte* desc, ulong size)
    {
        uint count = *(uint*)(desc + PostbuildCountOffset);
        byte* infos = *(byte**)(desc + PostbuildDescsOffset);

        for (uint i = 0; i < count && infos != null; i++)
        {
            byte* info = infos + ((nint)i * PostbuildDescSize);
            uint type = *(uint*)(info + PostbuildTypeOffset);

            if (type != CompactedSize && type != CurrentSize)
            {
                continue;
            }

            ulong* target = (ulong*)_memory.Find(*(ulong*)info, sizeof(ulong));

            if (target != null)
            {
                *target = size;
            }
        }
    }

    private bool Skip(string reason)
    {
        _once.Warn(reason, $"[bvh] build skipped: {reason}");

        return false;
    }
}
