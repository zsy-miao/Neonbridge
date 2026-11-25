// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>A spare allocator lent while the game's allocator records elsewhere goes back once it has been idle.</summary>
public sealed unsafe class SpareAllocatorsTests
{
    /// <summary>ID3D12Device CreateCommandAllocator and CreateCommandList; ID3D12GraphicsCommandList Close and Reset.</summary>
    private const int CreateAllocatorSlot = 9;
    private const int CreateListSlot = 12;
    private const int CloseSlot = 9;
    private const int ResetSlot = 10;
    private const int DirectType = 0;

    private static readonly Guid AllocatorIid = new("6102dee4-af59-4b09-b999-b44d73f09b24");
    private static readonly Guid ListIid = new("5b160d0f-ac1b-4185-8ba8-b3ae42a5a455");

    [Fact]
    public void A_spare_not_lent_for_its_idle_resets_is_released()
    {
        using var test = TestRuntime.Create();
        _ = new XboxDevice(test);
        nint device = test.Runtime.D3D12X.Objects.DesktopDevice;

        nint game = Allocator(device);
        nint recording = List(device, game);
        nint other = List(device, Allocator(device));
        Close(other);

        // The game's allocator backs a list still recording: the desktop refuses it, so a spare is lent.
        var spares = new SpareAllocators();
        var reset = (delegate* unmanaged<nint, nint, nint, int>)ComObject.Slot(other, ResetSlot);
        Assert.Equal(HResult.Ok, spares.Reset(other, game, 0, reset));
        Close(other);
        Close(recording);

        for (long i = 0; i < SpareAllocators.IdleResets - 1; i++)
        {
            spares.ResetSpares(game);
        }

        Assert.Equal(1, spares.Held(game));
        spares.ResetSpares(game);

        Assert.Equal(0, spares.Held(game));
        Assert.Equal(1, spares.Released);
    }

    private static nint Allocator(nint device)
    {
        Guid iid = AllocatorIid;
        nint allocator = 0;
        int hr = ((delegate* unmanaged<nint, int, Guid*, nint*, int>)ComObject.Slot(device, CreateAllocatorSlot))(device, DirectType, &iid, &allocator);
        Assert.Equal(HResult.Ok, hr);

        return allocator;
    }

    private static nint List(nint device, nint allocator)
    {
        Guid iid = ListIid;
        nint list = 0;
        int hr = ((delegate* unmanaged<nint, uint, int, nint, nint, Guid*, nint*, int>)ComObject.Slot(device, CreateListSlot))(device, 0, DirectType, allocator, 0, &iid, &list);
        Assert.Equal(HResult.Ok, hr);

        return list;
    }

    private static void Close(nint list)
    {
        Assert.Equal(HResult.Ok, ((delegate* unmanaged<nint, int>)ComObject.Slot(list, CloseSlot))(list));
    }
}
