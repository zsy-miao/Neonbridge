// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// The native side of GameInput, built on the first GameInputCreate: the vtables, the IGameInput object, the devices,
/// the reading history and the poller. Every native object carries a GCHandle of this session, which is how the
/// vtable entry points find it.
/// </summary>
internal sealed unsafe class GameInputSession : IDisposable
{
    private readonly ConcurrentDictionary<Guid, bool> _queries = new();
    private readonly DeviceCallbacks _callbacks = new();
    private readonly IGamepadSource _gamepads;
    private readonly TimeProvider _time;
    private readonly LoaderLog _log;
    private readonly GCHandle _self;
    private readonly InputVtables _vtables;
    private readonly ReadingRing _ring;
    private readonly InputDevices _devices;
    private readonly InputPoller _poller;
    private readonly ObjectHeader* _input;
    private long _readingCalls;
    private long _callbackRegistrations;

    public GameInputSession(IGamepadSource gamepads, IDesktopSource desktop, InputConfig config, TimeProvider time, UnresolvedStubs stubs, CodeHeap heap, LoaderLog log)
    {
        _gamepads = gamepads;
        _time = time;
        _log = log;
        _self = GCHandle.Alloc(this);
        nint owner = GCHandle.ToIntPtr(_self);
        _vtables = new InputVtables(stubs, heap);

        _ring = new ReadingRing(_vtables.Reading, owner);
        _devices = new InputDevices(InputDeviceCatalog.Describe(gamepads.SlotCount), _vtables.Device, owner, _ring, _callbacks);
        _poller = new InputPoller(gamepads, desktop, _devices, config, log);
        _input = (ObjectHeader*)NativeMemory.AllocZeroed((nuint)sizeof(ObjectHeader));
        _input->Vtable = _vtables.Input;
        _input->Owner = owner;

        _poller.Start(Timestamp());
    }

    /// <summary>Reading queries and device-callback registrations so far, for the thread sampler.</summary>
    public string Summary => $"{Volatile.Read(ref _readingCalls)} reading queries, {Volatile.Read(ref _callbackRegistrations)} device-callback registrations";

    /// <summary>The IGameInput object with one more reference.</summary>
    public nint AddInputReference()
    {
        Interlocked.Increment(ref _input->References);

        return (nint)_input;
    }

    public void Configure(InputConfig config)
    {
        _poller.Configure(config, Timestamp());
    }

    /// <summary>GetCurrentTimestamp: microseconds of the process clock.</summary>
    [HotPath]
    public ulong Timestamp()
    {
        long ticks = _time.GetTimestamp();
        long frequency = _time.TimestampFrequency;
        long whole = ticks / frequency * InputLimits.MicrosecondsPerSecond;

        return (ulong)(whole + (ticks % frequency * InputLimits.MicrosecondsPerSecond / frequency));
    }

    /// <summary>Polls the sources, then finds the newest or oldest matching reading strictly between two sequence numbers.</summary>
    [HotPath]
    public int GetReading(uint kind, nint device, long after, long before, bool newest, nint* result)
    {
        if (result == null)
        {
            return HResult.Pointer;
        }

        Interlocked.Increment(ref _readingCalls);
        _poller.Poll(Timestamp());
        ReadingObject* found = _ring.Acquire(kind, device, after, before, newest);
        *result = (nint)found;

        if (found == null)
        {
            return GameInputLayout.ReadingNotFound;
        }

        return HResult.Ok;
    }

    public int RegisterDeviceCallback(nint device, uint kind, uint statusFilter, uint enumeration, nint context, nint function, ulong* token)
    {
        if (token != null)
        {
            *token = 0;
        }

        if (function == 0)
        {
            return HResult.InvalidArgument;
        }

        Interlocked.Increment(ref _callbackRegistrations);
        ulong now = Timestamp();
        _poller.Poll(now);
        var registration = _callbacks.Add(device, kind, statusFilter, context, function);

        if (enumeration != GameInputLayout.NoEnumeration)
        {
            DeviceCallbacks.Enumerate(registration, _devices, now);
        }

        if (token != null)
        {
            *token = registration.Token;
        }

        return HResult.Ok;
    }

    /// <summary>FindDeviceFromId: the connected device with that APP_LOCAL_DEVICE_ID, with one more reference.</summary>
    public int FindDevice(byte* id, nint* result)
    {
        if (id == null || result == null)
        {
            return HResult.Pointer;
        }

        _poller.Poll(Timestamp());
        DeviceObject* device = _devices.FindConnected(new ReadOnlySpan<byte>(id, GameInputLayout.DeviceIdSize));
        *result = (nint)device;

        if (device == null)
        {
            return GameInputLayout.DeviceNotFound;
        }

        Interlocked.Increment(ref device->Header.References);

        return HResult.Ok;
    }

    public bool Unregister(ulong token)
    {
        return _callbacks.Remove(token);
    }

    public void SetRumble(int device, float lowFrequency, float highFrequency)
    {
        if (device < _gamepads.SlotCount)
        {
            _gamepads.SetRumble(device, lowFrequency, highFrequency);
        }
    }

    /// <summary>Logs a QueryInterface for an interface the shim does not have, once per interface id.</summary>
    public void LogQuery(Guid iid)
    {
        if (_queries.TryAdd(iid, true))
        {
            _log.Warn($"[input] QueryInterface({iid}) -> E_NOINTERFACE (logged once)");
        }
    }

    public void Dispose()
    {
        _devices.Dispose();
        _ring.Dispose();
        _vtables.Dispose();
        NativeMemory.Free(_input);
        _self.Free();
    }
}
