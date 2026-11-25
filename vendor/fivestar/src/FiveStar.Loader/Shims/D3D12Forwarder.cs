// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims;

/// <summary>
/// Forwards the d3d12_xs.dll root-signature functions to the PC d3d12.dll, whose C signatures are the same. This is
/// where struct translation goes once D3D12.X-only root-signature flags or desc versions show up.
/// </summary>
internal sealed unsafe class D3D12Forwarder
{
    private const string PcLibrary = "d3d12.dll";

    /// <summary>
    /// D3D12_VERSIONED_ROOT_SIGNATURE_DESC: the version, then a 1.0, 1.1 or 1.2 desc whose Flags sit at the same place
    /// in all three; the desktop flags end at SAMPLER_HEAP_DIRECTLY_INDEXED (0x800) (d3d12.h).
    /// </summary>
    private const int VersionedDescSize = 0x30;
    private const int FlagsOffset = 0x28;
    private const uint DesktopRootFlags = 0xFFF;

    private readonly object _gate = new();
    private readonly LoaderLog _log;
    private nint _library;

    public D3D12Forwarder(LoaderLog log)
    {
        _log = log;
    }

    /// <summary>HRESULT D3D12SerializeVersionedRootSignature(const D3D12_VERSIONED_ROOT_SIGNATURE_DESC*, ID3DBlob**, ID3DBlob**).</summary>
    public int SerializeVersionedRootSignature(nint desc, nint* blob, nint* error)
    {
        uint version = 0;

        if (desc != 0)
        {
            version = *(uint*)desc;
        }

        var target = (delegate* unmanaged<nint, nint*, nint*, int>)Export("D3D12SerializeVersionedRootSignature");
        nint ownError = 0;
        byte* desktop = stackalloc byte[VersionedDescSize];
        nint desktopDesc = 0;
        uint flags = 0;

        if (desc != 0)
        {
            // The D3D12.X flags above the desktop ones (0x3000000 from XboxBvhBuilder.dll) are dropped.
            Buffer.MemoryCopy((void*)desc, desktop, VersionedDescSize, VersionedDescSize);
            flags = *(uint*)(desktop + FlagsOffset);
            *(uint*)(desktop + FlagsOffset) = flags & DesktopRootFlags;
            desktopDesc = (nint)desktop;
        }

        nint* errorOut = error;

        if (errorOut == null)
        {
            errorOut = &ownError;
        }

        int hr = target(desktopDesc, blob, errorOut);
        _log.Detail($"  [d3d12_xs] D3D12SerializeVersionedRootSignature(version=0x{version:x}, flags 0x{flags:x}) -> 0x{hr:x8} (forwarded to d3d12.dll)");

        if (HResult.Failed(hr))
        {
            _log.Warn($"[d3d12_xs] root signature rejected: {BlobText(*errorOut)}");
        }

        ComObject.Release(ownError);

        return hr;
    }

    /// <summary>The text in an ID3DBlob (GetBufferPointer slot 3, GetBufferSize slot 4), or empty.</summary>
    private static string BlobText(nint blob)
    {
        const int BufferPointerSlot = 3;
        const int BufferSizeSlot = 4;

        if (blob == 0)
        {
            return string.Empty;
        }

        nint text = ((delegate* unmanaged<nint, nint>)ComObject.Slot(blob, BufferPointerSlot))(blob);
        nuint size = ((delegate* unmanaged<nint, nuint>)ComObject.Slot(blob, BufferSizeSlot))(blob);

        return Marshal.PtrToStringAnsi(text, (int)size).TrimEnd('\0', '\n');
    }

    /// <summary>HRESULT D3D12SerializeRootSignature(const D3D12_ROOT_SIGNATURE_DESC*, D3D_ROOT_SIGNATURE_VERSION, ID3DBlob**, ID3DBlob**).</summary>
    public int SerializeRootSignature(nint desc, uint version, nint* blob, nint* error)
    {
        var target = (delegate* unmanaged<nint, uint, nint*, nint*, int>)Export("D3D12SerializeRootSignature");
        int hr = target(desc, version, blob, error);
        _log.Detail($"  [d3d12_xs] D3D12SerializeRootSignature(version=0x{version:x}) -> 0x{hr:x8} (forwarded)");

        return hr;
    }

    /// <summary>HRESULT D3D12Create[Versioned]RootSignatureDeserializer(const void* data, SIZE_T size, REFIID, void** out).</summary>
    public int CreateDeserializer(string function, nint data, nuint size, Guid* iid, nint* result)
    {
        var target = (delegate* unmanaged<nint, nuint, Guid*, nint*, int>)Export(function);
        int hr = target(data, size, iid, result);
        _log.Detail($"  [d3d12_xs] {function}(size=0x{size:x}, iid={*iid}) -> 0x{hr:x8} (forwarded)");

        return hr;
    }

    /// <summary>HRESULT D3D12CreateDevice(IUnknown* adapter, D3D_FEATURE_LEVEL, REFIID, void**) on the default adapter.</summary>
    public int CreateDevice(int minimumFeatureLevel, Guid* iid, nint* device)
    {
        var target = (delegate* unmanaged<nint, int, Guid*, nint*, int>)Export("D3D12CreateDevice");
        int hr = target(0, minimumFeatureLevel, iid, device);
        _log.Detail($"  [d3d12_xs] D3D12CreateDevice(level=0x{minimumFeatureLevel:x}) -> 0x{hr:x8}");

        return hr;
    }

    /// <summary>HRESULT D3D12GetDebugInterface(REFIID, void**): the debug layer and DRED settings objects.</summary>
    public int GetDebugInterface(Guid* iid, nint* result)
    {
        var target = (delegate* unmanaged<Guid*, nint*, int>)Export("D3D12GetDebugInterface");

        return target(iid, result);
    }

    private nint Export(string name)
    {
        lock (_gate)
        {
            if (_library == 0)
            {
                _library = NativeLibrary.Load(PcLibrary);
            }

            return NativeLibrary.GetExport(_library, name);
        }
    }
}
