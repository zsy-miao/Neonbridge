// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims;

/// <summary>
/// The mappable reservations sorted by address. Adding copies the array under a lock; lookups read the current array
/// without locking or allocating. The game frees reservations with plain VirtualFree, which the loader does not see,
/// so a new reservation drops every entry it overlaps.
/// </summary>
internal sealed class XMemReservationTable
{
    private readonly object _gate = new();
    private XMemReservation[] _sorted = [];

    public void Add(XMemReservation reservation)
    {
        lock (_gate)
        {
            var kept = _sorted.Where(entry => !entry.Overlaps(reservation.Address, reservation.Size));
            Volatile.Write(ref _sorted, kept.Append(reservation).OrderBy(entry => (ulong)entry.Address).ToArray());
        }
    }

    /// <summary>The reservation holding <paramref name="address"/>, if any.</summary>
    public XMemReservation? Find(nint address)
    {
        var sorted = Volatile.Read(ref _sorted);
        int low = 0;
        int high = sorted.Length - 1;

        while (low <= high)
        {
            int middle = low + ((high - low) >> 1);
            var entry = sorted[middle];

            if (entry.Contains(address))
            {
                return entry;
            }

            if ((ulong)address < (ulong)entry.Address)
            {
                high = middle - 1;
            }
            else
            {
                low = middle + 1;
            }
        }

        return null;
    }

    /// <summary>The reservations now, sorted by address.</summary>
    public XMemReservation[] Snapshot()
    {
        return Volatile.Read(ref _sorted);
    }

    /// <summary>Empties the table and returns what it held.</summary>
    public XMemReservation[] TakeAll()
    {
        lock (_gate)
        {
            var all = _sorted;
            Volatile.Write(ref _sorted, []);

            return all;
        }
    }
}
