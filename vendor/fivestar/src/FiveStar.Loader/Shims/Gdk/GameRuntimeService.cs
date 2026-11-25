// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Globalization;
using System.Runtime.InteropServices;
using FiveStar.Formats.Platform;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XGameRuntime initialization and the thunk-library ABI. Titles linked against xgameruntime.lib call
/// <c>QueryApiImpl(&amp;clsid, &amp;iid, &amp;object)</c> and then COM-style vtable slots; this service answers from a
/// (clsid, iid, slot) to function-name table, building each object's vtable from drop-this thunks into the named
/// shims. Unmapped slots return E_NOTIMPL; unknown pairs return REGDB_E_CLASSNOTREG, and each requested pair is logged
/// once (the census).
/// </summary>
internal sealed unsafe class GameRuntimeService
{
    /// <summary>Slots 0..2 are IUnknown: QueryInterface, AddRef, Release.</summary>
    public const int FirstMethodSlot = 3;

    private const int QueryInterfaceSlot = 0;
    private const int AddRefSlot = 1;
    private const int ReleaseSlot = 2;

    /// <summary>An object is a vtable pointer followed by one unused pointer.</summary>
    private const int ObjectSize = 2 * sizeof(ulong);

    private const char CommentMark = '#';

    // XGameRuntimeFeature values (XGameRuntimeFeature.h) of the features reported unavailable.
    private const uint FeatureAppCapture = 1;
    private const uint FeatureSpeechSynthesizer = 13;
    private const uint FeatureGameStreaming = 21;

    private readonly ConcurrentDictionary<uint, bool> _featureQueries = new();
    private readonly ConcurrentDictionary<(Guid Clsid, Guid Iid), int> _census = new();
    private readonly Dictionary<(Guid Clsid, Guid Iid), nint> _objects = [];
    private readonly Dictionary<(Guid Clsid, Guid Iid), SortedDictionary<int, string>> _abiTable = [];
    private readonly List<string> _initCalls = [];
    private readonly GdkConfig _config;
    private readonly ShimTable _shims;
    private readonly UnresolvedStubs _stubs;
    private readonly CodeHeap _heap;
    private readonly CallTrace _trace;
    private readonly LoaderLog _log;
    private volatile bool _initialized;

    public GameRuntimeService(GdkConfig config, ShimTable shims, UnresolvedStubs stubs, CodeHeap heap, CallTrace trace, LoaderLog log)
    {
        _config = config;
        _shims = shims;
        _stubs = stubs;
        _heap = heap;
        _trace = trace;
        _log = log;
    }

    public bool Initialized => _initialized;

    /// <summary>The (clsid, iid) pairs requested so far and the HRESULT each got.</summary>
    public IReadOnlyDictionary<(Guid Clsid, Guid Iid), int> Census => _census;

    public IReadOnlyList<string> InitCalls
    {
        get
        {
            lock (_initCalls)
            {
                return [.. _initCalls];
            }
        }
    }

    /// <summary>XGameRuntimeInitialize and its variants: always S_OK.</summary>
    public int Initialize(string what)
    {
        _config.Ensure();
        _initialized = true;
        _log.Info($"  [gdk] {what} -> S_OK (offline shim)");

        return HResult.Ok;
    }

    public void Uninitialize(string what)
    {
        _initialized = false;
        _log.Info($"  [gdk] {what}");
    }

    /// <summary>InitializeApiImpl*: recorded and answered with S_OK.</summary>
    public int InitializeApi(string what)
    {
        _config.Ensure();
        _initialized = true;

        lock (_initCalls)
        {
            _initCalls.Add(what);
        }

        _log.Info($"  [gdk] {what} -> S_OK (thunk-library init)");

        return HResult.Ok;
    }

    /// <summary>
    /// Every feature is available except the ones that need console hardware or services with no PC stand-in: app
    /// capture, speech synthesis and game streaming. Each feature is logged once.
    /// </summary>
    public bool IsFeatureAvailable(uint feature)
    {
        bool available = feature != FeatureAppCapture && feature != FeatureSpeechSynthesizer && feature != FeatureGameStreaming;

        if (_featureQueries.TryAdd(feature, available))
        {
            _log.Info($"  [gdk] XGameRuntimeIsFeatureAvailable({feature}) -> {available} (logged once per feature)");
        }

        return available;
    }

    public void ReportError(int hr, byte* message)
    {
        _log.Warn($"[gdk] XErrorReport({HResult.Format(hr)}): {Utf8Text.Read((nint)message)}");
    }

    /// <summary>Maps vtable slot <paramref name="slot"/> of (clsid, iid) to a named shim; before the first query of that pair.</summary>
    public void RegisterAbi(Guid clsid, Guid iid, int slot, string function)
    {
        ArgumentOutOfRangeException.ThrowIfLessThan(slot, FirstMethodSlot);

        lock (_abiTable)
        {
            if (!_abiTable.TryGetValue((clsid, iid), out var slots))
            {
                slots = [];
                _abiTable[(clsid, iid)] = slots;
            }

            slots[slot] = function;
        }
    }

