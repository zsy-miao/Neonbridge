// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// The forms (lane mask or integer) one translated block reads and leaves its SGPRs in: the form each read before the
/// block's first write of the register assumed, the form of its last write, and registers a read inside the block took
/// in another form than the block itself last wrote them in.
/// </summary>
internal sealed class BlockForms
{
    /// <summary>Registers read before any write in the block: whether a read took the mask form, the integer form.</summary>
    public Dictionary<RegisterKey, (bool Mask, bool Integer)> Exposed { get; } = [];

    /// <summary>The form of the block's last write of each register it writes (true: a lane mask).</summary>
    public Dictionary<RegisterKey, bool> Exits { get; } = [];

    /// <summary>Registers read inside the block in another form than its own last write left.</summary>
    public HashSet<RegisterKey> Conflicts { get; } = [];

    public void Read(RegisterKey key, bool mask)
    {
        if (Exits.TryGetValue(key, out var written))
        {
            if (written != mask)
            {
                Conflicts.Add(key);
            }

            return;
        }

        var (asMask, asInteger) = Exposed.GetValueOrDefault(key);
        Exposed[key] = (asMask || mask, asInteger || !mask);
    }

    public void Write(RegisterKey key, bool mask)
    {
        Exits[key] = mask;
    }
}
