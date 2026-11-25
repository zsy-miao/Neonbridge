// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// One window of a page section (<see cref="SectionHeaps"/>): the bytes at <see cref="Start"/> of
/// <see cref="Chunk"/>'s section, its own view of them, and the heap over that view while it is open.
/// </summary>
internal sealed class SectionWindow
{
    public SectionWindow(XMemPageChunk chunk, ulong start, ulong bytes, int id)
    {
        Chunk = chunk;
        Start = start;
        Bytes = bytes;
        Id = id;
    }

    public XMemPageChunk Chunk { get; }

    /// <summary>The window's first byte in its section.</summary>
    public ulong Start { get; }

    public ulong Bytes { get; }

    /// <summary>The id waiting address-map pages name (<see cref="Binding.WindowFeedback"/>); 0 past the ids there are.</summary>
    public int Id { get; }

    /// <summary>The window's view; kept when the heap is released, so a reopen maps nothing.</summary>
    public nint View { get; set; }

    /// <summary>The heap over the view; 0 while the window is closed.</summary>
    public nint Heap { get; set; }

    /// <summary>The desktop refused a heap here; it is not asked again.</summary>
    public bool Failed { get; set; }

    /// <summary>A placed buffer of the game's lives on the heap: the window stays open.</summary>
    public bool Pinned { get; set; }

    /// <summary>The present at which the window last opened or programs last asked for it.</summary>
    public long LastWanted { get; set; }

    /// <summary>Released at a present (<see cref="WindowResidency"/>): it waits for the GPU and is not chosen again.</summary>
    public bool Retiring { get; set; }
}
