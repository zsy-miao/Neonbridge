// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims;

/// <summary>
/// The pages of one multi-page allocation, consecutive in one section. Which page backs which of the group's frames is
/// decided when the frame is first mapped: the page at the mapped address's offset from the group's origin (the start
/// of the region the first mapping went into), so a range the game maps from the group in any order of pieces is in
/// section order. A frame freed or located before it is mapped takes the first free page.
/// </summary>
internal sealed class XMemPageGroup
{
    private readonly bool[] _taken;
    private ulong _cursor;
    private nint _origin;

    public XMemPageGroup(ulong firstPage, ulong count)
    {
        FirstPage = firstPage;
        Count = count;
        _taken = new bool[count];
    }

    public ulong FirstPage { get; }

    public ulong Count { get; }

    /// <summary>
    /// Takes a page for a frame mapped at <paramref name="address"/> inside a region starting at
    /// <paramref name="origin"/> (0, 0 for a frame that is not being mapped). Called once per frame, so a page is free.
    /// </summary>
    public ulong Take(nint address, nint origin)
    {
        if (address != 0 && _origin == 0)
        {
            _origin = origin;
        }

        ulong index = (ulong)(address - _origin) / XMemPageStore.PageSize;

        if (address != 0 && address >= _origin && index < Count && !_taken[index])
        {
            _taken[index] = true;

            return FirstPage + index;
        }

        while (_taken[_cursor])
        {
            _cursor++;
        }

        _taken[_cursor] = true;

        return FirstPage + _cursor;
    }
}
