// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// Game descriptor indices to desktop heap indices, for a game heap larger than the desktop one. The game heap is cut
/// into pages of <see cref="PageSize"/> descriptors; a page gets the next free desktop page the first time a
/// descriptor in it is written, so any set of up to <see cref="Capacity"/> touched pages fits, wherever they lie.
/// Indices within a page stay contiguous. Lookups of mapped pages take no lock.
/// </summary>
internal sealed class DescriptorRemap
{
    /// <summary>Descriptors per page (log2).</summary>
    public const int PageShift = 12;
    public const uint PageSize = 1u << PageShift;

    /// <summary>Returned for a descriptor whose page found no free desktop page.</summary>
    public const uint Unmapped = uint.MaxValue;

    private const uint PageMask = PageSize - 1;

    /// <summary>Desktop index of each game page's first descriptor; 0 until the page is mapped.</summary>
    private readonly uint[] _pages;
    private readonly object _gate = new();
    private readonly uint _first;
    private uint _mapped;

    /// <param name="first">The first desktop index the game's descriptors may take (never 0).</param>
    /// <param name="desktopCount">The desktop heap's size.</param>
    /// <param name="gameCount">The game heap's size.</param>
    public DescriptorRemap(uint first, uint desktopCount, uint gameCount)
    {
        _first = first;
        _pages = new uint[(gameCount + PageMask) >> PageShift];
        Capacity = desktopCount > first ? (desktopCount - first) >> PageShift : 0;
    }

    /// <summary>Desktop pages available to the game.</summary>
    public uint Capacity { get; }

    /// <summary>Game pages mapped so far.</summary>
    public uint MappedPages => Volatile.Read(ref _mapped);

    /// <summary>The desktop index of game descriptor <paramref name="index"/>, mapping its page on first use; <see cref="Unmapped"/> when no desktop page is left.</summary>
    [HotPath]
    public uint Map(uint index)
    {
        uint page = index >> PageShift;

        if (page >= (uint)_pages.Length)
        {
            return Unmapped;
        }

        uint start = Volatile.Read(ref _pages[page]);

        if (start == 0)
        {
            start = MapPage(page);
        }

        if (start == 0)
        {
            return Unmapped;
        }

        return start + (index & PageMask);
    }

    /// <summary>Descriptors from <paramref name="index"/> to the end of its page: the longest run contiguous on the desktop.</summary>
    [HotPath]
    public static uint RunLength(uint index)
    {
        return PageSize - (index & PageMask);
    }

    /// <summary>
    /// Takes <paramref name="pages"/> consecutive desktop pages that no game page maps to; returns the desktop index of
    /// the first, or <see cref="Unmapped"/> when too few are left.
    /// </summary>
    public uint Reserve(uint pages)
    {
        lock (_gate)
        {
            if (_mapped + pages > Capacity)
            {
                return Unmapped;
            }

            uint start = _first + (_mapped << PageShift);
            Volatile.Write(ref _mapped, _mapped + pages);

            return start;
        }
    }

    /// <summary>First use of a page: takes the next desktop page; 0 when none is left.</summary>
    private uint MapPage(uint page)
    {
        lock (_gate)
        {
            uint start = _pages[page];

            if (start != 0 || _mapped >= Capacity)
            {
                return start;
            }

            start = _first + (_mapped << PageShift);
            Volatile.Write(ref _pages[page], start);
            Volatile.Write(ref _mapped, _mapped + 1);

            return start;
        }
    }
}
