// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XAccessibility.h and XSpeechSynthesizer.h offline. High contrast follows the Windows setting; closed captions use
/// the console defaults (white text on black, normal size) and remember what the game last enabled; no speech voices
/// are installed, so titles skip text-to-speech.
/// </summary>
internal sealed unsafe class AccessibilityService
{
    // XClosedCaptionProperties { XColor background, font, window; edge attribute; font style; float scale; bool enabled; }.
    public const int CaptionSize = 28;
    public const int CaptionEnabledOffset = 24;
    private const int CaptionBackgroundOffset = 0;
    private const int CaptionFontOffset = 4;
    private const int CaptionScaleOffset = 20;

    /// <summary>XColor bytes A, R, G, B: opaque black and opaque white.</summary>
    private const uint OpaqueBlack = 0x000000FF;
    private const uint OpaqueWhite = 0xFFFFFFFF;
    private const float NormalScale = 1.0f;

    // XHighContrastMode.
    public const uint HighContrastOff = 0;
    public const uint HighContrastOther = 3;

    // SPI_GETHIGHCONTRAST with HIGHCONTRASTW { UINT cbSize; DWORD dwFlags; LPWSTR lpszDefaultScheme; } and HCF_HIGHCONTRASTON.
    private const uint GetHighContrast = 0x0042;
    private const int HighContrastSize = 16;
    private const int HighContrastFlagsOffset = 4;
    private const uint HighContrastOn = 0x1;

    private int _captionsEnabled;

    public bool CaptionsEnabled => Volatile.Read(ref _captionsEnabled) != 0;

    public int GetClosedCaptionProperties(byte* properties)
    {
        if (properties == null)
        {
            return HResult.Pointer;
        }

        new Span<byte>(properties, CaptionSize).Clear();
        *(uint*)(properties + CaptionBackgroundOffset) = OpaqueBlack;
        *(uint*)(properties + CaptionFontOffset) = OpaqueWhite;
        *(float*)(properties + CaptionScaleOffset) = NormalScale;

        if (CaptionsEnabled)
        {
            properties[CaptionEnabledOffset] = GdkModule.True;
        }

        return HResult.Ok;
    }

    public int SetClosedCaptionsEnabled(bool enabled)
    {
        int value = 0;

        if (enabled)
        {
            value = 1;
        }

        Volatile.Write(ref _captionsEnabled, value);

        return HResult.Ok;
    }

    public static int GetHighContrastMode(uint* mode)
    {
        if (mode == null)
        {
            return HResult.Pointer;
        }

        byte* settings = stackalloc byte[HighContrastSize];
        new Span<byte>(settings, HighContrastSize).Clear();
        *(uint*)settings = HighContrastSize;
        *mode = HighContrastOff;

        if (User32.SystemParametersInfoW(GetHighContrast, HighContrastSize, settings, 0) != 0
            && (*(uint*)(settings + HighContrastFlagsOffset) & HighContrastOn) != 0)
        {
            *mode = HighContrastOther;
        }

        return HResult.Ok;
    }

    /// <summary>No voices: the callback is never called.</summary>
    public static int EnumerateInstalledVoices(nint callback)
    {
        if (callback == 0)
        {
            return HResult.InvalidArgument;
        }

        return HResult.Ok;
    }
}
