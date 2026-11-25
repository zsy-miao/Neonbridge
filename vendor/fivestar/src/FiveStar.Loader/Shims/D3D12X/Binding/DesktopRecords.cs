// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// The V# fields of buffer views written into desktop heaps (heaps the game made that are not in title memory), by
/// desktop CPU handle. Their descriptors carry no record; a copy into one of the game's heaps writes the fields the
/// view's record would have held (<see cref="ShaderVisibleHeap.SetFields"/>), so a program reading the copy as a V#
/// finds its stride, records, format and, where proven, its console base. Before, such copies (CopyInto) wrote no
/// fields: MS_VS_Transform_PositionOnly read its per-meshlet V# from one as {heap index, 0, 0, 0} and every typed load
/// through it was out of range (w25).
/// </summary>
internal static class DesktopRecords
{
    /// <summary>Copies that carried fields logged, the first ones.</summary>
    private const int CarriedLogged = 16;

    private static readonly ConcurrentDictionary<nint, BufferRecord> Records = new();

    private static int _carried;

    /// <summary>Whether any desktop-heap buffer view is known: copies skip the lookups otherwise.</summary>
    public static bool Any => !Records.IsEmpty;

    /// <summary>Keeps (or, for a view with no buffer fields, forgets) the fields of the descriptor at <paramref name="handle"/>.</summary>
    [HotPath]
    public static void Note(nint handle, BufferRecord buffer)
    {
        if (buffer.IsBuffer)
        {
            Records[handle] = buffer;
        }
        else if (!Records.IsEmpty)
        {
            Records.TryRemove(handle, out _);
        }
    }

    /// <summary>The fields of the buffer view at <paramref name="handle"/>; false when none is known there.</summary>
    [HotPath]
    public static bool TryFind(nint handle, out BufferRecord buffer)
    {
        return Records.TryGetValue(handle, out buffer);
    }

    /// <summary>Logs the first copies that carried fields: the game index (window record index) and the fields.</summary>
    public static void NoteCarried(uint index, BufferRecord buffer)
    {
        int count = Interlocked.Increment(ref _carried);

        if (count <= CarriedLogged)
        {
            ShimContext.Runtime.D3D12X.Warnings.Warn($"record-carried-{count}", $"[d3d12_xs] a desktop-heap buffer view copied in at game index {index} carries its fields: base 0x{buffer.Base:x}, stride {buffer.Stride}, records {buffer.Records}, format 0x{buffer.Format:x}");
        }
    }
}
