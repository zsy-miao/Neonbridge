// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XSpeechSynthesizer.h: speech synthesis is reported unavailable (XGameRuntimeIsFeatureAvailable, no installed
/// voices), so XSpeechSynthesizerCreate fails with HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED) and no synthesizer or
/// stream handle ever exists; the functions taking one return E_INVALIDARG.
/// </summary>
internal static unsafe partial class XSpeechSynthesizerShim
{
    /// <summary>HRESULT XSpeechSynthesizerCreate(XSpeechSynthesizerHandle* speechSynthesizer).</summary>
    [Shim(GdkModule.Name, "XSpeechSynthesizerCreate")]
    [UnmanagedCallersOnly]
    private static int XSpeechSynthesizerCreate(nint* synthesizer)
    {
        if (synthesizer == null)
        {
            return HResult.Pointer;
        }

        *synthesizer = 0;

        return HResult.NotSupported;
    }

    /// <summary>HRESULT XSpeechSynthesizerCloseHandle(XSpeechSynthesizerHandle): null is allowed.</summary>
    [Shim(GdkModule.Name, "XSpeechSynthesizerCloseHandle")]
    [UnmanagedCallersOnly]
    private static int XSpeechSynthesizerCloseHandle(nint synthesizer)
    {
        if (synthesizer == 0)
        {
            return HResult.Ok;
        }

        return HResult.InvalidArgument;
    }

    /// <summary>HRESULT XSpeechSynthesizerSetDefaultVoice(handle) and XSpeechSynthesizerCloseStreamHandle(handle).</summary>
    [Shim(GdkModule.Name, "XSpeechSynthesizerSetDefaultVoice")]
    [Shim(GdkModule.Name, "XSpeechSynthesizerCloseStreamHandle")]
    [UnmanagedCallersOnly]
    private static int NoHandle(nint handle)
    {
        return HResult.InvalidArgument;
    }

    /// <summary>HRESULT XSpeechSynthesizerSetCustomVoice(XSpeechSynthesizerHandle, const char* voiceId).</summary>
    [Shim(GdkModule.Name, "XSpeechSynthesizerSetCustomVoice")]
    [UnmanagedCallersOnly]
    private static int XSpeechSynthesizerSetCustomVoice(nint synthesizer, byte* voiceId)
    {
        return HResult.InvalidArgument;
    }

    /// <summary>HRESULT XSpeechSynthesizerCreateStreamFrom{Text,Ssml}(XSpeechSynthesizerHandle, const char*, XSpeechSynthesizerStreamHandle*).</summary>
    [Shim(GdkModule.Name, "XSpeechSynthesizerCreateStreamFromText")]
    [Shim(GdkModule.Name, "XSpeechSynthesizerCreateStreamFromSsml")]
    [UnmanagedCallersOnly]
    private static int CreateStream(nint synthesizer, byte* text, nint* stream)
    {
        if (stream != null)
        {
            *stream = 0;
        }

        return HResult.InvalidArgument;
    }

    /// <summary>HRESULT XSpeechSynthesizerGetStreamDataSize(XSpeechSynthesizerStreamHandle, size_t* bufferSize).</summary>
    [Shim(GdkModule.Name, "XSpeechSynthesizerGetStreamDataSize")]
    [UnmanagedCallersOnly]
    private static int XSpeechSynthesizerGetStreamDataSize(nint stream, nuint* size)
    {
        if (size != null)
        {
            *size = 0;
        }

        return HResult.InvalidArgument;
    }

    /// <summary>HRESULT XSpeechSynthesizerGetStreamData(XSpeechSynthesizerStreamHandle, size_t bufferSize, void* buffer, size_t* bufferUsed).</summary>
    [Shim(GdkModule.Name, "XSpeechSynthesizerGetStreamData")]
    [UnmanagedCallersOnly]
    private static int XSpeechSynthesizerGetStreamData(nint stream, nuint size, nint buffer, nuint* used)
    {
        if (used != null)
        {
            *used = 0;
        }

        return HResult.InvalidArgument;
    }
}
