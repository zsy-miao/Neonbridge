// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Shims.Ffx;

/// <summary>
/// The console FidelityFX loader's entry points on a PC (docs/middleware.md). With a PC FidelityFX DLL configured,
/// calls are forwarded with the D3D12.X device, command list and resources swapped in place for the desktop objects
/// and put back afterwards. Without one, ffxCreateContext reports no provider, the jitter queries are answered here
/// (the game divides by the phase count unchecked) and dispatches are refused.
/// </summary>
internal sealed unsafe class FidelityFxBridge
{
    private readonly IDesktopObjectResolver _objects;
    private readonly LoaderLog _log;
    private PcFidelityFxLibrary? _library;
    private int _refusedDispatches;

    public FidelityFxBridge(IDesktopObjectResolver objects, LoaderLog log)
    {
        _objects = objects;
        _log = log;
    }

    /// <summary>Loads the PC DLL at <paramref name="path"/>; false (logged) leaves FidelityFX off.</summary>
    public bool Load(string path)
    {
        _library = PcFidelityFxLibrary.TryLoad(path, _log);

        return _library is not null;
    }

    /// <summary>ffxReturnCode_t ffxCreateContext(ffxContext* context, ffxCreateContextDescHeader* desc, const ffxAllocationCallbacks* memCb).</summary>
    public uint CreateContext(nint* context, byte* desc, nint allocator)
    {
        if (context == null || desc == null)
        {
            return FfxLayout.ReturnParameter;
        }

        *context = 0;
        var library = _library;

        if (library is null)
        {
            _log.Info($"  [ffx] ffxCreateContext(type 0x{Type(desc):x}) -> no provider: no PC FidelityFX DLL (--ffx)");

            return FfxLayout.ReturnNoProvider;
        }

        byte* backend = FindInChain(desc, FfxLayout.XboxBackendDescType);
        nint xboxDevice = 0;

        if (backend != null)
        {
            xboxDevice = Field(backend, FfxLayout.PayloadOffset);
            *(ulong*)backend = FfxLayout.PcBackendDescType;
            Field(backend, FfxLayout.PayloadOffset) = _objects.Device(xboxDevice);
        }

        uint result;

        try
        {
            result = library.Create(context, desc, allocator);
        }
        finally
        {
            if (backend != null)
            {
                *(ulong*)backend = FfxLayout.XboxBackendDescType;
                Field(backend, FfxLayout.PayloadOffset) = xboxDevice;
            }
        }

        _log.Info($"  [ffx] ffxCreateContext(type 0x{Type(desc):x}, device 0x{xboxDevice:x}) -> {result} (forwarded to {library.Path})");

        return result;
    }

    /// <summary>ffxReturnCode_t ffxDestroyContext(ffxContext* context, const ffxAllocationCallbacks* memCb).</summary>
    public uint DestroyContext(nint* context, nint allocator)
    {
        var library = _library;

        if (library is null || context == null || *context == 0)
        {
            return FfxLayout.ReturnParameter;
        }

        return library.Destroy(context, allocator);
    }

    /// <summary>ffxReturnCode_t ffxConfigure(ffxContext* context, const ffxConfigureDescHeader* desc).</summary>
    public uint Configure(nint* context, byte* desc)
    {
        var library = _library;

        if (desc == null)
        {
            return FfxLayout.ReturnParameter;
        }

        if (library is null || (context != null && *context == 0))
        {
            return FfxLayout.ReturnNoProvider;
        }

        return library.Configure(context, desc);
    }

    /// <summary>ffxReturnCode_t ffxQuery(ffxContext* context, ffxQueryDescHeader* desc).</summary>
    public uint Query(nint* context, byte* desc)
    {
        if (desc == null)
        {
            return FfxLayout.ReturnParameter;
        }

        var library = _library;

        if (library is not null && (context == null || *context != 0))
        {
            uint result = library.Query(context, desc);

            if (result == FfxLayout.ReturnOk)
            {
                return result;
            }
        }

        return AnswerQuery(desc);
    }

    /// <summary>ffxReturnCode_t ffxDispatch(ffxContext* context, const ffxDispatchDescHeader* desc): the upscale dispatch only.</summary>
    [HotPath]
    public uint Dispatch(nint* context, byte* desc)
    {
        var library = _library;

        if (library is null || context == null || desc == null || *context == 0)
        {
            NoteRefusedDispatch();

            return FfxLayout.ReturnParameter;
        }

        if (Type(desc) != FfxLayout.DispatchUpscaleType)
        {
            return FfxLayout.ReturnUnknownDescType;
        }

        Span<nint> saved = stackalloc nint[FfxLayout.DispatchResourceCount + 1];
        SwapToDesktop(desc, saved);

        try
        {
            return library.Dispatch(context, desc);
        }
        finally
        {
            Restore(desc, saved);
        }
    }

