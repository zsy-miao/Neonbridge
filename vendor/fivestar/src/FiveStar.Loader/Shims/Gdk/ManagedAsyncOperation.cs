// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// A shim-side *Async operation run through XAsync: <see cref="Work"/> runs on the work port and returns the HRESULT
/// and result size; <see cref="Write"/> lays the result out in the caller's buffer, which may hold pointers into itself.
/// </summary>
internal abstract class ManagedAsyncOperation
{
    public abstract AsyncWorkResult Work();

    public virtual unsafe int Write(byte* buffer, nuint size)
    {
        return HResult.Ok;
    }
}
