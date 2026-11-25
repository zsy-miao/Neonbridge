// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Input;

/// <summary>The implemented IGameInput methods. Every other slot is a not-implemented stub.</summary>
internal static unsafe class GameInputMethods
{
    private const long NoSequence = 0;

    public static void Fill(nint* vtable)
    {
        delegate* unmanaged<nint, Guid*, nint*, int> query = &QueryInterface;
        delegate* unmanaged<nint, ulong> timestamp = &GetCurrentTimestamp;
        delegate* unmanaged<nint, uint, nint, nint*, int> current = &GetCurrentReading;
        delegate* unmanaged<nint, nint, uint, nint, nint*, int> next = &GetNextReading;
        delegate* unmanaged<nint, nint, uint, nint, nint*, int> previous = &GetPreviousReading;
        delegate* unmanaged<nint, nint, uint, uint, uint, nint, nint, ulong*, int> register = &RegisterDeviceCallback;

        delegate* unmanaged<nint, ulong, void> stop = &StopCallback;
        delegate* unmanaged<nint, ulong, ulong, byte> unregister = &UnregisterCallback;
        delegate* unmanaged<nint, byte*, nint*, int> find = &FindDeviceFromId;
        vtable[GameInputLayout.QueryInterfaceSlot] = (nint)query;
        vtable[GameInputLayout.InputGetCurrentTimestamp] = (nint)timestamp;
        vtable[GameInputLayout.InputGetCurrentReading] = (nint)current;

        vtable[GameInputLayout.InputGetNextReading] = (nint)next;
        vtable[GameInputLayout.InputGetPreviousReading] = (nint)previous;
        vtable[GameInputLayout.InputRegisterDeviceCallback] = (nint)register;
        vtable[GameInputLayout.InputStopCallback] = (nint)stop;
        vtable[GameInputLayout.InputUnregisterCallback] = (nint)unregister;
        vtable[GameInputLayout.InputFindDeviceFromId] = (nint)find;
    }

    [UnmanagedCallersOnly]
    private static int QueryInterface(nint self, Guid* iid, nint* result)
    {
        try
        {
            return InputObjects.Query(self, iid, result, GameInputLayout.InputIid);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "GameInputMethods.QueryInterface");

            return HResult.Fail;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static ulong GetCurrentTimestamp(nint self)
    {
        try
        {
            return InputObjects.Session(self).Timestamp();
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "GameInputMethods.GetCurrentTimestamp");

            return default;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static int GetCurrentReading(nint self, uint kind, nint device, nint* reading)
    {
        try
        {
            return InputObjects.Session(self).GetReading(kind, device, NoSequence, long.MaxValue, true, reading);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "GameInputMethods.GetCurrentReading");

            return HResult.Fail;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static int GetNextReading(nint self, nint reference, uint kind, nint device, nint* reading)
    {
        try
        {
            if (reference == 0)
            {
                return HResult.InvalidArgument;
            }

            long after = ((ReadingObject*)reference)->Sequence;

            return InputObjects.Session(self).GetReading(kind, device, after, long.MaxValue, false, reading);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "GameInputMethods.GetNextReading");

            return HResult.Fail;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static int GetPreviousReading(nint self, nint reference, uint kind, nint device, nint* reading)
    {
        try
        {
            if (reference == 0)
            {
                return HResult.InvalidArgument;
            }

            long before = ((ReadingObject*)reference)->Sequence;

            return InputObjects.Session(self).GetReading(kind, device, NoSequence, before, true, reading);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "GameInputMethods.GetPreviousReading");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int RegisterDeviceCallback(nint self, nint device, uint kind, uint statusFilter, uint enumeration, nint context, nint function, ulong* token)
    {
        try
        {
            return InputObjects.Session(self).RegisterDeviceCallback(device, kind, statusFilter, enumeration, context, function, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "GameInputMethods.RegisterDeviceCallback");

            return HResult.Fail;
        }
    }

    /// <summary>StopCallback: the callback is removed at once.</summary>
    [UnmanagedCallersOnly]
    private static void StopCallback(nint self, ulong token)
    {
        try
        {
            InputObjects.Session(self).Unregister(token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "GameInputMethods.StopCallback");
        }
    }

    [UnmanagedCallersOnly]
    private static int FindDeviceFromId(nint self, byte* id, nint* device)
    {
        try
        {
            return InputObjects.Session(self).FindDevice(id, device);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "GameInputMethods.FindDeviceFromId");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static byte UnregisterCallback(nint self, ulong token, ulong timeoutMicroseconds)
    {
        try
        {
            bool removed = InputObjects.Session(self).Unregister(token);
            byte result = removed ? (byte)1 : (byte)0;

            return result;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "GameInputMethods.UnregisterCallback");

            return default;
        }
    }
}
