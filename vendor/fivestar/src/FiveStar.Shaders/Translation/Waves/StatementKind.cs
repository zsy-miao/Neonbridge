// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>The shape of one translated HLSL statement.</summary>
internal enum StatementKind
{
    /// <summary><c>[type] name = value;</c> without side effects.</summary>
    Assign,

    /// <summary><c>if (active) name = value;</c> without side effects: only active lanes write.</summary>
    Guarded,

    /// <summary>A statement with an effect outside its variables (memory, exports, barriers, messages).</summary>
    Effect,

    /// <summary>Any other statement; it may assign the names in <see cref="WindowStatement.Assigned"/>.</summary>
    Other,

    /// <summary>The <c>pc</c> bookkeeping of the dispatch loop.</summary>
    Control,
}
