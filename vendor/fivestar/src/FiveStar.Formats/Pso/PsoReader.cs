// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Rpf;

namespace FiveStar.Formats.Pso;

/// <summary>Opens a PSO file or PRD0 resource, and sniffs what a blob is.</summary>
public static class PsoReader
{
    /// <summary>Bytes needed by <see cref="Sniff"/>.</summary>
    public const int SniffLength = 0x20;

    /// <summary>A <c>PSIN</c> file as an L-format PSO, anything else as a PRD0 resource.</summary>
    public static PsoDocument Load(byte[] data)
    {
        if (data.AsSpan().StartsWith(PsoFraming.FileTag))
        {
            return PsoFileReader.Read(data);
        }

        return PsoResourceReader.Read(data);
    }

    public static PsoKind Sniff(ReadOnlySpan<byte> head)
    {
        if (head.StartsWith(PsoFraming.FileTag))
        {
            return PsoKind.Psin;
        }

        if (BlobClassifier.HasPrd0Magic(head))
        {
            return PsoKind.Prd0;
        }

        if (BlobClassifier.IsVirtualSegment(head))
        {
            return PsoKind.Rsc;
        }

        return PsoKind.None;
    }
}
