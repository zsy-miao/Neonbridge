// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// An XGameUI call whose system UI the player closed at once: S_OK with no result, S_OK with one uint32_t (the
/// message dialog's cancel button), or E_ABORT (text entry and web sign-in, which need an answer).
/// </summary>
internal sealed class DismissedUiOperation : ManagedAsyncOperation
{
    private readonly int _result;
    private readonly bool _hasValue;
    private readonly uint _value;

    private DismissedUiOperation(int result, bool hasValue, uint value)
    {
        _result = result;
        _hasValue = hasValue;
        _value = value;
    }

    public static DismissedUiOperation Closed()
    {
        return new DismissedUiOperation(HResult.Ok, false, 0);
    }

    public static DismissedUiOperation Canceled()
    {
        return new DismissedUiOperation(HResult.Abort, false, 0);
    }

    public static DismissedUiOperation WithValue(uint value)
    {
        return new DismissedUiOperation(HResult.Ok, true, value);
    }

    public override AsyncWorkResult Work()
    {
        nuint size = 0;

        if (_hasValue)
        {
            size = sizeof(uint);
        }

        return new AsyncWorkResult(_result, size);
    }

    public override unsafe int Write(byte* buffer, nuint size)
    {
        *(uint*)buffer = _value;

        return HResult.Ok;
    }
}
