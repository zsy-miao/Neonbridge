// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>The MMDevice API call and the WASAPI vtable slots the audio recorder uses (mmdeviceapi.h, audioclient.h).</summary>
internal static unsafe class Mmdevapi
{
    /// <summary>VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK: the device path that activates a process loopback client.</summary>
    public const string ProcessLoopbackDevice = "VAD\\Process_Loopback";

    /// <summary>AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK.</summary>
    public const int ProcessLoopbackActivation = 1;

    /// <summary>PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE.</summary>
    public const int IncludeProcessTree = 0;

    /// <summary>VT_BLOB: a PROPVARIANT holding a size and a pointer.</summary>
    public const ushort BlobVariant = 0x41;

    /// <summary>AUDCLNT_SHAREMODE_SHARED.</summary>
    public const int SharedMode = 0;

    /// <summary>AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM.</summary>
    public const uint LoopbackConvertFlags = 0x0002_0000 | 0x8000_0000;

    /// <summary>AUDCLNT_BUFFERFLAGS_SILENT.</summary>
    public const uint SilentBuffer = 2;

    public const int GetActivateResultSlot = 3;
    public const int InitializeSlot = 3;
    public const int StartSlot = 10;
    public const int GetServiceSlot = 14;
    public const int GetBufferSlot = 3;
    public const int ReleaseBufferSlot = 4;
    public const int GetNextPacketSizeSlot = 5;

    public static readonly Guid AudioClient = new("1CB9AD4C-DBFA-4c32-B178-C2F568A703B2");
    public static readonly Guid AudioCaptureClient = new("C8ADBD64-E71E-48a0-A4DE-185C395CD317");
    public static readonly Guid CompletionHandler = new("41D949AB-9862-444A-80F6-C261334DA5EB");
    public static readonly Guid AgileObject = new("94ea2b94-e9cc-49e0-c0ff-ee64ca8f5b90");
    public static readonly Guid Unknown = new("00000000-0000-0000-C000-000000000046");

    private const string Library = "mmdevapi";

    /// <summary>HRESULT ActivateAudioInterfaceAsync(path, riid, PROPVARIANT* params, handler, operation**).</summary>
    [DllImport(Library)]
    public static extern int ActivateAudioInterfaceAsync(char* path, Guid* iid, void* parameters, nint handler, nint* operation);
}
