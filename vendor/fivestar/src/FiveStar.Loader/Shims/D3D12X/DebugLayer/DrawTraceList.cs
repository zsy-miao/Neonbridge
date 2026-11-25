// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>What <see cref="DrawTrace"/> saw set on one command list since it was created.</summary>
internal sealed class DrawTraceList
{
    /// <summary>The render-target view handles of the last OMSetRenderTargets.</summary>
    public nint[] Targets { get; set; } = [];

    /// <summary>The depth-stencil view handle; 0 for none.</summary>
    public nint DepthStencil { get; set; }

    public string Viewport { get; set; } = "not set";

    public string Scissor { get; set; } = "not set";

    /// <summary>The vertex buffers as trace lines.</summary>
    public string VertexBuffers { get; set; } = "\n      no vertex buffers set";

    /// <summary>How often the list was reset (Reset, slots 10 and 139).</summary>
    public long Resets { get; set; }
}
