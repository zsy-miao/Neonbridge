// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Shims.Ffx;

/// <summary>
/// A PC FidelityFX API DLL the user supplies (the DX12 loader of the AMD FidelityFX SDK). The five entry points have
/// the same C signatures as the console's.
/// </summary>
internal sealed unsafe class PcFidelityFxLibrary
{
    private readonly delegate* unmanaged<nint*, byte*, nint, uint> _create;
    private readonly delegate* unmanaged<nint*, nint, uint> _destroy;
    private readonly delegate* unmanaged<nint*, byte*, uint> _configure;
    private readonly delegate* unmanaged<nint*, byte*, uint> _query;
    private readonly delegate* unmanaged<nint*, byte*, uint> _dispatch;

    private PcFidelityFxLibrary(string path, nint create, nint destroy, nint configure, nint query, nint dispatch)
    {
        Path = path;
        _create = (delegate* unmanaged<nint*, byte*, nint, uint>)create;
        _destroy = (delegate* unmanaged<nint*, nint, uint>)destroy;
        _configure = (delegate* unmanaged<nint*, byte*, uint>)configure;
        _query = (delegate* unmanaged<nint*, byte*, uint>)query;
        _dispatch = (delegate* unmanaged<nint*, byte*, uint>)dispatch;
    }

    public string Path { get; }

    /// <summary>Loads the DLL and its five exports; null (logged) when the file, the load or an export fails.</summary>
    public static PcFidelityFxLibrary? TryLoad(string path, LoaderLog log)
    {
        if (!File.Exists(path))
        {
            log.Warn($"  [ffx] {path} not found: FidelityFX stays off");

            return null;
        }

        if (!NativeLibrary.TryLoad(path, out nint handle))
        {
            log.Warn($"  [ffx] {path} did not load: FidelityFX stays off");

            return null;
        }

        bool found = NativeLibrary.TryGetExport(handle, "ffxCreateContext", out nint create)
            & NativeLibrary.TryGetExport(handle, "ffxDestroyContext", out nint destroy)
            & NativeLibrary.TryGetExport(handle, "ffxConfigure", out nint configure)
            & NativeLibrary.TryGetExport(handle, "ffxQuery", out nint query)
            & NativeLibrary.TryGetExport(handle, "ffxDispatch", out nint dispatch);

        if (!found)
        {
            log.Warn($"  [ffx] {path} lacks the ffxCreateContext/DestroyContext/Configure/Query/Dispatch exports: FidelityFX stays off");
            NativeLibrary.Free(handle);

            return null;
        }

        log.Info($"  [ffx] FidelityFX API from {path}");

        return new PcFidelityFxLibrary(path, create, destroy, configure, query, dispatch);
    }

    public uint Create(nint* context, byte* desc, nint allocator)
    {
        return _create(context, desc, allocator);
    }

    public uint Destroy(nint* context, nint allocator)
    {
        return _destroy(context, allocator);
    }

    public uint Configure(nint* context, byte* desc)
    {
        return _configure(context, desc);
    }

    public uint Query(nint* context, byte* desc)
    {
        return _query(context, desc);
    }

    [HotPath]
    public uint Dispatch(nint* context, byte* desc)
    {
        return _dispatch(context, desc);
    }
}
