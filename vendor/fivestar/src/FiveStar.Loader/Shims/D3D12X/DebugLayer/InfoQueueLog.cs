// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// The debug layer's messages in the loader log. ID3D12InfoQueue1::RegisterMessageCallback delivers each message on
/// the thread of the call that caused it, so an error is logged with the mapped frames that made the call. Without
/// ID3D12InfoQueue1 the stored messages are drained after a failed creation and on device removal.
/// </summary>
internal sealed unsafe class InfoQueueLog
{
    /// <summary>ID3D12InfoQueue and ID3D12InfoQueue1 slots (d3d12sdklayers.h).</summary>
    private const int ClearStoredMessagesSlot = 4;
    private const int GetMessageSlot = 5;
    private const int GetNumStoredMessagesSlot = 8;
    private const int AddStorageFilterEntriesSlot = 12;
    private const int RegisterMessageCallbackSlot = 38;
    private const int UnregisterMessageCallbackSlot = 39;

    /// <summary>D3D12_MESSAGE_CALLBACK_FLAG_NONE: the storage filter applies to the callback too.</summary>
    private const int CallbackFlags = 0;

    /// <summary>
    /// D3D12_MESSAGE_ID_CREATEINPUTLAYOUT_TYPE_MISMATCH (d3d12sdklayers.h): an element format whose type differs from
    /// the shader input's. Translated vertex shaders declare every fetched attribute float4 and read its bits with
    /// asuint, as the console's VGPRs hold them, so an integer element (R8G8B8A8_UINT blend indices) is meant to arrive
    /// unconverted; the warning is denied rather than logged for each such pipeline.
    /// </summary>
    private const int InputLayoutTypeMismatch = 245;

    /// <summary>D3D12_MESSAGE_SEVERITY values (d3d12sdklayers.h): 0 corruption, 1 error, 2 warning, 3 info, 4 message.</summary>
    private const int WarningSeverity = 2;

    /// <summary>D3D12_MESSAGE: Category, Severity, ID, then pDescription at +0x10.</summary>
    private const int MessageDescriptionOffset = 0x10;
    private const int MessageSeverityOffset = 4;
    private const int MessageIdOffset = 8;

    /// <summary>How deep an error's stack is walked and how many mapped frames are logged.</summary>
    private const int StackDepth = 48;
    private const int MappedFrames = 6;
    private const string MappedPrefix = "[mapped] ";

    private static readonly string[] SeverityNames = ["corruption", "error", "warning", "info", "message"];
    private static readonly Guid InfoQueueIid = new("0742a90b-c387-483f-b946-30a7e4e61458");
    private static readonly Guid InfoQueue1Iid = new("2852dd88-b484-4c0c-b6b1-67168500e600");

    private readonly object _gate = new();
    private readonly ImageTable _images;
    private readonly LoaderLog _log;
    private nint _queue;
    private bool _callback;
    private GCHandle _self;
    private uint _cookie;

    public InfoQueueLog(ImageTable images, LoaderLog log)
    {
        _images = images;
        _log = log;
    }

    /// <summary>Registers the message callback on the device's info queue, or keeps the queue for draining.</summary>
    public void Attach(nint device)
    {
        AttachQueue(device);
        DenyExpected();
    }

    private void AttachQueue(nint device)
    {
        Guid iid1 = InfoQueue1Iid;
        nint queue1 = 0;

        if (HResult.Ok == QueryInterface(device, &iid1, &queue1))
        {
            delegate* unmanaged<int, int, int, byte*, void*, void> callback = &OnMessage;
            uint cookie;
            var register = (delegate* unmanaged<nint, nint, int, void*, uint*, int>)ComObject.Slot(queue1, RegisterMessageCallbackSlot);

            // The context is this log (kept alive by the handle), so the warm-up, which has no runtime, logs too.
            _self = GCHandle.Alloc(this);
            int hr = register(queue1, (nint)callback, CallbackFlags, (void*)GCHandle.ToIntPtr(_self), &cookie);

            _cookie = cookie;
            _callback = !HResult.Failed(hr);
            _queue = queue1;
            _log.Info($"  [d3d12-debug] info-queue messages through RegisterMessageCallback -> {HResult.Format(hr)}");

            if (_callback)
            {
                return;
            }
        }

        Guid iid = InfoQueueIid;
        nint queue = 0;
        int found = QueryInterface(device, &iid, &queue);
        _queue = queue;
        _log.Info($"  [d3d12-debug] no message callback: stored messages drained after failures (ID3D12InfoQueue {HResult.Format(found)})");
    }

