// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Calls;

/// <summary>Calls into XboxBvhBuilder.dll, each checked against its disassembly first.</summary>
internal static unsafe class BvhRecipes
{
    public const string Module = "xboxbvhbuilder.dll";
    public const string Set64BitMortonCodes = "?Set64BitMortonCodes@XboxBvhBuilder@@QEAAX_N@Z";
    public const string PrebuildInfo = "?GetRaytracingAccelerationStructurePrebuildInfo@XboxBvhBuilder@@QEAAXPEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS@@PEBUD3D12XBOX_XDXR_STREAM_DESC@@PEAUD3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO@@@Z";
    public const string ValidateRehydrate = "?ValidateRehydrateAccelerationStructureBuildDesc@XboxBvhBuilder@@QEAAJPEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC@@@Z";

    /// <summary>The global Set64BitMortonCodes writes (from the disassembly: <c>mov [rip+global],dl; ret</c>).</summary>
    private const uint MortonFlagRva = 0xD7B4C;

    /// <summary>A stand-in "this": the member functions called here do not read it.</summary>
    private const int FakeThisSize = 64;

    // D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC (d3d12.h): 0x38 bytes.
    private const int DescSize = 0x38;
    private const int DestOffset = 0x00;
    private const int InputsTypeOffset = 0x08;
    private const int InputsNumDescsOffset = 0x10;
    private const int SourceOffset = 0x20;

    /// <summary>D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL.</summary>
    private const uint BottomLevel = 1;

    // D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS (0x18) and D3D12_RAYTRACING_GEOMETRY_DESC (0x38), d3d12.h.
    private const int InputsSize = 0x18;
    private const int InputsCountOffset = 0x08;
    private const int InputsGeometriesOffset = 0x10;
    private const int GeometrySize = 0x38;
    private const int VertexFormatOffset = 0x14;
    private const int VertexCountOffset = 0x1C;
    private const int VertexStrideOffset = 0x30;

    /// <summary>DXGI_FORMAT_R32G32B32_FLOAT, one f32 xyz per vertex.</summary>
    private const uint PositionFormat = 6;
    private const uint PositionStride = 12;
    private const uint TriangleVertices = 3;

    /// <summary>The prebuild info's three sizes; the result size is rounded up to 1 KiB by the DLL.</summary>
    private const int PrebuildFields = 3;
    private const int ResultField = 0;
    private const int ScratchField = 1;
    private const int UpdateField = 2;
    private const ulong ResultAlignment = 1024;

    private const int DestSize = 0x10000;
    private const int SourceSize = 0x1000;

    /// <summary>The source header word the validator tests, and the CRC32 slot (CRC of NumDescs = 0 with seed 0 is 0).</summary>
    private const uint SourceHeaderFlag = 0x01000000;
    private const int SourceCrcOffset = 8;
    private const uint WrongCrc = 0xDEADBEEF;

    /// <summary>Set64BitMortonCodes(bool) ignores "this" and toggles the global.</summary>
    public static ExportCallResult ToggleMortonCodes(ExportCallContext call)
    {
        byte* flag = (byte*)call.Image.Va(MortonFlagRva);
        byte* fakeThis = stackalloc byte[FakeThisSize];
        var set = (delegate* unmanaged<nint, byte, void>)call.Function;
        byte initial = *flag;

        // Set, then clear.
        set((nint)fakeThis, 1);
        byte on = *flag;
        set((nint)fakeThis, 0);
        byte off = *flag;

        return new ExportCallResult($"global flag (+0x{MortonFlagRva:x}): initial={initial}, after (true)={on}, after (false)={off}", on == 1 && off == 0);
    }

    /// <summary>
    /// The prebuild size query for a one-triangle BLAS with a zeroed stand-in object: CPU arithmetic over the inputs,
    /// the object is passed nowhere. Expects a nonzero result size rounded to 1 KiB and a nonzero scratch size.
    /// </summary>
    public static ExportCallResult ConsoleSizes(ExportCallContext call)
    {
        var query = (delegate* unmanaged<nint, byte*, void*, ulong*, void>)call.Function;
        byte* self = stackalloc byte[FakeThisSize];
        byte* inputs = stackalloc byte[InputsSize];
        byte* geometry = stackalloc byte[GeometrySize];
        ulong* info = stackalloc ulong[PrebuildFields];
        new Span<byte>(self, FakeThisSize).Clear();

        new Span<byte>(inputs, InputsSize).Clear();
        new Span<byte>(geometry, GeometrySize).Clear();
        *(uint*)inputs = BottomLevel;
        *(uint*)(inputs + InputsCountOffset) = 1;
        *(ulong*)(inputs + InputsGeometriesOffset) = (ulong)geometry;

        *(uint*)(geometry + VertexFormatOffset) = PositionFormat;
        *(uint*)(geometry + VertexCountOffset) = TriangleVertices;
        *(ulong*)(geometry + VertexStrideOffset) = PositionStride;
        query((nint)self, inputs, null, info);
        bool expected = info[ResultField] != 0 && info[ResultField] % ResultAlignment == 0 && info[ScratchField] != 0;

        return new ExportCallResult($"one-triangle BLAS: result {info[ResultField]}, scratch {info[ScratchField]}, update {info[UpdateField]}", expected);
    }

    /// <summary>
    /// CPU-only validation: XMemVirtualQuery on the destination and source, then the CRC32 of NumDescs against the
    /// source header; S_OK for a matching CRC, E_INVALIDARG otherwise.
    /// </summary>
    public static ExportCallResult ValidateRehydrateDesc(ExportCallContext call)
    {
        var validate = (delegate* unmanaged<nint, nint, int>)call.Function;
        nint dest = Kernel32.VirtualAlloc(0, DestSize, Win32Constants.MemReserveCommit, Win32Constants.PageReadWrite);
        nint source = Kernel32.VirtualAlloc(0, SourceSize, Win32Constants.MemReserveCommit, Win32Constants.PageReadWrite);
        byte* self = stackalloc byte[FakeThisSize];
        byte* desc = stackalloc byte[DescSize];
        new Span<byte>(self, FakeThisSize).Clear();

        new Span<byte>(desc, DescSize).Clear();
        *(ulong*)(desc + DestOffset) = (ulong)dest;
        *(uint*)(desc + InputsTypeOffset) = BottomLevel;
        *(uint*)(desc + InputsNumDescsOffset) = 0;
        *(ulong*)(desc + SourceOffset) = (ulong)source;
        *(uint*)source = SourceHeaderFlag;

        *(uint*)(source + SourceCrcOffset) = 0;
        int good = validate((nint)self, (nint)desc);
        *(uint*)(source + SourceCrcOffset) = WrongCrc;
        int bad = validate((nint)self, (nint)desc);
        _ = Kernel32.VirtualFree(dest, 0, Win32Constants.MemRelease);
        _ = Kernel32.VirtualFree(source, 0, Win32Constants.MemRelease);

        string summary = $"matching CRC -> 0x{good:x8} (expect 0 = S_OK), wrong CRC -> 0x{bad:x8} (expect 0x80070057 = E_INVALIDARG)";

        return new ExportCallResult(summary, good == HResult.Ok && bad == HResult.InvalidArgument);
    }
}
