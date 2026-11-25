// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XDisplay.h on the PC display. XDisplayTryEnableHdrMode cannot switch a PC monitor, so it reports what Windows
/// runs: Enabled with the display's luminance when the desktop is HDR10, else Disabled. A timeout deferral holds a
/// Windows "display required" power request, so the screen does not dim or sleep while any deferral is open.
/// </summary>
internal sealed unsafe class DisplayService : IDisposable
{
    // XDisplayHdrModeResult.
    public const uint HdrEnabled = 1;
    public const uint HdrDisabled = 2;

    /// <summary>XDisplayHdrModeInfo: minToneMapLuminance, maxToneMapLuminance, maxFullFrameToneMapLuminance.</summary>
    public const int HdrInfoFloats = 3;

    private const int InfoMinSlot = 0;
    private const int InfoMaxSlot = 1;
    private const int InfoFullFrameSlot = 2;

    /// <summary>POWER_REQUEST_TYPE PowerRequestDisplayRequired.</summary>
    private const int DisplayRequired = 0;

    // REASON_CONTEXT { ULONG Version; DWORD Flags; union { ...; LPWSTR SimpleReasonString; } }: 32 bytes on x64.
    private const int ReasonContextSize = 32;
    private const int ReasonFlagsOffset = 4;
    private const int ReasonStringOffset = 8;
    private const uint ReasonSimpleString = 1;
    private const string Reason = "XDisplayAcquireTimeoutDeferral";

    private static readonly nint InvalidHandle = -1;

    private readonly HandleTable<object> _deferrals = new("XDISPDEF");
    private readonly object _gate = new();
    private readonly NativeBoundary _boundary;
    private nint _request;
    private int _active;
    private int _reported;

    public DisplayService(NativeBoundary boundary)
    {
        _boundary = boundary;
    }

    public int ActiveDeferrals => Volatile.Read(ref _active);

    /// <summary>XDisplayHdrModeResult XDisplayTryEnableHdrMode(XDisplayHdrModePreference, XDisplayHdrModeInfo*).</summary>
    public uint TryEnableHdr(uint preference, float* info)
    {
        var display = DisplayProbe.Primary();
        bool enabled = display is not null && display.Value.Enabled;

        if (info != null)
        {
            new Span<float>(info, HdrInfoFloats).Clear();
        }

        if (Interlocked.Exchange(ref _reported, 1) == 0)
        {
            _boundary.Info($"XDisplayTryEnableHdrMode(preference {preference}) -> PC display {Describe(display)}");
        }

        if (!enabled)
        {
            return HdrDisabled;
        }

        if (info != null)
        {
            info[InfoMinSlot] = display!.Value.MinLuminance;
            info[InfoMaxSlot] = display.Value.MaxLuminance;
            info[InfoFullFrameSlot] = display.Value.MaxFullFrameLuminance;
        }

        return HdrEnabled;
    }

    public int AcquireDeferral(nint* handle)
    {
        if (handle == null)
        {
            return HResult.Pointer;
        }

        lock (_gate)
        {
            if (_active == 0)
            {
                Hold(true);
            }

            _active++;
        }

        *handle = _deferrals.Add(new object());

        return HResult.Ok;
    }

    public void CloseDeferral(nint handle)
    {
        if (!_deferrals.Remove(handle))
        {
            return;
        }

        lock (_gate)
        {
            _active--;

            if (_active == 0)
            {
                Hold(false);
            }
        }
    }

    public void Dispose()
    {
        lock (_gate)
        {
            if (_request == 0 || _request == InvalidHandle)
            {
                return;
            }

            if (_active > 0)
            {
                _ = Kernel32.PowerClearRequest(_request, DisplayRequired);
            }

            _ = Kernel32.CloseHandle(_request);
            _request = 0;
        }
    }

    private static string Describe(HdrDisplay? display)
    {
        if (display is null)
        {
            return "unknown (no DXGI output)";
        }

        var value = display.Value;
        string mode = "SDR";

        if (value.Enabled)
        {
            mode = "HDR10";
        }

        return $"{mode}, {value.MinLuminance}..{value.MaxLuminance} nits, full frame {value.MaxFullFrameLuminance}";
    }

    /// <summary>Sets or clears the power request; created on first use. Called under the gate.</summary>
    private void Hold(bool on)
    {
        if (_request == 0)
        {
            _request = CreateRequest();
        }

        if (_request == InvalidHandle)
        {
            return;
        }

        if (!on)
        {
            _ = Kernel32.PowerClearRequest(_request, DisplayRequired);

            return;
        }

        if (Kernel32.PowerSetRequest(_request, DisplayRequired) == 0)
        {
            _boundary.Warn("PowerSetRequest failed; the display may still time out");
        }
    }

    private nint CreateRequest()
    {
        byte* context = stackalloc byte[ReasonContextSize];
        new Span<byte>(context, ReasonContextSize).Clear();
        *(uint*)(context + ReasonFlagsOffset) = ReasonSimpleString;
        nint request;

        fixed (char* reason = Reason)
        {
            *(char**)(context + ReasonStringOffset) = reason;
            request = Kernel32.PowerCreateRequest(context);
        }

        if (request == InvalidHandle)
        {
            _boundary.Warn("PowerCreateRequest failed; display timeout deferrals have no effect");
        }

        return request;
    }
}