    /// <summary>Stops the callback, so the runtime does not call into a process whose runtime is shutting down.</summary>
    public void Detach()
    {
        if (!_callback)
        {
            return;
        }

        _callback = false;
        ((delegate* unmanaged<nint, uint, int>)ComObject.Slot(_queue, UnregisterMessageCallbackSlot))(_queue, _cookie);
    }

    /// <summary>Logs and clears the stored messages, when there is no callback.</summary>
    public void Drain()
    {
        lock (_gate)
        {
            if (_callback || _queue == 0)
            {
                return;
            }

            ulong count = ((delegate* unmanaged<nint, ulong>)ComObject.Slot(_queue, GetNumStoredMessagesSlot))(_queue);

            for (ulong index = 0; index < count; index++)
            {
                DrainOne(index);
            }

            ((delegate* unmanaged<nint, void>)ComObject.Slot(_queue, ClearStoredMessagesSlot))(_queue);
        }
    }

    /// <summary>One message: errors and corruption as errors with the mapped callers, warnings as warnings, the rest as details.</summary>
    public void Write(int severity, int id, string description)
    {
        string name = severity >= 0 && severity < SeverityNames.Length ? SeverityNames[severity] : severity.ToString(System.Globalization.CultureInfo.InvariantCulture);
        string line = $"[d3d12-debug] {name} #{id}: {description}";

        if (severity > WarningSeverity)
        {
            _log.Detail("  " + line);

            return;
        }

        if (severity == WarningSeverity)
        {
            _log.Warn(line);

            return;
        }

        _log.Error(line);
        LogMappedCallers();
    }

    [UnmanagedCallersOnly]
    private static void OnMessage(int category, int severity, int id, byte* description, void* context)
    {
        try
        {
            string text = description == null ? string.Empty : Marshal.PtrToStringAnsi((nint)description) ?? string.Empty;
            ((InfoQueueLog)GCHandle.FromIntPtr((nint)context).Target!).Write(severity, id, text);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "InfoQueueLog.OnMessage");
        }
    }

    /// <summary>Keeps the messages that describe intended behaviour out of storage, and so out of the callback.</summary>
    private void DenyExpected()
    {
        if (_queue == 0)
        {
            return;
        }

        int* ids = stackalloc int[] { InputLayoutTypeMismatch };
        var filter = new InfoQueueFilter { DenyIdCount = 1, DenyIds = ids };
        int hr = ((delegate* unmanaged<nint, InfoQueueFilter*, int>)ComObject.Slot(_queue, AddStorageFilterEntriesSlot))(_queue, &filter);
        _log.Info($"  [d3d12-debug] message #{InputLayoutTypeMismatch} (input-layout type mismatch, intended) denied -> {HResult.Format(hr)}");
    }

    private static int QueryInterface(nint unknown, Guid* iid, nint* result)
    {
        return ((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(unknown, ComObject.QueryInterfaceSlot))(unknown, iid, result);
    }

    private void DrainOne(ulong index)
    {
        var get = (delegate* unmanaged<nint, ulong, byte*, nuint*, int>)ComObject.Slot(_queue, GetMessageSlot);
        nuint length = 0;

        if (HResult.Failed(get(_queue, index, null, &length)) || length == 0)
        {
            return;
        }

        byte* message = (byte*)NativeMemory.Alloc(length);

        try
        {
            if (!HResult.Failed(get(_queue, index, message, &length)))
            {
                nint text = *(nint*)(message + MessageDescriptionOffset);
                Write(*(int*)(message + MessageSeverityOffset), *(int*)(message + MessageIdOffset), Marshal.PtrToStringAnsi(text) ?? string.Empty);
            }
        }
        finally
        {
            NativeMemory.Free(message);
        }
    }

    private void LogMappedCallers()
    {
        int logged = 0;

        foreach (var frame in NativeStack.Walk(_images, StackDepth))
        {
            if (logged == MappedFrames || !frame.Where.StartsWith(MappedPrefix, StringComparison.Ordinal))
            {
                continue;
            }

            _log.Error($"    called from {frame.Where}");
            logged++;
        }
    }
}
