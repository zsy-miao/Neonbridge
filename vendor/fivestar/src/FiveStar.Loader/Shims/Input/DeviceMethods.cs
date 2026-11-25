// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Input;

/// <summary>The implemented IGameInputDevice methods. Every other slot is a not-implemented stub.</summary>
internal static unsafe class DeviceMethods
{
    public static void Fill(nint* vtable)
    {
        delegate* unmanaged<nint, Guid*, nint*, int> query = &QueryInterface;
        delegate* unmanaged<nint, nint> info = &GetDeviceInfo;
        delegate* unmanaged<nint, uint> status = &GetDeviceStatus;
        delegate* unmanaged<nint, byte*, void> battery = &GetBatteryState;
        delegate* unmanaged<nint, byte*, void> rumble = &SetRumbleState;
        vtable[GameInputLayout.QueryInterfaceSlot] = (nint)query;

        vtable[GameInputLayout.DeviceGetDeviceInfo] = (nint)info;
        vtable[GameInputLayout.DeviceGetDeviceStatus] = (nint)status;
        vtable[GameInputLayout.DeviceGetBatteryState] = (nint)battery;
        vtable[GameInputLayout.DeviceSetRumbleState] = (nint)rumble;
    }

    [UnmanagedCallersOnly]
    private static int QueryInterface(nint self, Guid* iid, nint* result)
    {
        try
        {
            return InputObjects.Query(self, iid, result, GameInputLayout.DeviceIid);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "DeviceMethods.QueryInterface");

            return HResult.Fail;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static nint GetDeviceInfo(nint self)
    {
        return ((DeviceObject*)self)->Info;
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static uint GetDeviceStatus(nint self)
    {
        try
        {
            return Volatile.Read(ref ((DeviceObject*)self)->Status);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "DeviceMethods.GetDeviceStatus");

            return default;
        }
    }

    /// <summary>No source reports a battery: the state is zero with an unknown status.</summary>
    [UnmanagedCallersOnly]
    private static void GetBatteryState(nint self, byte* state)
    {
        try
        {
            new Span<byte>(state, GameInputLayout.BatteryStateSize).Clear();
            *(int*)(state + GameInputLayout.BatteryStatusOffset) = GameInputLayout.BatteryUnknown;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "DeviceMethods.GetBatteryState");
        }
    }

    /// <summary>The low- and high-frequency motors go to the source; trigger motors are ignored. Null stops both.</summary>
    [UnmanagedCallersOnly]
    private static void SetRumbleState(nint self, byte* parameters)
    {
        try
        {
            float low = 0;
            float high = 0;

            if (parameters != null)
            {
                low = *(float*)(parameters + GameInputLayout.RumbleLowFrequencyOffset);
                high = *(float*)(parameters + GameInputLayout.RumbleHighFrequencyOffset);
            }

            InputObjects.Session(self).SetRumble(((ObjectHeader*)self)->Index, low, high);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "DeviceMethods.SetRumbleState");
        }
    }
}
