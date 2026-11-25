// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>What a node of a <see cref="ScopeGraph"/> stands for.</summary>
internal enum ScopeNodeKind
{
    /// <summary>A block of the scope.</summary>
    Block,

    /// <summary>A loop nested directly in the scope, collapsed to one node whose successors are its exits.</summary>
    Loop,

    /// <summary>A branch back to the header of the scope's loop.</summary>
    Continue,

    /// <summary>A branch out of the scope (out of its loop, or to the program end at the top level).</summary>
    Exit,

    /// <summary>The single sink after every continue and exit, the root of the post-dominator tree.</summary>
    Sink,
}