    /// <summary>Loads "clsid iid slot FunctionName" lines ('#' starts a comment); returns the number of entries.</summary>
    public int LoadAbiTable(string path)
    {
        int count = 0;

        foreach (string raw in File.ReadLines(path))
        {
            string line = raw.Split(CommentMark)[0].Trim();

            if (line.Length == 0)
            {
                continue;
            }

            var fields = line.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);
            var clsid = Guid.Parse(fields[(int)AbiField.Clsid], CultureInfo.InvariantCulture);
            var iid = Guid.Parse(fields[(int)AbiField.Iid], CultureInfo.InvariantCulture);
            RegisterAbi(clsid, iid, int.Parse(fields[(int)AbiField.Slot], CultureInfo.InvariantCulture), fields[(int)AbiField.Function]);
            count++;
        }

        _log.Info($"  [gdk] loaded {count} XGameRuntime ABI entries from {path}");

        return count;
    }

    /// <summary>Registers the named thunks read from a GDK library; returns the number of entries.</summary>
    public int LoadAbiLibrary(IReadOnlyList<GdkThunk> thunks, string source)
    {
        int count = 0;

        foreach (var thunk in thunks)
        {
            if (thunk.Name is not null && thunk.Slot >= FirstMethodSlot)
            {
                RegisterAbi(thunk.Clsid, thunk.Iid, thunk.Slot, thunk.Name);
                count++;
            }
        }

        _log.Info($"  [gdk] loaded {count} XGameRuntime ABI entries from {source}");

        return count;
    }

    /// <summary>HRESULT QueryApiImpl(const GUID* clsid, const GUID* iid, void** object) (inferred from call sites).</summary>
    public int Query(Guid* clsid, Guid* iid, nint* result)
    {
        if (clsid == null || iid == null || result == null)
        {
            return HResult.Pointer;
        }

        var key = (*clsid, *iid);
        *result = 0;
        nint created = ObjectFor(key);
        int hr = created != 0 ? HResult.Ok : HResult.ClassNotRegistered;

        if (_census.TryAdd(key, hr))
        {
            string answer = created != 0 ? "bridged object" : "REGDB_E_CLASSNOTREG (no ABI table entry)";
            _log.Info($"  [gdk] QueryApiImpl({key.Item1}, {key.Item2}) -> {answer} (logged once per pair)");
        }

        *result = created;

        return hr;
    }

    private nint ObjectFor((Guid Clsid, Guid Iid) key)
    {
        lock (_objects)
        {
            if (!_objects.TryGetValue(key, out nint created))
            {
                created = BuildObject(key);
                _objects[key] = created;
            }

            return created;
        }
    }

    private nint BuildObject((Guid Clsid, Guid Iid) key)
    {
        SortedDictionary<int, string>? slots;

        lock (_abiTable)
        {
            _abiTable.TryGetValue(key, out slots);
        }

        if (slots is null)
        {
            return 0;
        }

        int count = slots.Keys.Max() + 1;
        nint* vtable = (nint*)NativeMemory.AllocZeroed((nuint)(count * sizeof(nint)));
        WriteUnknown(vtable);

        for (int i = FirstMethodSlot; i < count; i++)
        {
            vtable[i] = DropThisThunk.Create(_heap, _log, Traced(SlotTarget(key, slots, i), key, slots, i));
        }

        // One read-write to read-execute flip for every stub and thunk of this object.
        _heap.Seal();
        nint* instance = (nint*)NativeMemory.AllocZeroed(ObjectSize);
        instance[0] = (nint)vtable;

        return (nint)instance;
    }

    private static void WriteUnknown(nint* vtable)
    {
        delegate* unmanaged<nint, Guid*, nint*, int> queryInterface = &AbiObjectCallbacks.QueryInterface;
        delegate* unmanaged<nint, uint> addRef = &AbiObjectCallbacks.AddRef;
        vtable[QueryInterfaceSlot] = (nint)queryInterface;
        vtable[AddRefSlot] = (nint)addRef;
        vtable[ReleaseSlot] = (nint)addRef;
    }

    /// <summary>While the call trace is on (<c>--run</c>), GDK calls are recorded and counted like imports.</summary>
    private nint Traced(nint target, (Guid Clsid, Guid Iid) key, SortedDictionary<int, string> slots, int slot)
    {
        if (!_trace.Enabled)
        {
            return target;
        }

        string name = slots.GetValueOrDefault(slot) ?? $"{key.Clsid}/{key.Iid}#slot{slot}";

        return _trace.Wrap(target, $"{GdkModule.Name}!{name}");
    }

    /// <summary>The named shim of a slot, else a not-implemented stub (E_NOTIMPL unless the catalog says otherwise).</summary>
    private nint SlotTarget((Guid Clsid, Guid Iid) key, SortedDictionary<int, string> slots, int slot)
    {
        if (slots.TryGetValue(slot, out string? name))
        {
            var entry = _shims.TryResolve(GdkModule.Name, name, 0);

            if (entry is not null)
            {
                return entry.Address;
            }
        }

        string what = name ?? $"{key.Clsid}/{key.Iid}#slot{slot}";
        long value = HResult.NotImplemented;

        if (_shims.NotImplementedFunctions.TryGetValue((GdkModule.Name, what), out long listed))
        {
            value = listed;
        }

        return _stubs.Create(GdkModule.Name, what, value, true, false);
    }
}
