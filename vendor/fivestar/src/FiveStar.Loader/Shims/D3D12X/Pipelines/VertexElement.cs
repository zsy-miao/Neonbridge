// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>A D3D12_INPUT_ELEMENT_DESC.</summary>
/// <param name="Semantic">Semantic name.</param>
/// <param name="SemanticIndex">Semantic index.</param>
/// <param name="Format">DXGI_FORMAT.</param>
/// <param name="InputSlot">Vertex buffer slot.</param>
/// <param name="Offset">Aligned byte offset.</param>
/// <param name="Classification">0 per vertex, 1 per instance.</param>
/// <param name="StepRate">Instance data step rate.</param>
internal sealed record VertexElement(
    string Semantic, int SemanticIndex, int Format, int InputSlot, int Offset, int Classification, int StepRate);
