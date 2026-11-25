// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// The reading history: a fixed ring of native IGameInputReading objects, numbered by a sequence that grows with every
/// reading. One poller writes (the service guarantees it); any thread searches. A slot is reused only when no one
/// holds a reference, so a reading the game keeps never changes under it. The writer holds one reference on the
/// latest reading of each device.
/// </summary>
internal sealed unsafe class ReadingRing : IDisposable
{
    /// <summary>How often a search retries when the reading it found was replaced before it could take a reference.</summary>
    private const int AcquireAttempts = 4;

    private readonly ReadingObject* _slots;
    private int _next;
    private long _sequence;

    public ReadingRing(nint vtable, nint owner)
    {
        _slots = (ReadingObject*)NativeMemory.AllocZeroed((nuint)(InputLimits.HistoryDepth * sizeof(ReadingObject)));

        for (int i = 0; i < InputLimits.HistoryDepth; i++)
        {
            _slots[i].Header.Vtable = vtable;
            _slots[i].Header.Owner = owner;
        }
    }

    /// <summary>The next slot nobody holds, emptied and holding the writer's reference; null when every slot is held.</summary>
    [HotPath]
    public ReadingObject* Claim()
    {
        for (int tried = 0; tried < InputLimits.HistoryDepth; tried++)
        {
            ReadingObject* slot = _slots + _next;
            _next = (_next + 1) % InputLimits.HistoryDepth;

            if (Volatile.Read(ref slot->Header.References) != 0)
            {
                continue;
            }

            long old = Interlocked.Exchange(ref slot->Sequence, 0);

            if (Interlocked.CompareExchange(ref slot->Header.References, 1, 0) == 0)
            {
                return slot;
            }

            // A reader took it between the two steps; it keeps its reading.
            Interlocked.CompareExchange(ref slot->Sequence, old, 0);
        }

        return null;
    }

    /// <summary>Makes a claimed slot visible with the next sequence number.</summary>
    [HotPath]
    public void Publish(ReadingObject* slot)
    {
        _sequence++;
        Volatile.Write(ref slot->Sequence, _sequence);
    }

    /// <summary>
    /// The matching reading with a sequence strictly between <paramref name="after"/> and <paramref name="before"/>,
    /// the newest or the oldest one, with a reference taken for the caller; null when there is none.
    /// </summary>
    [HotPath]
    public ReadingObject* Acquire(uint kind, nint device, long after, long before, bool newest)
    {
        for (int attempt = 0; attempt < AcquireAttempts; attempt++)
        {
            ReadingObject* found = Find(kind, device, after, before, newest, out long sequence);

            if (found == null)
            {
                return null;
            }

            Interlocked.Increment(ref found->Header.References);

            if (Volatile.Read(ref found->Sequence) == sequence)
            {
                return found;
            }

            Interlocked.Decrement(ref found->Header.References);
        }

        return null;
    }

    public void Dispose()
    {
        NativeMemory.Free(_slots);
    }

    [HotPath]
    private ReadingObject* Find(uint kind, nint device, long after, long before, bool newest, out long sequence)
    {
        ReadingObject* best = null;
        sequence = 0;

        for (int i = 0; i < InputLimits.HistoryDepth; i++)
        {
            ReadingObject* slot = _slots + i;
            long current = Volatile.Read(ref slot->Sequence);

            if (current <= after || current >= before || !Matches(slot, kind, device))
            {
                continue;
            }

            if (best == null || (newest && current > sequence) || (!newest && current < sequence))
            {
                best = slot;
                sequence = current;
            }
        }

        return best;
    }

    [HotPath]
    private static bool Matches(ReadingObject* slot, uint kind, nint device)
    {
        if (device != 0 && slot->Device != device)
        {
            return false;
        }

        return kind == GameInputLayout.KindUnknown || (slot->Kind & kind) != 0;
    }
}
