// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Translates game addresses to desktop ones by range: console GPU addresses (on the console, GPU address == CPU
/// address) to desktop GPU addresses, and descriptor handles in game memory to desktop descriptor handles. A new range
/// replaces the ones it overlaps. Lookups read a sorted array that is replaced on each addition, so they take no lock.
/// </summary>
internal sealed class AddressTable
{
    /// <summary>IUnknown::AddRef.</summary>
    private const int AddRefSlot = 1;

    private readonly object _gate = new();
    private AddressRange[] _ranges = [];
    private Action<AddressRange>? _added;

    /// <summary>Calls <paramref name="added"/> with every range added from now on, in order.</summary>
    public void Observe(Action<AddressRange> added)
    {
        lock (_gate)
        {
            _added = added;
        }
    }

    /// <summary>
    /// Adds <paramref name="range"/>, which holds a reference on its desktop resource until a later range replaces it:
    /// the game releases buffers whose addresses views, copies and the shader address map still translate, and a view
    /// made on a released buffer crashed the driver (CreateShaderResourceView, nvwgf2umx.dll, at the story load).
    /// </summary>
    public void Add(AddressRange range)
    {
        lock (_gate)
        {
            var kept = new List<AddressRange>(_ranges.Length + 1);

            foreach (var existing in _ranges)
            {
                if (existing.GameAddress + existing.Size <= range.GameAddress || range.GameAddress + range.Size <= existing.GameAddress)
                {
                    kept.Add(existing);
                }
                else
                {
                    Release(existing.Resource);
                }
            }

            AddRef(range.Resource);
            kept.Add(range);
            kept.Sort((a, b) => a.GameAddress.CompareTo(b.GameAddress));
            Volatile.Write(ref _ranges, kept.ToArray());
            _added?.Invoke(range);
        }
    }

    private static unsafe void AddRef(nint resource)
    {
        if (resource != 0)
        {
            ((delegate* unmanaged<nint, uint>)ComObject.Slot(resource, AddRefSlot))(resource);
        }
    }

    private static void Release(nint resource)
    {
        if (resource != 0)
        {
            ComObject.Release(resource);
        }
    }

    /// <summary>The desktop address of <paramref name="gameAddress"/>; false outside every known range.</summary>
    [HotPath]
    public bool TryTranslate(ulong gameAddress, out ulong desktopAddress)
    {
        desktopAddress = 0;

        if (!TryFind(gameAddress, out var range))
        {
            return false;
        }

        desktopAddress = range.DesktopAddress + (gameAddress - range.GameAddress);

        return true;
    }

    /// <summary>The range holding <paramref name="gameAddress"/>; false outside every known range.</summary>
    [HotPath]
    public bool TryFind(ulong gameAddress, out AddressRange found)
    {
        var ranges = Volatile.Read(ref _ranges);
        int low = 0;
        int high = ranges.Length - 1;

        while (low <= high)
        {
            int middle = low + ((high - low) >> 1);
            var range = ranges[middle];

            if (gameAddress < range.GameAddress)
            {
                high = middle - 1;
            }
            else if (gameAddress >= range.GameAddress + range.Size)
            {
                low = middle + 1;
            }
            else
            {
                found = range;

                return true;
            }
        }

        found = default;

        return false;
    }
}
