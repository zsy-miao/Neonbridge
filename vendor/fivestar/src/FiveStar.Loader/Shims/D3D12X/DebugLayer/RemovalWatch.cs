// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// Notices device removal when it happens rather than at the game's next failing call: a removed device sets every
/// fence to UINT64_MAX, so an event waiting for that value on a private fence fires.
/// </summary>
internal sealed unsafe class RemovalWatch : IDisposable
{
    /// <summary>ID3D12Device::CreateFence and ID3D12Fence::SetEventOnCompletion (d3d12.h).</summary>
    private const int CreateFenceSlot = 36;
    private const int SetEventOnCompletionSlot = 9;
    private const int NoFenceFlags = 0;

    private static readonly Guid FenceIid = new("0a753dcf-c4d8-4b91-adf6-be5a60d95a76");

    private readonly EventWaitHandle _removed = new(false, EventResetMode.ManualReset);
    private nint _fence;

    /// <summary>Calls <paramref name="onRemoved"/> on a pool thread once the device is removed; the HRESULT of the set-up.</summary>
    public int Start(nint device, Action onRemoved)
    {
        Guid iid = FenceIid;
        nint fence = 0;
        int hr = ((delegate* unmanaged<nint, ulong, int, Guid*, nint*, int>)ComObject.Slot(device, CreateFenceSlot))(device, 0, NoFenceFlags, &iid, &fence);

        if (HResult.Failed(hr))
        {
            return hr;
        }

        _fence = fence;
        nint handle = _removed.SafeWaitHandle.DangerousGetHandle();
        hr = ((delegate* unmanaged<nint, ulong, nint, int>)ComObject.Slot(fence, SetEventOnCompletionSlot))(fence, ulong.MaxValue, handle);

        if (!HResult.Failed(hr))
        {
            ThreadPool.RegisterWaitForSingleObject(_removed, (_, _) => onRemoved(), null, Timeout.Infinite, true);
        }

        return hr;
    }

    /// <summary>Closes the event; the fence stays with the device.</summary>
    public void Dispose()
    {
        _removed.Dispose();
    }
}
