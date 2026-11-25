// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XUserFindControllerForUserWithUiAsync: no picker; the result is the APP_LOCAL_DEVICE_ID of the first gamepad.</summary>
internal sealed class ControllerPairOperation : ManagedAsyncOperation
{
    public override AsyncWorkResult Work()
    {
        return new AsyncWorkResult(HResult.Ok, UserService.DeviceIdSize);
    }

    public override unsafe int Write(byte* buffer, nuint size)
    {
        UserService.ControllerId.CopyTo(new Span<byte>(buffer, UserService.DeviceIdSize));

        return HResult.Ok;
    }
}
