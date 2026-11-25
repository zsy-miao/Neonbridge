// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.D3D12X.Resources;
using FiveStar.Loader.Shims.Input;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// The signatures an input script's <c>screen</c> and <c>learn</c> lines ask for: after <see cref="Request"/>, the
/// next present's plane is read back (once the presenting queue is idle), shrunk to a <see cref="ScreenSignature"/>
/// and handed to the deliver callback. Nothing is read while no one asks, and the frame itself is not kept.
/// </summary>
internal sealed class ScreenSampler : IDisposable
{
    private readonly TextureReadback _readback = new();
    private readonly Action<ScreenSignature> _deliver;
    private readonly LoaderLog _log;
    private int _requested;

    public ScreenSampler(Action<ScreenSignature> deliver, LoaderLog log)
    {
        _deliver = deliver;
        _log = log;
    }

    /// <summary>Whether a signature was asked for since the last one.</summary>
    public bool Due => Volatile.Read(ref _requested) != 0;

    /// <summary>Asks for the next present's signature; any thread.</summary>
    public void Request()
    {
        Volatile.Write(ref _requested, 1);
    }

    /// <summary>Reads back <paramref name="resource"/> resting in <paramref name="state"/> and delivers its signature.</summary>
    public void Sample(nint device, nint resource, int state)
    {
        Volatile.Write(ref _requested, 0);
        ReadbackImage? image;

        try
        {
            image = _readback.Read(device, resource, state);
        }
        catch (Exception exception) when (exception is InvalidOperationException or TimeoutException or SharpGen.Runtime.SharpGenException)
        {
            _log.Warn($"[input] screen readback failed: {exception.Message}");

            return;
        }

        var rgba = image is null ? null : ReadbackPixels.ToRgba(image);

        if (rgba is null)
        {
            return;
        }

        _deliver(ScreenSignature.Of(rgba, image!.Width, image.Height));
    }

    public void Dispose()
    {
        _readback.Dispose();
    }
}
