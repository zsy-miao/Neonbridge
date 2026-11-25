// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The desktop pipeline state a command list set since its last Reset that a new desktop list does not inherit, kept
/// in its wrapper so a list split at a memory wait (<see cref="ListSplits"/>) can set it again on the next segment:
/// descriptor heaps, topology, vertex and index buffers, render targets, viewports, scissors, blend factor, stencil
/// reference and depth bounds, as the desktop list got them. Root signatures and the pipeline come from the binding
/// state (<see cref="Binding.CommandListBindings"/>, <see cref="RectangleListState"/>).
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct ListReplayState
{
    /// <summary>Offset in the command-list wrapper, past the game's fields and the shim's other state (0x1000 on).</summary>
    public const int Offset = 0x1100;

    public const int MaxHeaps = 2;
    public const int MaxVertexBuffers = 16;
    public const int MaxTargets = 8;
    public const int MaxViewports = 16;
    public const int ViewportFloats = 6;
    public const int RectInts = 4;
    public const int ColorFloats = 4;

    /// <summary>Bytes of a D3D12_VERTEX_BUFFER_VIEW.</summary>
    public const int ViewBytes = 16;

    public uint Set;
    public uint HeapCount;
    public fixed long Heaps[MaxHeaps];
    public uint Topology;
    public uint VertexMask;
    public VertexBufferView Index;
    public fixed byte Vertex[MaxVertexBuffers * ViewBytes];
    public uint TargetCount;
    public int SingleHandle;
    public fixed long Targets[MaxTargets];
    public long DepthStencil;
    public uint ViewportCount;
    public fixed float Viewports[MaxViewports * ViewportFloats];
    public uint ScissorCount;
    public fixed int Scissors[MaxViewports * RectInts];
    public fixed float Blend[ColorFloats];
    public uint StencilReference;
    public float DepthMin;
    public float DepthMax;

    /// <summary>Bits of <see cref="Set"/>: which state the list set.</summary>
    public const uint HeapsBit = 1;
    public const uint TopologyBit = 2;
    public const uint IndexBit = 4;
    public const uint TargetsBit = 8;
    public const uint DepthStencilBit = 16;
    public const uint ViewportsBit = 32;
    public const uint ScissorsBit = 64;
    public const uint BlendBit = 128;
    public const uint StencilBit = 256;
    public const uint DepthBoundsBit = 512;

    public static ListReplayState* Of(XboxObject* list)
    {
        return (ListReplayState*)((byte*)list + Offset);
    }

    /// <summary>The vertex buffer view of <paramref name="slot"/>.</summary>
    public VertexBufferView* VertexView(uint slot)
    {
        fixed (byte* views = Vertex)
        {
            return (VertexBufferView*)views + slot;
        }
    }
}
