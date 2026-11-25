// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Shims.D3D12X.Binding;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// With <c>--watch-transient</c>: the game range of every buffer UAV by the desktop index its record names (kept at
/// creation, <see cref="Note"/>), and, before each marked operation, the UAVs of the descriptor tables its user data
/// points at. A user dword that is a window pointer into the view records (from <see cref="DescriptorWindow.ViewBase"/>,
/// a multiple of the 32-byte record, not 0) is taken as a table; its first <see cref="DescriptorWindow.MinTableRecords"/>
/// records are read from the window as the program sees them, and each buffer UAV among them whose game range overlaps
/// the transient pool goes to <see cref="TransientPoolHits"/> with the record's V# words (stride, num_records,
/// OOB_SELECT in dword 3).
/// </summary>
internal static unsafe class TransientPoolViews
{
    private const uint MaxViews = DescriptorWindow.MaxViews;
    private const uint RecordBytes = DescriptorWindow.ViewRecordBytes;
    private const uint ViewEnd = DescriptorWindow.ViewBase + (MaxViews * RecordBytes);

    /// <summary>Bytes of a raw view's element.</summary>
    private const uint RawElement = 4;

    private static View* _views;

    /// <summary>A buffer UAV's range in game memory; Start 0 for none.</summary>
    [StructLayout(LayoutKind.Sequential)]
    private struct View
    {
        public ulong Start;
        public ulong Bytes;
    }

    /// <summary>Whether UAV creations are kept (the watch is on).</summary>
    public static bool Enabled => _views != null;

    /// <summary>Starts keeping UAV creations; call once, from the watch.</summary>
    public static void Enable()
    {
        _views = (View*)NativeMemory.AllocZeroed((nuint)(MaxViews * (uint)sizeof(View)));
    }

    /// <summary>
    /// A buffer UAV written to the record at <paramref name="handle"/> (title memory, dword 0 its desktop index), over a
    /// buffer at <paramref name="game"/>; <paramref name="firstElement"/> as the game's desc gives it.
    /// </summary>
    [HotPath]
    public static void Note(nint handle, ulong game, ulong firstElement, BufferRecord record)
    {
        uint desktop = *(uint*)handle;

        if (desktop >= MaxViews)
        {
            return;
        }

        uint element = record.Stride;

        if (element == 0)
        {
            element = RawElement;
        }

        ulong bytes = (ulong)record.Records * element;

        if (record.Stride == 0)
        {
            bytes = record.Records;
        }

        ulong start = 0;

        if (game != 0)
        {
            start = game + (firstElement * element);
        }

        _views[desktop] = new View { Start = start, Bytes = bytes };

        if (start != 0 && TransientPoolHits.InPool(start, bytes))
        {
            PoolStoreTrace.AddView(desktop);
        }
    }

    /// <summary>Once the pool is known: the views made before that which overlap it go to the store trace.</summary>
    public static void PoolKnown()
    {
        if (_views == null)
        {
            return;
        }

        for (uint desktop = 0; desktop < MaxViews; desktop++)
        {
            var view = _views[desktop];

            if (view.Start != 0 && TransientPoolHits.InPool(view.Start, view.Bytes))
            {
                PoolStoreTrace.AddView(desktop);
            }
        }
    }

    /// <summary>The game address a buffer UAV at <paramref name="desktop"/> starts at; 0 for none.</summary>
    public static ulong StartOf(uint desktop)
    {
        if (_views == null || desktop >= MaxViews)
        {
            return 0;
        }

        return _views[desktop].Start;
    }

    /// <summary>The table UAVs that overlap the pool, for marked operation <paramref name="index"/> of recording <paramref name="serial"/>.</summary>
    [HotPath]
    public static void Search(uint* data, uint serial, int index, nint pipeline, bool compute)
    {
        byte* window = ShimContext.Runtime.D3D12X.Bindings.Window.Data;

        if (_views == null || window == null)
        {
            return;
        }

        for (uint i = 0; i < CommandListBindings.UserDataDwords; i++)
        {
            uint pointer = data[i];

            if (pointer == DescriptorWindow.ViewBase || pointer >= ViewEnd || (pointer - DescriptorWindow.ViewBase) % RecordBytes != 0)
            {
                continue;
            }

            for (uint k = 0; k < DescriptorWindow.MinTableRecords && pointer + (k * RecordBytes) < ViewEnd; k++)
            {
                uint* record = (uint*)(window + pointer + (k * RecordBytes));
                uint desktop = record[DescriptorWindow.MirroredDesktopDword];

                if (desktop >= MaxViews)
                {
                    continue;
                }

                var view = _views[desktop];

                if (view.Start != 0 && TransientPoolHits.InPool(view.Start, view.Bytes))
                {
                    TransientPoolHits.AddTable(serial, index, pipeline, compute, i, (int)k, view.Start, view.Bytes, record);
                }
            }
        }
    }
}
