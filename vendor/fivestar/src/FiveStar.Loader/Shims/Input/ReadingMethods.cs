// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// The implemented IGameInputReading methods: they copy out of the reading object without touching the session.
/// Every other slot is a not-implemented stub (0 for the counts and state getters of other input kinds).
/// </summary>
internal static unsafe class ReadingMethods
{
    private const byte True = 1;
    private const byte False = 0;

    public static void Fill(nint* vtable)
    {
        delegate* unmanaged<nint, Guid*, nint*, int> query = &QueryInterface;
        delegate* unmanaged<nint, uint> kind = &GetInputKind;
        delegate* unmanaged<nint, uint, ulong> sequence = &GetSequenceNumber;
        delegate* unmanaged<nint, ulong> timestamp = &GetTimestamp;
        delegate* unmanaged<nint, nint*, void> device = &GetDevice;
        delegate* unmanaged<nint, nint*, byte> raw = &GetRawReport;

        delegate* unmanaged<nint, uint> keyCount = &GetKeyCount;
        delegate* unmanaged<nint, uint, KeyState*, uint> keys = &GetKeyState;
        delegate* unmanaged<nint, MouseState*, byte> mouse = &GetMouseState;
        delegate* unmanaged<nint, GamepadState*, byte> gamepad = &GetGamepadState;
        vtable[GameInputLayout.QueryInterfaceSlot] = (nint)query;
        vtable[GameInputLayout.ReadingGetInputKind] = (nint)kind;

        vtable[GameInputLayout.ReadingGetSequenceNumber] = (nint)sequence;
        vtable[GameInputLayout.ReadingGetTimestamp] = (nint)timestamp;
        vtable[GameInputLayout.ReadingGetDevice] = (nint)device;
        vtable[GameInputLayout.ReadingGetRawReport] = (nint)raw;
        vtable[GameInputLayout.ReadingGetKeyCount] = (nint)keyCount;
        vtable[GameInputLayout.ReadingGetKeyState] = (nint)keys;

        vtable[GameInputLayout.ReadingGetMouseState] = (nint)mouse;
        vtable[GameInputLayout.ReadingGetGamepadState] = (nint)gamepad;
    }

    [UnmanagedCallersOnly]
    private static int QueryInterface(nint self, Guid* iid, nint* result)
    {
        try
        {
            return InputObjects.Query(self, iid, result, GameInputLayout.ReadingIid);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ReadingMethods.QueryInterface");

            return HResult.Fail;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static uint GetInputKind(nint self)
    {
        return ((ReadingObject*)self)->Kind;
    }

    /// <summary>One sequence for every input kind: the reading's place in the history.</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    private static ulong GetSequenceNumber(nint self, uint kind)
    {
        return (ulong)((ReadingObject*)self)->Sequence;
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static ulong GetTimestamp(nint self)
    {
        return ((ReadingObject*)self)->Timestamp;
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static void GetDevice(nint self, nint* device)
    {
        try
        {
            nint value = ((ReadingObject*)self)->Device;
            Interlocked.Increment(ref ((ObjectHeader*)value)->References);
            *device = value;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ReadingMethods.GetDevice");
        }
    }

    [UnmanagedCallersOnly]
    private static byte GetRawReport(nint self, nint* report)
    {
        *report = 0;

        return False;
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static uint GetKeyCount(nint self)
    {
        return (uint)((ReadingObject*)self)->KeyCount;
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static uint GetKeyState(nint self, uint capacity, KeyState* states)
    {
        try
        {
            var reading = (ReadingObject*)self;
            int count = reading->KeyCount;

            if (capacity < (uint)count)
            {
                count = (int)capacity;
            }

            for (int i = 0; i < count; i++)
            {
                states[i] = reading->Keys[i];
            }

            return (uint)count;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ReadingMethods.GetKeyState");

            return default;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static byte GetMouseState(nint self, MouseState* state)
    {
        var reading = (ReadingObject*)self;

        if ((reading->Kind & GameInputLayout.KindMouse) == 0)
        {
            *state = default;

            return False;
        }

        *state = reading->Mouse;

        return True;
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static byte GetGamepadState(nint self, GamepadState* state)
    {
        var reading = (ReadingObject*)self;

        if ((reading->Kind & GameInputLayout.KindGamepad) == 0)
        {
            *state = default;

            return False;
        }

        *state = reading->Gamepad;

        return True;
    }
}
