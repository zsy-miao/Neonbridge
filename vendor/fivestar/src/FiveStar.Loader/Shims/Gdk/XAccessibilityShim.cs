// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XAccessibility.h entry points and the voice list of XSpeechSynthesizer.h; the work is in
/// <see cref="AccessibilityService"/>. Synthesis is in <see cref="XSpeechSynthesizerShim"/>; speech-to-text stays in
/// the not-implemented catalog.
/// </summary>
internal static unsafe partial class XAccessibilityShim
{
    [Shim(GdkModule.Name, "XClosedCaptionGetProperties")]
    [UnmanagedCallersOnly]
    private static int XClosedCaptionGetProperties(byte* properties)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Accessibility.GetClosedCaptionProperties(properties);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XAccessibilityShim.XClosedCaptionGetProperties");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XClosedCaptionSetEnabled")]
    [UnmanagedCallersOnly]
    private static int XClosedCaptionSetEnabled(byte enabled)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Accessibility.SetClosedCaptionsEnabled(enabled != 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XAccessibilityShim.XClosedCaptionSetEnabled");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XHighContrastGetMode")]
    [UnmanagedCallersOnly]
    private static int XHighContrastGetMode(uint* mode)
    {
        try
        {
            return AccessibilityService.GetHighContrastMode(mode);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XAccessibilityShim.XHighContrastGetMode");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XSpeechSynthesizerEnumerateInstalledVoices(void* context, XSpeechSynthesizerInstalledVoicesCallback*).</summary>
    [Shim(GdkModule.Name, "XSpeechSynthesizerEnumerateInstalledVoices")]
    [UnmanagedCallersOnly]
    private static int XSpeechSynthesizerEnumerateInstalledVoices(nint context, nint callback)
    {
        try
        {
            return AccessibilityService.EnumerateInstalledVoices(callback);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XAccessibilityShim.XSpeechSynthesizerEnumerateInstalledVoices");

            return HResult.Fail;
        }
    }
}
