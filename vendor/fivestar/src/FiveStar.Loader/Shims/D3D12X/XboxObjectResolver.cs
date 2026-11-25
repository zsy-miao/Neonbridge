// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.Ffx;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The desktop objects behind the wrappers <see cref="XboxObjects"/> hands out, for middleware bridges. A pointer
/// that is not a wrapper is already a desktop object and passes through.
/// </summary>
internal sealed class XboxObjectResolver : IDesktopObjectResolver
{
    private readonly XboxObjects _objects;

    public XboxObjectResolver(XboxObjects objects)
    {
        _objects = objects;
    }

    [HotPath]
    public nint Device(nint xboxDevice)
    {
        return _objects.Unwrap(xboxDevice);
    }

    [HotPath]
    public nint CommandList(nint xboxCommandList)
    {
        return _objects.Unwrap(xboxCommandList);
    }

    [HotPath]
    public nint Resource(nint xboxResource)
    {
        return _objects.Unwrap(xboxResource);
    }
}
