// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.Ffx;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// The console FidelityFX loader's exports without a PC FidelityFX DLL: the game ignores the ffxCreateContext result
/// and divides its frame counter by the queried jitter phase count, so the shim must fail creation cleanly and still
/// answer the jitter queries.
/// </summary>
public sealed unsafe class FidelityFxShimTests
{
    private const ulong CreateUpscaleType = 0x00010000;
    private const uint RenderWidth = 1280;
    private const uint DisplayWidth = 1920;

    [Fact]
    public void Without_a_pc_dll_creation_reports_no_provider_and_jitter_is_still_answered()
    {
        using var test = TestRuntime.Create();
        var create = (delegate* unmanaged<nint*, byte*, nint, uint>)test.Shim(FfxLayout.Module, "ffxCreateContext");
        var query = (delegate* unmanaged<nint*, byte*, uint>)test.Shim(FfxLayout.Module, "ffxQuery");
        var dispatch = (delegate* unmanaged<nint*, byte*, uint>)test.Shim(FfxLayout.Module, "ffxDispatch");

        nint context = -1;
        ulong* createDesc = stackalloc ulong[6];
        createDesc[0] = CreateUpscaleType;

        Assert.Equal(FfxLayout.ReturnNoProvider, create(&context, (byte*)createDesc, 0));
        Assert.Equal(0, context);

        int phaseCount = 0;
        byte* phaseDesc = stackalloc byte[0x20];
        *(ulong*)phaseDesc = FfxLayout.QueryJitterPhaseCountType;
        *(uint*)(phaseDesc + FfxLayout.PayloadOffset) = RenderWidth;
        *(uint*)(phaseDesc + FfxLayout.JitterDisplayWidthOffset) = DisplayWidth;
        *(int**)(phaseDesc + FfxLayout.JitterPhaseCountOutOffset) = &phaseCount;

        Assert.Equal(FfxLayout.ReturnOk, query(&context, phaseDesc));
        Assert.Equal(18, phaseCount);

        float x = float.NaN;
        float y = float.NaN;
        byte* offsetDesc = stackalloc byte[0x28];

        *(ulong*)offsetDesc = FfxLayout.QueryJitterOffsetType;
        *(int*)(offsetDesc + FfxLayout.JitterPhaseCountOffset) = phaseCount;
        *(float**)(offsetDesc + FfxLayout.JitterOffsetXOutOffset) = &x;
        *(float**)(offsetDesc + FfxLayout.JitterOffsetYOutOffset) = &y;

        Assert.Equal(FfxLayout.ReturnOk, query(&context, offsetDesc));
        Assert.Equal(0.0f, x);
        Assert.Equal((1.0f / 3.0f) - 0.5f, y, 1e-6f);

        ulong* dispatchDesc = stackalloc ulong[0x40];
        dispatchDesc[0] = FfxLayout.DispatchUpscaleType;

        Assert.Equal(FfxLayout.ReturnParameter, dispatch(&context, (byte*)dispatchDesc));
    }

    [Fact]
    public void A_missing_pc_dll_leaves_fidelityfx_off()
    {
        using var test = TestRuntime.Create();

        Assert.False(test.Runtime.FidelityFx.Load(Path.Combine(test.Folder, "amd_fidelityfx_loader_dx12.dll")));
    }
}
