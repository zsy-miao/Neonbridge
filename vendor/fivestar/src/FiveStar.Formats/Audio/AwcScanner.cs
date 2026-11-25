// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;

namespace FiveStar.Formats.Audio;

/// <summary>
/// Finds AWC containers stored raw in an archive, without its table of contents: an <c>ADAT</c> magic on a 16-byte
/// boundary whose header tables add up (<see cref="AwcReader"/>). A match is skipped over as a whole.
/// </summary>
public sealed class AwcScanner
{
    private const int Alignment = 16;

    private readonly IByteSource _source;

    public AwcScanner(IByteSource source)
    {
        _source = source;
    }

    /// <summary>Every container from <paramref name="start"/> on, in archive order.</summary>
    public IEnumerable<AwcContainer> Scan(long start)
    {
        var position = start;

        while (true)
        {
            var found = ByteSearch.IndexOf(_source, AwcReader.Magic, position, _source.Length);

            if (found < 0)
            {
                yield break;
            }

            AwcContainer? container = null;

            if (found % Alignment == 0)
            {
                container = AwcReader.TryRead(_source, found);
            }

            if (container is null)
            {
                position = found + 1;

                continue;
            }

            yield return container;

            position = found + container.Size;
        }
    }
}
