// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>A graphics pipeline record (0x70 bytes).</summary>
/// <param name="Index">Position in the record table.</param>
/// <param name="Hash">The u64 at 0x00, unique per record.</param>
/// <param name="StateMeta">Metadata entry at 0x08: the pipeline's fixed-function state.</param>
/// <param name="InputLayoutMeta">Metadata entry at 0x0C: the vertex input layout.</param>
/// <param name="Key">The 16 bytes at 0x10 as hex, unique per record.</param>
/// <param name="GroupA">The 16 bytes at 0x20 as hex (110 distinct values).</param>
/// <param name="GroupB">The 8 bytes at 0x30 as hex (93 distinct values).</param>
/// <param name="Stages">The used slots of the seven at 0x38.</param>
public sealed record GraphicsPipelineRecord(
    int Index,
    ulong Hash,
    int StateMeta,
    int InputLayoutMeta,
    string Key,
    string GroupA,
    string GroupB,
    IReadOnlyList<PipelineStageLink> Stages)
{
    /// <summary>The link of a slot, or null when the slot is empty.</summary>
    public PipelineStageLink? Stage(PipelineSlot slot)
    {
        foreach (var link in Stages)
        {
            if (link.Slot == slot)
            {
                return link;
            }
        }

        return null;
    }
}
