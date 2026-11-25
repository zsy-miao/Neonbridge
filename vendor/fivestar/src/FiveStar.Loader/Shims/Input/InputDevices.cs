// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// The device objects, their status and the latest reading of each. Publishing a state that equals the latest one
/// does nothing, so a new reading appears only when input changes, as in GameInput.
/// </summary>
internal sealed unsafe class InputDevices : IDisposable
{
    private readonly DeviceObject*[] _objects;
    private readonly nint[] _latest;
    private readonly ReadingRing _ring;
    private readonly DeviceCallbacks _callbacks;

    public InputDevices(IReadOnlyList<DeviceDescription> descriptions, nint vtable, nint owner, ReadingRing ring, DeviceCallbacks callbacks)
    {
        _objects = new DeviceObject*[descriptions.Count];
        _latest = new nint[descriptions.Count];
        _ring = ring;
        _callbacks = callbacks;

        for (int i = 0; i < descriptions.Count; i++)
        {
            var device = (DeviceObject*)NativeMemory.AllocZeroed((nuint)sizeof(DeviceObject));
            device->Header.Vtable = vtable;
            device->Header.Owner = owner;
            device->Header.References = 1;
            device->Header.Index = i;

            device->Info = DeviceInfoWriter.Write(descriptions[i]);
            device->Kind = descriptions[i].Kind;
            _objects[i] = device;
        }
    }

    public int Count => _objects.Length;

    [HotPath]
    public DeviceObject* Object(int index)
    {
        return _objects[index];
    }

    /// <summary>The connected device whose APP_LOCAL_DEVICE_ID is <paramref name="id"/>, else null.</summary>
    public DeviceObject* FindConnected(ReadOnlySpan<byte> id)
    {
        foreach (DeviceObject* device in _objects)
        {
            var deviceId = new ReadOnlySpan<byte>((byte*)device->Info + GameInputLayout.InfoDeviceIdOffset, GameInputLayout.DeviceIdSize);

            if ((device->Status & GameInputLayout.StatusConnected) != 0 && deviceId.SequenceEqual(id))
            {
                return device;
            }
        }

        return null;
    }

    /// <summary>Sets a device's GameInputDeviceStatus and reports a change to the device callbacks.</summary>
    [HotPath]
    public void SetStatus(int index, uint status, ulong now)
    {
        DeviceObject* device = _objects[index];
        uint previous = device->Status;

        if (previous == status)
        {
            return;
        }

        device->Status = status;
        _callbacks.Notify(device, previous, now);
    }

    [HotPath]
    public void PublishGamepad(int index, GamepadState state, ulong now)
    {
        var latest = (ReadingObject*)_latest[index];

        if (latest != null && latest->Gamepad.Equals(state))
        {
            return;
        }

        ReadingObject* reading = Begin(index, GameInputLayout.KindGamepad, now);

        if (reading != null)
        {
            reading->Gamepad = state;
            Commit(index, reading);
        }
    }

    [HotPath]
    public void PublishMouse(int index, MouseState state, ulong now)
    {
        var latest = (ReadingObject*)_latest[index];
        bool same = latest != null && latest->Mouse.Buttons == state.Buttons && latest->Mouse.PositionX == state.PositionX && latest->Mouse.PositionY == state.PositionY;

        if (same)
        {
            return;
        }

        ReadingObject* reading = Begin(index, GameInputLayout.KindMouse, now);

        if (reading != null)
        {
            reading->Mouse = state;
            Commit(index, reading);
        }
    }

    [HotPath]
    public void PublishKeyboard(int index, ref KeyBuffer keys, int count, ulong now)
    {
        var latest = (ReadingObject*)_latest[index];

        if (latest != null && SameKeys(ref latest->Keys, latest->KeyCount, ref keys, count))
        {
            return;
        }

        ReadingObject* reading = Begin(index, GameInputLayout.KindKeyboard, now);

        if (reading != null)
        {
            reading->Keys = keys;
            reading->KeyCount = count;
            Commit(index, reading);
        }
    }

    public void Dispose()
    {
        foreach (DeviceObject* device in _objects)
        {
            NativeMemory.Free((void*)device->Info);
            NativeMemory.Free(device);
        }
    }

    [HotPath]
    private ReadingObject* Begin(int index, uint kind, ulong now)
    {
        ReadingObject* reading = _ring.Claim();

        if (reading == null)
        {
            return null;
        }

        reading->Header.Index = index;
        reading->Device = (nint)_objects[index];
        reading->Kind = kind;
        reading->Timestamp = now;

        reading->KeyCount = 0;
        reading->Gamepad = default;
        reading->Mouse = default;

        return reading;
    }

    /// <summary>Publishes the reading and moves the writer's reference from the previous latest reading to it.</summary>
    [HotPath]
    private void Commit(int index, ReadingObject* reading)
    {
        _ring.Publish(reading);
        nint previous = Interlocked.Exchange(ref _latest[index], (nint)reading);

        if (previous != 0)
        {
            InputObjects.Drop(previous);
        }
    }

    [HotPath]
    private static bool SameKeys(ref KeyBuffer left, int leftCount, ref KeyBuffer right, int rightCount)
    {
        if (leftCount != rightCount)
        {
            return false;
        }

        for (int i = 0; i < leftCount; i++)
        {
            if (left[i].VirtualKey != right[i].VirtualKey)
            {
                return false;
            }
        }

        return true;
    }
}
