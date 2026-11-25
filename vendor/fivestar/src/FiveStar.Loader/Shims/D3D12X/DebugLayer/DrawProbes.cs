// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Formats.Images;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.D3D12X.Resources;
using Vortice.Direct3D12;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// Debug readback of a render target right after a draw, inside the game's own command list: the target goes from the
/// state it is in on that list to COPY_SOURCE, is copied into a readback buffer and goes back, so the copy sees exactly
/// what the draw left. <see cref="Report"/> maps the buffers once the GPU finished (the caller waited) and logs what
/// each holds. The game executes some closed lists more than once, so a buffer is released only at a later report
/// after the list it was copied on has been reset (the GPU is idle then and no execution can name it any more): released
/// when its own copy completed, a buffer that a second execution of the list still writes would be freed memory on the
/// GPU. Buffers alive, reported or not, stay under <see cref="LiveBytesCap"/>: a probe past it is skipped (in boot w20
/// the probes of --d3d12-dump-targets read back about 2.3 GB of targets every 30 s). The report reads each mapped
/// buffer row by row (<see cref="ReadbackPixels.Summarize(ReadOnlySpan{byte}, int, int, int, int, Span{byte})"/>):
/// no copy of the target on the managed heap, and a pooled image only for a PNG.
/// </summary>
internal sealed unsafe class DrawProbes
{
    /// <summary>The most bytes of readback buffers alive at once.</summary>
    public const long LiveBytesCap = 512L << 20;

    private const int Channels = 4;
    private const long SkipsLogged = 100;
    private const long BytesPerMegabyte = 1L << 20;

    private readonly object _gate = new();
    private readonly List<Probe> _pending = [];
    private readonly List<Probe> _reported = [];
    private readonly LoaderLog _log;
    private long _liveBytes;
    private long _skipped;

    public DrawProbes(LoaderLog log)
    {
        _log = log;
    }

    /// <summary>Records the copy of <paramref name="target"/> (in <paramref name="state"/> on the list) into the desktop list <paramref name="list"/>.</summary>
    /// <paramref name="key"/> and <paramref name="generation"/> name the game's list and how often it was reset; with
    /// <paramref name="png"/> the report also writes the image there.
    public void Record(nint device, nint list, nint target, int state, string label, nint key, long generation, string? png = null)
    {
        Marshal.AddRef(device);
        using var desktop = new ID3D12Device(device);
        Marshal.AddRef(target);
        var texture = new ID3D12Resource(target);
        var description = texture.Description;

        // A multisampled or non-2D target cannot be copied to a buffer this way.
        if (description.Dimension != ResourceDimension.Texture2D || description.SampleDescription.Count > 1)
        {
            texture.Dispose();

            return;
        }

        var footprints = new PlacedSubresourceFootPrint[1];
        var rows = new uint[1];
        var rowBytes = new ulong[1];
        desktop.GetCopyableFootprints(description, 0, 1, 0, footprints, rows, rowBytes, out ulong total);

        if (!Admit((long)total, label))
        {
            texture.Dispose();

            return;
        }

        var buffer = desktop.CreateCommittedResource(new HeapProperties(HeapType.Readback), HeapFlags.None, Vortice.Direct3D12.ResourceDescription.Buffer(total), ResourceStates.CopyDest);

        Marshal.AddRef(list);
        using var commands = new ID3D12GraphicsCommandList(list);
        var resting = (ResourceStates)state;

        commands.ResourceBarrierTransition(texture, resting, ResourceStates.CopySource);
        commands.CopyTextureRegion(new TextureCopyLocation(buffer, footprints[0]), 0, 0, 0, new TextureCopyLocation(texture, 0), null);
        commands.ResourceBarrierTransition(texture, ResourceStates.CopySource, resting);
        texture.Dispose();

        lock (_gate)
        {
            _pending.Add(new Probe(label, buffer, (long)total, footprints[0], rows[0], (int)description.Width, (int)description.Format, key, generation, png));
        }
    }

    /// <summary>
    /// Logs what every recorded probe holds; the GPU has finished the lists that recorded them. Buffers of lists reset
    /// since their probe (<paramref name="generation"/> of a list key) are released.
    /// </summary>
    public void Report(Func<nint, long> generation)
    {
        lock (_gate)
        {
            for (int i = _reported.Count - 1; i >= 0; i--)
            {
                if (generation(_reported[i].List) != _reported[i].Generation)
                {
                    _reported[i].Buffer.Dispose();
                    _liveBytes -= _reported[i].Bytes;
                    _reported.RemoveAt(i);
                }
            }

            foreach (var probe in _pending)
            {
                _log.Info($"  [draws] after {probe.Label}: {Summary(probe)}");
                _reported.Add(probe);
            }

            _pending.Clear();
        }
    }

    /// <summary>Whether a buffer of <paramref name="bytes"/> fits under <see cref="LiveBytesCap"/> beside <paramref name="live"/>.</summary>
    public static bool Fits(long live, long bytes)
    {
        return live + bytes <= LiveBytesCap;
    }

    /// <summary>Counts a new buffer in, or the skip when it does not fit.</summary>
    private bool Admit(long bytes, string label)
    {
        lock (_gate)
        {
            if (Fits(_liveBytes, bytes))
            {
                _liveBytes += bytes;

                return true;
            }

            _skipped++;

            if (_skipped % SkipsLogged == 1)
            {
                _log.Info($"  [draws] {label}: skipped, {_liveBytes / BytesPerMegabyte} MB of readback buffers alive ({_skipped} skipped so far)");
            }

            return false;
        }
    }

    private static string Summary(Probe probe)
    {
        void* data;
        probe.Buffer.Map(0, null, &data).CheckError();
        int imageBytes = probe.Width * (int)probe.Rows * Channels;
        byte[]? image = null;

        try
        {
            if (probe.Png is not null)
            {
                image = System.Buffers.ArrayPool<byte>.Shared.Rent(imageBytes);
            }

            var source = new ReadOnlySpan<byte>(data, checked((int)probe.Bytes));
            var target = image is null ? Span<byte>.Empty : image.AsSpan(0, imageBytes);
            var summary = ReadbackPixels.Summarize(source, (int)probe.Footprint.Footprint.RowPitch, probe.Width, (int)probe.Rows, probe.Format, target);

            if (summary is null)
            {
                return $"format {probe.Format} not converted";
            }

            if (probe.Png is not null)
            {
                PngWriter.Write(probe.Png, probe.Width, (int)probe.Rows, target);
            }

            return summary;
        }
        finally
        {
            probe.Buffer.Unmap(0);

            if (image is not null)
            {
                System.Buffers.ArrayPool<byte>.Shared.Return(image);
            }
        }
    }

    /// <summary>One recorded copy: its buffer and size, the copied footprint, and the game's list and generation it was copied on.</summary>
    private sealed record Probe(string Label, ID3D12Resource Buffer, long Bytes, PlacedSubresourceFootPrint Footprint, uint Rows, int Width, int Format, nint List, long Generation, string? Png);
}
