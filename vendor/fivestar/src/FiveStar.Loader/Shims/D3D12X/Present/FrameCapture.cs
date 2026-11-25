// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Images;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.D3D12X.Resources;
using FiveStar.Loader.Shims.Input;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// Debug readback of what the game presents (<c>--capture-dir</c>, <c>--capture-presents</c>): at the chosen present
/// numbers, and at the next present after <see cref="RequestNext"/>, the plane is read back after the presenting queue
/// went idle, summarized in the log and, with a folder,
/// written as <c>present-N.png</c>. Only the plane's own texels are read, never the window or the desktop. Other
/// debug paths save textures through <see cref="Save"/>.
/// </summary>
internal sealed class FrameCapture : IDisposable
{
    /// <summary>No requested present: present numbers start at 1.</summary>
    private const long NoPresent = 0;

    private readonly HashSet<long> _presents;
    private readonly TextureReadback _readback = new();
    private readonly LoaderLog _log;
    private int _requested;
    private long _requestedPresent;

    public FrameCapture(string? directory, IEnumerable<long> presents, LoaderLog log)
    {
        Directory = directory;
        _presents = [.. presents];
        _log = log;
    }

    /// <summary>Where PNG files go; null logs summaries only.</summary>
    public string? Directory { get; }

    /// <summary>Whether present number <paramref name="present"/> (1-based) is captured.</summary>
    [HotPath]
    public bool Wants(long present)
    {
        if (Volatile.Read(ref _requested) != 0)
        {
            Interlocked.CompareExchange(ref _requestedPresent, present, NoPresent);
            Volatile.Write(ref _requested, 0);
        }

        return _presents.Contains(present) || present == Volatile.Read(ref _requestedPresent);
    }

    /// <summary>Captures the next present as well (an input script's <c>capture</c> line); any thread.</summary>
    public void RequestNext()
    {
        Volatile.Write(ref _requestedPresent, NoPresent);
        Volatile.Write(ref _requested, 1);
    }

    /// <summary>Reads back <paramref name="resource"/> resting in <paramref name="state"/>, logs a summary and writes <paramref name="name"/>.png.</summary>
    public void Save(nint device, nint resource, int state, string name)
    {
        ReadbackImage? image;

        try
        {
            image = _readback.Read(device, resource, state);
        }
        catch (Exception exception) when (exception is InvalidOperationException or TimeoutException or SharpGen.Runtime.SharpGenException)
        {
            _log.Warn($"[capture] {name}: readback failed: {exception.Message}");

            return;
        }

        if (image is null)
        {
            _log.Info($"  [capture] {name}: not a single-sample 2D texture, skipped");

            return;
        }

        var rgba = ReadbackPixels.ToRgba(image);

        if (rgba is null)
        {
            _log.Info($"  [capture] {name}: {image.Width}x{image.Height} format {image.Format} not converted");

            return;
        }

        _log.Info($"  [capture] {name}: {image.Width}x{image.Height} format {image.Format}: {ReadbackPixels.Summarize(rgba, image.Width, image.Height)}");
        _log.Info($"  [capture] {name}: screen signature {ScreenSignature.Of(rgba, image.Width, image.Height)}");
        Write(name, image, rgba);
    }

    public void Dispose()
    {
        _readback.Dispose();
    }

    private void Write(string name, ReadbackImage image, byte[] rgba)
    {
        if (Directory is null)
        {
            return;
        }

        System.IO.Directory.CreateDirectory(Directory);
        string path = Path.Combine(Directory, name + ".png");
        PngWriter.Write(path, image.Width, image.Height, rgba);
        _log.Info($"  [capture] wrote {path}");
    }
}