    /// <summary>The queries the game makes, answered without an upscaler: jitter phase count and offset, zero memory.</summary>
    internal static uint AnswerQuery(byte* desc)
    {
        switch (Type(desc))
        {
            case FfxLayout.QueryJitterPhaseCountType:
                return AnswerPhaseCount(desc);
            case FfxLayout.QueryJitterOffsetType:
                return AnswerJitterOffset(desc);
            case FfxLayout.QueryGpuMemoryUsageType:
                return AnswerMemoryUsage(desc);
            default:
                return FfxLayout.ReturnNoProvider;
        }
    }

    private static uint AnswerPhaseCount(byte* desc)
    {
        uint renderWidth = *(uint*)(desc + FfxLayout.PayloadOffset);
        uint displayWidth = *(uint*)(desc + FfxLayout.JitterDisplayWidthOffset);
        int* output = (int*)Field(desc, FfxLayout.JitterPhaseCountOutOffset);

        if (output == null)
        {
            return FfxLayout.ReturnParameter;
        }

        *output = FsrJitter.PhaseCount(renderWidth, displayWidth);

        return FfxLayout.ReturnOk;
    }

    private static uint AnswerJitterOffset(byte* desc)
    {
        int index = *(int*)(desc + FfxLayout.PayloadOffset);
        int phaseCount = *(int*)(desc + FfxLayout.JitterPhaseCountOffset);
        float* x = (float*)Field(desc, FfxLayout.JitterOffsetXOutOffset);
        float* y = (float*)Field(desc, FfxLayout.JitterOffsetYOutOffset);

        if (x == null || y == null)
        {
            return FfxLayout.ReturnParameter;
        }

        (*x, *y) = FsrJitter.Offset(index, phaseCount);

        return FfxLayout.ReturnOk;
    }

    private static uint AnswerMemoryUsage(byte* desc)
    {
        byte* output = (byte*)Field(desc, FfxLayout.PayloadOffset);

        if (output == null)
        {
            return FfxLayout.ReturnParameter;
        }

        new Span<byte>(output, FfxLayout.MemoryUsageSize).Clear();

        return FfxLayout.ReturnOk;
    }

    private static ulong Type(byte* desc)
    {
        return *(ulong*)desc;
    }

    private static ref nint Field(byte* desc, int offset)
    {
        return ref *(nint*)(desc + offset);
    }

    private static byte* FindInChain(byte* desc, ulong type)
    {
        byte* node = desc;

        for (int i = 0; i < FfxLayout.MaxChainLength && node != null; i++)
        {
            if (Type(node) == type)
            {
                return node;
            }

            node = (byte*)Field(node, FfxLayout.NextOffset);
        }

        return null;
    }

    [HotPath]
    private void SwapToDesktop(byte* desc, Span<nint> saved)
    {
        saved[0] = Field(desc, FfxLayout.DispatchCommandListOffset);
        Field(desc, FfxLayout.DispatchCommandListOffset) = _objects.CommandList(saved[0]);

        for (int i = 0; i < FfxLayout.DispatchResourceCount; i++)
        {
            int offset = FfxLayout.DispatchFirstResourceOffset + (i * FfxLayout.ResourceStride);
            saved[i + 1] = Field(desc, offset);
            Field(desc, offset) = _objects.Resource(saved[i + 1]);
        }
    }

    [HotPath]
    private static void Restore(byte* desc, ReadOnlySpan<nint> saved)
    {
        Field(desc, FfxLayout.DispatchCommandListOffset) = saved[0];

        for (int i = 0; i < FfxLayout.DispatchResourceCount; i++)
        {
            Field(desc, FfxLayout.DispatchFirstResourceOffset + (i * FfxLayout.ResourceStride)) = saved[i + 1];
        }
    }

    private void NoteRefusedDispatch()
    {
        if (Interlocked.Increment(ref _refusedDispatches) == 1)
        {
            _log.Warn("  [ffx] ffxDispatch without an upscaler context: refused, the upscaled output is not written (logged once)");
        }
    }
}
