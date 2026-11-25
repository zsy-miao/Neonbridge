// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;

namespace FiveStar.Loader.Shims.Bvh;

/// <summary>
/// The shipped XboxBvhBuilder.dll, mapped on first use for its two CPU-only entry points: the prebuild size query and
/// the CPU rehydration of a serialized BLAS. Neither reads the builder object (the query passes it nowhere, the
/// rehydration export drops it and the device), and the globals they read are set only by Set64BitMortonCodes, so
/// both run without the constructor, which needs console compute shaders.
/// </summary>
internal sealed unsafe class ConsoleBvhModule
{
    private const string FileName = "xboxbvhbuilder.dll";
    private const string PrebuildInfoExport = "?GetRaytracingAccelerationStructurePrebuildInfo@XboxBvhBuilder@@QEAAXPEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS@@PEBUD3D12XBOX_XDXR_STREAM_DESC@@PEAUD3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO@@@Z";
    private const string ConstructorExport = "??0XboxBvhBuilder@@QEAA@PEAUID3D12Device11@@_N@Z";
    private const string RehydrateExport = "?RehydrateRaytracingAccelerationStructureX@XboxBvhBuilder@@QEAAJPEAUID3D12Device11@@PEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC@@@Z";

    /// <summary>A stand-in object: neither entry point reads it.</summary>
    private const int FakeObjectSize = 64;

    private readonly ImageLoader _loader;
    private readonly LoaderLog _log;
    private readonly object _gate = new();
    private bool _tried;
    private nint _prebuildInfo;
    private nint _rehydrate;
    private nint _fakeObject;
    private nint _constructor;
    private nint _imageBase;

    public ConsoleBvhModule(ImageLoader loader, LoaderLog log)
    {
        _loader = loader;
        _log = log;
    }

    /// <summary>Maps the module once; false when it is missing or lacks the exports.</summary>
    public bool TryBind()
    {
        lock (_gate)
        {
            if (!_tried)
            {
                _tried = true;
                Bind();
            }

            return _rehydrate != 0 && _prebuildInfo != 0;
        }
    }

    /// <summary>The module's base address once mapped; 0 before.</summary>
    public nint ImageBase => _imageBase;

    /// <summary>The shipped constructor on <paramref name="self"/> (the game's object) with our device; false without it.</summary>
    public bool Construct(nint self, nint device, byte morton)
    {
        if (!TryBind() || _constructor == 0)
        {
            return false;
        }

        ((delegate* unmanaged<nint, nint, byte, nint>)_constructor)(self, device, morton);

        return true;
    }

    /// <summary>XboxBvhBuilder::GetRaytracingAccelerationStructurePrebuildInfo: the console's sizes.</summary>
    public void PrebuildInfo(void* inputs, void* stream, ulong* info)
    {
        ((delegate* unmanaged<nint, void*, void*, ulong*, void>)_prebuildInfo)(_fakeObject, inputs, stream, info);
    }

    /// <summary>XboxBvhBuilder::RehydrateRaytracingAccelerationStructureX on CPU addresses: S_OK or E_INVALIDARG.</summary>
    public int Rehydrate(void* description)
    {
        return ((delegate* unmanaged<nint, nint, void*, int>)_rehydrate)(_fakeObject, 0, description);
    }

    private void Bind()
    {
        var image = _loader.FindMapped(FileName);

        if (image is null)
        {
            string? path = _loader.FindModuleFile(FileName);

            if (path is null)
            {
                _log.Warn($"[bvh] {FileName} not found in the search directories: no rehydration, desktop sizes");

                return;
            }

            image = _loader.Load(path, false);
        }

        var prebuild = image.Exports.Find(PrebuildInfoExport, 0);
        var rehydrate = image.Exports.Find(RehydrateExport, 0);

        if (prebuild is null || rehydrate is null)
        {
            _log.Warn($"[bvh] {FileName} lacks the prebuild or rehydration export");

            return;
        }

        // Lives for the process, like the module.
        _fakeObject = (nint)NativeMemory.AllocZeroed(FakeObjectSize);
        _constructor = image.Exports.Find(ConstructorExport, 0)?.Address ?? 0;
        _imageBase = image.Base;
        _prebuildInfo = prebuild.Address;
        _rehydrate = rehydrate.Address;
        _log.Info($"[bvh] {FileName}: console prebuild sizes and CPU rehydration");
    }
}
