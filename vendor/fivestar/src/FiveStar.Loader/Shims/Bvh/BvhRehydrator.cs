// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Shims.D3D12X.Resources;

namespace FiveStar.Loader.Shims.Bvh;

/// <summary>
/// RehydrateRaytracingAccelerationStructureBatched2X on the CPU: each serialized BLAS goes through the shipped DLL's
/// own CPU rehydration (<see cref="ConsoleBvhModule.Rehydrate"/>) on a copy of its build desc whose console GPU
/// addresses (destination, transform, index and vertex buffers) are replaced by the CPU addresses of the same memory.
/// The result is the console's AMD-layout BLAS in game memory, where the translated shaders read it. Every range the
/// rehydration touches is checked readable first; a desc that fails is skipped and reported once.
/// </summary>
internal sealed unsafe class BvhRehydrator
{
    // D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC (d3d12.h): Dest, Inputs (Type, Flags, NumDescs,
    // DescsLayout, pGeometryDescs), Source, Scratch.
    private const int DescSize = 0x30;
    private const int DestOffset = 0x00;
    private const int TypeOffset = 0x08;
    private const int CountOffset = 0x10;
    private const int LayoutOffset = 0x14;
    private const int GeometriesOffset = 0x18;
    private const int SourceOffset = 0x20;

    /// <summary>D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL.</summary>
    private const uint BottomLevel = 1;

    /// <summary>D3D12_ELEMENTS_LAYOUT_ARRAY_OF_POINTERS.</summary>
    private const uint ArrayOfPointers = 1;

    private readonly ConsoleBvhModule _module;
    private readonly GameMemorySource _memory;
    private readonly OnceLog _once;
    private readonly object _gate = new();
    private byte* _desc;
    private byte* _geometries;
    private uint _geometryCapacity;

    public BvhRehydrator(ConsoleBvhModule module, GameMemorySource memory, OnceLog once)
    {
        _module = module;
        _memory = memory;
        _once = once;
    }

    /// <summary>Rehydrates <paramref name="count"/> descs; <paramref name="sources"/> optionally holds their CPU source addresses.</summary>
    public int Rehydrate(uint count, byte* descs, ulong* sources)
    {
        int result = HResult.Ok;

        lock (_gate)
        {
            for (uint i = 0; i < count; i++)
            {
                byte* desc = descs + ((nint)i * DescSize);
                ulong source = sources != null ? sources[i] : *(ulong*)(desc + SourceOffset);
                int one = RehydrateOne(desc, source);

                if (HResult.Failed(one) && result == HResult.Ok)
                {
                    result = one;
                }
            }
        }

        return result;
    }

    private int RehydrateOne(byte* desc, ulong sourceAddress)
    {
        byte* header = _memory.Find(sourceAddress, SerializedBvhHeader.Size);

        if (header == null || !SerializedBvhHeader.IsSerialized(header) || *(uint*)(desc + TypeOffset) != BottomLevel)
        {
            return Skip("not a readable serialized BLAS", HResult.InvalidArgument);
        }

        ulong size = SerializedBvhHeader.TotalSize(header);
        byte* source = _memory.Find(sourceAddress, size);
        byte* dest = _memory.Find(*(ulong*)(desc + DestOffset), size);

        if (source == null || dest == null)
        {
            return Skip("source or destination not mapped", HResult.Ok);
        }

        byte* geometries = TranslateGeometries(desc);

        if (geometries == null)
        {
            return Skip("geometry buffers not mapped", HResult.Ok);
        }

        PrepareDesc(desc, dest, source, geometries);

        return _module.Rehydrate(_desc);
    }

    private void PrepareDesc(byte* desc, byte* dest, byte* source, byte* geometries)
    {
        if (_desc == null)
        {
            _desc = (byte*)NativeMemory.AllocZeroed(DescSize);
        }

        new ReadOnlySpan<byte>(desc, DescSize).CopyTo(new Span<byte>(_desc, DescSize));
        *(ulong*)(_desc + DestOffset) = (ulong)dest;
        *(uint*)(_desc + LayoutOffset) = 0;
        *(ulong*)(_desc + GeometriesOffset) = (ulong)geometries;
        *(ulong*)(_desc + SourceOffset) = (ulong)source;
    }

    /// <summary>The desc's geometry array with CPU addresses, as one contiguous array; null when a buffer is unreadable.</summary>
    private byte* TranslateGeometries(byte* desc)
    {
        uint count = *(uint*)(desc + CountOffset);
        bool pointers = *(uint*)(desc + LayoutOffset) == ArrayOfPointers;
        byte* array = *(byte**)(desc + GeometriesOffset);
        Reserve(count);

        for (uint i = 0; i < count; i++)
        {
            byte* geometry = pointers ? ((byte**)array)[i] : array + ((nint)i * TriangleGeometry.Size);
            byte* copy = _geometries + ((nint)i * TriangleGeometry.Size);

            if (!TriangleGeometry.TryTranslate(geometry, copy, _memory))
            {
                return null;
            }
        }

        return _geometries;
    }

    private void Reserve(uint count)
    {
        if (count <= _geometryCapacity)
        {
            return;
        }

        _geometries = (byte*)NativeMemory.Realloc(_geometries, (nuint)count * TriangleGeometry.Size);
        _geometryCapacity = count;
    }

    private int Skip(string reason, int result)
    {
        _once.Warn(reason, $"[bvh] rehydration skipped: {reason}");

        return result;
    }
}
