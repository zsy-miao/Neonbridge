// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// RegisterDeviceCallback registrations. The initial enumeration runs on the registering thread before it returns,
/// for asynchronous and blocking enumeration alike; later status changes are reported on the thread whose call
/// polled them.
/// </summary>
internal sealed unsafe class DeviceCallbacks
{
    private readonly object _gate = new();
    private readonly List<DeviceCallback> _registrations = [];
    private ulong _lastToken;

    public DeviceCallback Add(nint device, uint kind, uint statusFilter, nint context, nint function)
    {
        lock (_gate)
        {
            _lastToken++;
            var registration = new DeviceCallback(_lastToken, device, kind, statusFilter, context, function);
            _registrations.Add(registration);

            return registration;
        }
    }

    public bool Remove(ulong token)
    {
        lock (_gate)
        {
            return _registrations.RemoveAll(r => r.Token == token) > 0;
        }
    }

    /// <summary>Reports every connected device the registration watches, with a previous status of none.</summary>
    public static void Enumerate(DeviceCallback registration, InputDevices devices, ulong now)
    {
        for (int i = 0; i < devices.Count; i++)
        {
            DeviceObject* device = devices.Object(i);

            if (Watches(registration, device) && (device->Status & registration.StatusFilter) != 0)
            {
                Invoke(registration, device, now, device->Status, GameInputLayout.StatusNone);
            }
        }
    }

    /// <summary>Reports a status change of one device to every registration that watches the changed bits.</summary>
    public void Notify(DeviceObject* device, uint previous, ulong now)
    {
        DeviceCallback[] registrations;

        lock (_gate)
        {
            registrations = [.. _registrations];
        }

        uint changed = device->Status ^ previous;

        foreach (var registration in registrations)
        {
            if (Watches(registration, device) && (changed & registration.StatusFilter) != 0)
            {
                Invoke(registration, device, now, device->Status, previous);
            }
        }
    }

    private static bool Watches(DeviceCallback registration, DeviceObject* device)
    {
        if (registration.Device != 0 && registration.Device != (nint)device)
        {
            return false;
        }

        return registration.Kind == GameInputLayout.KindUnknown || (device->Kind & registration.Kind) != 0;
    }

    private static void Invoke(DeviceCallback registration, DeviceObject* device, ulong now, uint current, uint previous)
    {
        var callback = (delegate* unmanaged<ulong, nint, nint, ulong, uint, uint, void>)registration.Function;
        callback(registration.Token, registration.Context, (nint)device, now, current, previous);
    }
}
