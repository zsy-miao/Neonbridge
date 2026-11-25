// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>What <see cref="PassRules"/> does with a pipeline's recorded work.</summary>
internal enum PassAction
{
    /// <summary>The work runs.</summary>
    Run,

    /// <summary>The work is skipped and its targets keep what they hold.</summary>
    Hold,

    /// <summary>A draw clears its render targets to the stub's colour instead; a dispatch is skipped.</summary>
    Stub,
}
