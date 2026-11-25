// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Rpf;

/// <summary>What a carved entry holds, judged from its first bytes.</summary>
public enum BlobKind
{
    /// <summary>A PSO-backed paged resource (virtual segment with a <c>PRD0</c> header at +0x10).</summary>
    Prd0,

    /// <summary>Another paged resource's virtual segment (u64 0, then a pointer 0x5xxxxxxx), or an <c>RSC</c> file.</summary>
    Rsc,

    /// <summary>The physical segment that directly follows a virtual one.</summary>
    Phys,

    /// <summary>A PSO file (<c>PSIN</c>).</summary>
    Psin,

    Xml,

    /// <summary>A Bink 2 video (<c>KB2</c>).</summary>
    Bk2,

    Riff,

    Dxbc,

    /// <summary>A shader group (<c>SGD5</c>).</summary>
    Sgd5,

    /// <summary>A texture descriptor table (<c>ETPF</c>).</summary>
    Etpf,

    /// <summary>A texture page file (<c>RDS5</c>).</summary>
    Rds5,

    /// <summary>Printable text.</summary>
    Txt,

    /// <summary>Anything else.</summary>
    Bin,
}
