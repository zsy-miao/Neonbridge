// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Bvh;

/// <summary>
/// XboxBvhBuilder.dll, the console's ray-tracing acceleration-structure builder: a C++ class whose constructor makes
/// compute pipelines from console shader code on the D3D12.X device, which the desktop cannot run (its first pipeline,
/// device slot 62, fails and the constructor throws). The exports bind to <see cref="BvhBuilderService"/> instead.
/// Member functions take the object in rcx; the constructor returns it. The rehydration's last argument is an
/// optional array of CPU addresses of the serialized sources (the game passes it; the descs hold GPU addresses).
/// </summary>
internal static unsafe partial class BvhBuilderShim
{
    public const string Module = "XboxBvhBuilder.dll";

    [Shim(Module, "??0XboxBvhBuilder@@QEAA@PEAUID3D12Device11@@_N@Z")]
    [UnmanagedCallersOnly]
    private static nint Construct(nint self, nint device, byte morton)
    {
        try
        {
            var service = ShimContext.Runtime.BvhBuilder;

            if (service.GpuInventory)
            {
                service.Inventory(self, device, morton);

                return self;
            }

            service.Note("XboxBvhBuilder(ID3D12Device11*, bool)");

            return self;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "BvhBuilderShim.Construct");

            return default;
        }
    }

    [Shim(Module, "??1XboxBvhBuilder@@QEAA@XZ")]
    [UnmanagedCallersOnly]
    private static void Destruct(nint self)
    {
    }

    [Shim(Module, "?BuildRaytracingAccelerationStructureBatched2X@XboxBvhBuilder@@QEAAXPEAUID3D12GraphicsCommandList6@@IPEBUD3D12XBOX_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_BATCHED_DESC2@@@Z")]
    [UnmanagedCallersOnly]
    private static void BuildBatched(nint self, nint list, uint count, void* descs)
    {
        try
        {
            ShimContext.Runtime.BvhBuilder.Build(count, descs);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "BvhBuilderShim.BuildBatched");
        }
    }

    [Shim(Module, "?CopyRaytracingAccelerationStructure@XboxBvhBuilder@@QEAAXPEAUID3D12GraphicsCommandList4@@_K1W4D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE@@@Z")]
    [UnmanagedCallersOnly]
    private static void Copy(nint self, nint list, ulong destination, ulong source, int mode)
    {
        try
        {
            ShimContext.Runtime.BvhBuilder.Copy(destination, source, mode);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "BvhBuilderShim.Copy");
        }
    }

    [Shim(Module, "?GetRaytracingAccelerationStructurePrebuildInfo@XboxBvhBuilder@@QEAAXPEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS@@PEBUD3D12XBOX_XDXR_STREAM_DESC@@PEAUD3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO@@@Z")]
    [UnmanagedCallersOnly]
    private static void PrebuildInfo(nint self, void* inputs, void* stream, ulong* info)
    {
        try
        {
            ShimContext.Runtime.BvhBuilder.PrebuildInfo(inputs, stream, info);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "BvhBuilderShim.PrebuildInfo");
        }
    }

    [Shim(Module, "?RehydrateRaytracingAccelerationStructureBatched2X@XboxBvhBuilder@@QEAAJPEAUID3D12GraphicsCommandList8@@IPEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC@@PEB_K@Z")]
    [UnmanagedCallersOnly]
    private static int Rehydrate(nint self, nint list, uint count, void* descs, ulong* sources)
    {
        try
        {
            return ShimContext.Runtime.BvhBuilder.Rehydrate(count, descs, sources);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "BvhBuilderShim.Rehydrate");

            return HResult.Fail;
        }
    }

    [Shim(Module, "?Set64BitMortonCodes@XboxBvhBuilder@@QEAAX_N@Z")]
    [UnmanagedCallersOnly]
    private static void SetMortonCodes(nint self, byte enable)
    {
    }
}
