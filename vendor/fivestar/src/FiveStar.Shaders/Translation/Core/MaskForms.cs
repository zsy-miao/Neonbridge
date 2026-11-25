// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// Records, block by block, the form each SGPR read assumed and each write left (<see cref="BlockForms"/>), so
/// <see cref="Stages.MaskFormAnalysis"/> can find the registers a read may reach in both forms along the program's paths.
/// </summary>
internal sealed class MaskForms
{
    private readonly Dictionary<long, BlockForms> _blocks = [];
    private BlockForms? _current;

    public IReadOnlyDictionary<long, BlockForms> Blocks => _blocks;

    /// <summary>Starts the block at <paramref name="address"/>.</summary>
    public void Begin(long address)
    {
        _current = new BlockForms();
        _blocks[address] = _current;
    }

    public void Read(RegisterKey key, bool mask)
    {
        if (key.Kind == RegisterKey.Scalar)
        {
            _current?.Read(key, mask);
        }
    }

    public void Write(RegisterKey key, bool mask)
    {
        if (key.Kind == RegisterKey.Scalar)
        {
            _current?.Write(key, mask);
        }
    }
}
