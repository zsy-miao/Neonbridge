// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XPackageMountWithUiAsync: mounts the package; the result is an XPackageMountHandle.</summary>
internal sealed class MountOperation : ManagedAsyncOperation
{
    private readonly PackageService _packages;
    private readonly string? _id;
    private nint _mount;

    public MountOperation(PackageService packages, string? id)
    {
        _packages = packages;
        _id = id;
    }

    public override unsafe AsyncWorkResult Work()
    {
        int hr = _packages.Mount(_id, out _mount);

        return new AsyncWorkResult(hr, (nuint)sizeof(nint));
    }

    public override unsafe int Write(byte* buffer, nuint size)
    {
        *(nint*)buffer = _mount;

        return HResult.Ok;
    }
}
