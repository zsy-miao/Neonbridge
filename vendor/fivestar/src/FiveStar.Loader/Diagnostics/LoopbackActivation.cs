// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// Activates a WASAPI process loopback client: the mix of one process tree's audio, never the rest of the PC's. The
/// activation is asynchronous and wants an agile completion handler; the handler here does nothing, and the caller
/// polls the operation until the client is ready.
/// </summary>
internal static unsafe class LoopbackActivation
{
    private const int PollMilliseconds = 10;
    private const int PollLimit = 500;

    private static nint _handler;

    /// <summary>An IAudioClient on the loopback of <paramref name="processId"/> and its children; throws when it fails.</summary>
    public static nint Activate(int processId)
    {
        var parameters = new ActivationParameters
        {
            Type = Mmdevapi.ProcessLoopbackActivation,
            ProcessId = processId,
            Mode = Mmdevapi.IncludeProcessTree,
        };
        var variant = new BlobVariant { Type = Mmdevapi.BlobVariant, Size = (uint)sizeof(ActivationParameters), Data = &parameters };

        Guid iid = Mmdevapi.AudioClient;
        nint operation = 0;
        int hr;

        fixed (char* path = Mmdevapi.ProcessLoopbackDevice)
        {
            hr = Mmdevapi.ActivateAudioInterfaceAsync(path, &iid, &variant, Handler(), &operation);
        }

        Check(hr, "ActivateAudioInterfaceAsync");

        try
        {
            return Wait(operation);
        }
        finally
        {
            ComObject.Release(operation);
        }
    }

    public static void Check(int hr, string call)
    {
        if (hr < 0)
        {
            throw new InvalidOperationException($"{call} failed: 0x{hr:x8}");
        }
    }

    private static nint Wait(nint operation)
    {
        var getResult = (delegate* unmanaged<nint, int*, nint*, int>)ComObject.Slot(operation, Mmdevapi.GetActivateResultSlot);

        for (int poll = 0; poll < PollLimit; poll++)
        {
            int activated = 0;
            nint client = 0;
            int hr = getResult(operation, &activated, &client);

            if (hr != HResult.IllegalMethodCall)
            {
                Check(hr, "GetActivateResult");
                Check(activated, "process loopback activation");

                return client;
            }

            Thread.Sleep(PollMilliseconds);
        }

        throw new InvalidOperationException("process loopback activation did not complete");
    }

    /// <summary>The completion handler: IUnknown, IAgileObject and an ActivateCompleted that returns S_OK.</summary>
    private static nint Handler()
    {
        if (_handler != 0)
        {
            return _handler;
        }

        var vtable = (HandlerVtable*)NativeMemory.Alloc((nuint)sizeof(HandlerVtable));
        vtable->QueryInterface = &QueryInterface;
        vtable->AddRef = &Count;
        vtable->Release = &Count;
        vtable->ActivateCompleted = &ActivateCompleted;

        var handler = (nint*)NativeMemory.Alloc((nuint)sizeof(nint));
        *handler = (nint)vtable;
        _handler = (nint)handler;

        return _handler;
    }

    [UnmanagedCallersOnly]
    private static int QueryInterface(nint self, Guid* iid, nint* result)
    {
        if (*iid == Mmdevapi.Unknown || *iid == Mmdevapi.CompletionHandler || *iid == Mmdevapi.AgileObject)
        {
            *result = self;

            return HResult.Ok;
        }

        *result = 0;

        return HResult.NoInterface;
    }

    /// <summary>AddRef and Release: the handler lives as long as the process, so no count is kept.</summary>
    [UnmanagedCallersOnly]
    private static uint Count(nint self) => 1;

    [UnmanagedCallersOnly]
    private static int ActivateCompleted(nint self, nint operation) => HResult.Ok;

    /// <summary>AUDIOCLIENT_ACTIVATION_PARAMS with AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS.</summary>
    [StructLayout(LayoutKind.Sequential)]
    private struct ActivationParameters
    {
        public int Type;
        public int ProcessId;
        public int Mode;
    }

    /// <summary>A PROPVARIANT of type VT_BLOB.</summary>
    [StructLayout(LayoutKind.Sequential)]
    private struct BlobVariant
    {
        public ushort Type;
        public ushort Reserved1;
        public uint Reserved2;
        public uint Size;
        public ActivationParameters* Data;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct HandlerVtable
    {
        public delegate* unmanaged<nint, Guid*, nint*, int> QueryInterface;
        public delegate* unmanaged<nint, uint> AddRef;
        public delegate* unmanaged<nint, uint> Release;
        public delegate* unmanaged<nint, nint, int> ActivateCompleted;
    }
}
